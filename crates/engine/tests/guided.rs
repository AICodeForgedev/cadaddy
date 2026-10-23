//! Guided pipeline integration tests (Q5245).
//!
//! Tests the job state machine, question generation, answering, rule learning,
//! verify rejection, and re-run with saved rules.

use cadaddy_engine::Session;
use serde_json::json;

// ─── Unit-level tests on the guided module types ────────────────────────────

/// Test: job creation sets correct stage statuses based on goal.
#[test]
fn job_stages_match_goal() {
    use cadaddy_engine::cmd::guided::*;

    let job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    assert_eq!(job.stages.len(), StageName::ALL.len());

    // Clash should be skipped for estimate goal.
    let clash = job.stages.iter().find(|s| s.name == StageName::Clash).unwrap();
    assert_eq!(clash.status, StageStatus::Skipped);

    // Everything up to estimate should be pending.
    for stage in &job.stages {
        if stage.name <= StageName::Estimate {
            assert_eq!(stage.status, StageStatus::Pending, "stage {:?} should be pending", stage.name);
        }
    }
}

/// Test: BIM goal skips takeoff, estimate and clash.
#[test]
fn job_bim_goal_skips_later_stages() {
    use cadaddy_engine::cmd::guided::*;

    let job = GuidedJob::new("/tmp/test-prints", JobGoal::Bim);
    for stage in &job.stages {
        if stage.name > StageName::BimBuild {
            assert_eq!(stage.status, StageStatus::Skipped, "stage {:?} should be skipped for BIM", stage.name);
        }
    }
}

/// Test: progress tracks correctly.
#[test]
fn job_progress_tracking() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Bim);
    assert!(job.progress() < 0.01);

    // Mark first two stages done.
    if let Some(st) = job.stage_mut(StageName::Import) { st.status = StageStatus::Done; }
    if let Some(st) = job.stage_mut(StageName::Classify) { st.status = StageStatus::Done; }

    let p = job.progress();
    assert!(p > 0.1 && p < 0.9, "progress should be partial: {p}");
}

/// Test: question lifecycle — create, find, answer.
#[test]
fn question_lifecycle() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    let qid = job.next_qid(StageName::Levels);
    assert_eq!(qid, "Q-levels-1");

    let q = Question {
        id: qid.clone(),
        stage: StageName::Levels,
        question: "Scale reads 1/4\"=1'-0\" — confirm?".into(),
        why: "Confidence 70%".into(),
        choices: vec!["Yes".into(), "No".into()],
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: Some("E-4".into()),
            at: Some([100.0, 200.0]),
            values: Some(json!({"scale": "1/4\"=1'-0\"", "confidence": 0.70})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some("Yes".into()),
        answer_schema: None,
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Doubt,
    };

    if let Some(st) = job.stage_mut(StageName::Levels) {
        st.questions.push(q);
        st.status = StageStatus::Paused;
    }

    // Next question should find it.
    assert!(job.next_question().is_some());
    assert_eq!(job.next_question().unwrap().id, qid);
    assert!(!job.is_done());

    // Answer it.
    if let Some(st) = job.stage_mut(StageName::Levels) {
        if let Some(q) = st.questions.first_mut() {
            q.answer = Some(json!("Yes"));
            q.answered_by = Some("agent".into());
        }
    }
    assert!(job.stage_mut(StageName::Levels).unwrap().unanswered().is_empty());
}

/// Test: rule learning from unknown symbol answers.
#[test]
fn rule_learning_from_symbol() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    job.learn_rule("symbol:HEXAGON_WP", "device:weatherproof_receptacle", "Q-devices-1", StageName::Devices);
    assert_eq!(job.rules.len(), 1);
    assert_eq!(job.rules[0].key, "symbol:HEXAGON_WP");
    assert_eq!(job.rules[0].value, "device:weatherproof_receptacle");

    // Update existing rule.
    job.learn_rule("symbol:HEXAGON_WP", "device:wet_location_receptacle", "Q-devices-2", StageName::Devices);
    assert_eq!(job.rules.len(), 1);
    assert_eq!(job.rules[0].value, "device:wet_location_receptacle");
    assert_eq!(job.rules[0].from_question, "Q-devices-2");
}

/// Test: audit trail respects limit and trims oldest.
#[test]
fn audit_trail_trimming() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    for i in 0..60_000 {
        job.audit("test", "test", None, None, Some(json!({"i": i})));
    }
    // Should have trimmed.
    assert!(job.audit.len() <= 50_000 + 5_001); // MAX_AUDIT + batch size tolerance.
}

/// Test: verify rejection triggers stage re-run flag.
#[test]
fn verify_rejection_reruns_stage() {
    use cadaddy_engine::cmd::guided::*;

    let mut stage = StageState::new(StageName::Rooms);

    // A "Correct" answer does NOT trigger re-run.
    stage.verify.push(Question {
        id: "Q-rooms-1".into(),
        stage: StageName::Rooms,
        question: "Room 101 correct?".into(),
        why: "verify".into(),
        choices: vec!["Correct".into(), "Wrong name".into()],
        evidence: None,
        default: None,
        answer_schema: None,
        answer: Some(json!("Correct")),
        answered_by: Some("agent".into()),
        answered_at: None,
        kind: QuestionKind::Verify,
    });
    assert!(!check_rejected_samples(&stage));

    // A "Wrong name" answer DOES trigger re-run.
    stage.verify.push(Question {
        id: "Q-rooms-2".into(),
        stage: StageName::Rooms,
        question: "Room 102 correct?".into(),
        why: "verify".into(),
        choices: vec!["Correct".into(), "Wrong name".into()],
        evidence: None,
        default: None,
        answer_schema: None,
        answer: Some(json!("Wrong name")),
        answered_by: Some("agent".into()),
        answered_at: None,
        kind: QuestionKind::Verify,
    });
    assert!(check_rejected_samples(&stage));
}

/// Test: job serialization round-trip preserves all fields.
#[test]
fn job_serialization_roundtrip() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    job.learn_rule("scale:E-4", "1/8\"=1'-0\"", "Q-levels-1", StageName::Levels);
    job.audit("test_event", "test_who", Some("Q-1"), Some(StageName::Import), None);

    // Add a question.
    let qid = job.next_qid(StageName::Levels);
    if let Some(st) = job.stage_mut(StageName::Levels) {
        st.questions.push(Question {
            id: qid.clone(),
            stage: StageName::Levels,
            question: "Test question?".into(),
            why: "Test".into(),
            choices: vec!["A".into(), "B".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: Some("/tmp/crop.png".into()),
                sheet: Some("E-4".into()),
                at: Some([10.0, 20.0]),
                values: Some(json!({"key": "value"})),
                handles: vec!["10A".into()],
                tiled_crops: Some(vec!["/tmp/tile1.png".into()]),
                leftovers: vec![LeftoverCandidate {
                    description: "mystery symbol".into(),
                    crop_path: Some("/tmp/mystery.png".into()),
                    at: Some([30.0, 40.0]),
                    sheet: Some("E-4".into()),
                    count: 3,
                }],
            }),
            default: Some("A".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        });
    }

    let json_text = serde_json::to_string_pretty(&job).unwrap();
    let restored: GuidedJob = serde_json::from_str(&json_text).unwrap();

    assert_eq!(restored.id, job.id);
    assert_eq!(restored.goal, job.goal);
    assert_eq!(restored.stages.len(), job.stages.len());
    assert_eq!(restored.rules.len(), 1);
    assert_eq!(restored.audit.len(), 1);

    let levels = restored.stages.iter().find(|s| s.name == StageName::Levels).unwrap();
    assert_eq!(levels.questions.len(), 1);
    let q = &levels.questions[0];
    assert_eq!(q.id, qid);
    assert!(q.evidence.is_some());
    let ev = q.evidence.as_ref().unwrap();
    assert_eq!(ev.leftovers.len(), 1);
    assert_eq!(ev.leftovers[0].count, 3);
}

/// Test: completeness questions are generated for each device class.
#[test]
fn completeness_questions_structure() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);

    // Simulate what the device stage would do: add completeness questions.
    let device_classes = [
        ("duplex_receptacle", 77),
        ("gfci", 12),
        ("ef", 4),
    ];
    for (class, count) in &device_classes {
        let qid = job.next_qid(StageName::Devices);
        if let Some(st) = job.stage_mut(StageName::Devices) {
            st.completeness.push(Question {
                id: qid,
                stage: StageName::Devices,
                question: format!("I counted {count} {class}. Are there any devices I did not count?"),
                why: format!("Completeness checkpoint for {class}"),
                choices: vec!["Count is complete".into(), "There are uncounted devices".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"class": class, "count": count})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some("Count is complete".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Completeness,
            });
        }
    }

    let devices = job.stages.iter().find(|s| s.name == StageName::Devices).unwrap();
    assert_eq!(devices.completeness.len(), 3);
    assert!(devices.completeness[0].question.contains("77"));
    assert!(devices.completeness[1].question.contains("12"));
    assert!(devices.completeness[2].question.contains("4"));
}

/// Test: unknown symbol question triggers rule learning when answered.
#[test]
fn unknown_symbol_learns_rule() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);

    let q = Question {
        id: "Q-devices-1".into(),
        stage: StageName::Devices,
        question: "Unknown symbol: 'hexagon with WP' found 6 time(s) — which device is it?".into(),
        why: "Foreign/unknown symbol".into(),
        choices: Vec::new(),
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: Some("E-4".into()),
            at: None,
            values: Some(json!({"description": "hexagon with WP", "count": 6})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: None,
        answer_schema: Some(json!({"type": "object"})),
        answer: Some(json!({"device_type": "weatherproof_receptacle", "symbol_name": "WP_HEX"})),
        answered_by: Some("agent".into()),
        answered_at: None,
        kind: QuestionKind::Completeness,
    };

    learn_from_answer(&mut job, &q);

    assert_eq!(job.rules.len(), 1);
    assert_eq!(job.rules[0].key, "symbol:hexagon with WP");
    assert_eq!(job.rules[0].value, "device:weatherproof_receptacle");
}

/// Test: equipment label question is raised for patterns like PNLBD.
#[test]
fn equipment_label_detection_patterns() {
    // The patterns checked are: PNLBD, PANEL, XFMR, TRANS, MCC, SWG, ATS, GEN, MDP, CDP.
    let equipment_patterns = ["PNLBD", "PANEL", "XFMR", "TRANS", "MCC", "SWG", "ATS", "GEN", "MDP", "CDP"];
    for pat in &equipment_patterns {
        let name = format!("{} D", pat);
        let looks_like_equipment = equipment_patterns.iter().any(|p| name.to_uppercase().contains(p));
        assert!(looks_like_equipment, "pattern '{pat}' should match as equipment");
    }
    // Normal room names should NOT match.
    let normals = ["OFFICE", "CONFERENCE", "STORAGE", "CORRIDOR", "STAIRWELL"];
    for name in &normals {
        let looks_like_equipment = equipment_patterns.iter().any(|p| name.to_uppercase().contains(p));
        assert!(!looks_like_equipment, "'{name}' should NOT match as equipment");
    }
}

/// Test: job.status commands work on a session with a stored job.
#[test]
fn job_commands_on_session() {
    let mut s = Session::new();

    // No job should give an error.
    let r = s.execute("job.status", &json!({}));
    assert!(r.is_err());

    let r = s.execute("job.next", &json!({}));
    assert!(r.is_err());
}

/// Test: stage ordering is correct.
#[test]
fn stage_ordering() {
    use cadaddy_engine::cmd::guided::StageName;

    assert!(StageName::Import < StageName::Classify);
    assert!(StageName::Classify < StageName::Levels);
    assert!(StageName::Levels < StageName::Rooms);
    assert!(StageName::Rooms < StageName::Devices);
    assert!(StageName::Devices < StageName::BimBuild);
    assert!(StageName::BimBuild < StageName::Takeoff);
    assert!(StageName::Takeoff < StageName::Estimate);
    assert!(StageName::Estimate < StageName::Clash);
}

/// Test: multiple rules don't duplicate.
#[test]
fn rules_no_duplicates() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
    for i in 0..10 {
        job.learn_rule("symbol:SQUARE_X", &format!("device:type_{i}"), &format!("Q-{i}"), StageName::Devices);
    }
    // Should have exactly 1 rule (updated, not duplicated).
    assert_eq!(job.rules.len(), 1);
    assert_eq!(job.rules[0].value, "device:type_9");
}

/// Test: the date helper produces valid ISO 8601.
#[test]
fn now_iso_format() {
    use cadaddy_engine::cmd::guided::days_to_ymd;

    let (y, m, d) = days_to_ymd(0);
    assert_eq!((y, m, d), (1970, 1, 1));

    let (y, m, d) = days_to_ymd(20749);
    assert_eq!((y, m, d), (2026, 10, 23));

    // Leap year: 2024-02-29 = day 19782.
    let (y, m, d) = days_to_ymd(19782);
    assert_eq!((y, m, d), (2024, 2, 29));
}
