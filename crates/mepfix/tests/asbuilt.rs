//! Integration tests for the as-built system: synthetic markups on generated fixture sets,
//! markup import, change generation, design-vs-asbuilt comparison, as-built truth application,
//! overlay marks, IFC properties, and summary statistics.

use cadaddy_mepfix::asbuilt::*;
use cadaddy_mepfix::truth::Truth;

fn fixture_truth() -> Truth {
    cadaddy_mepfix::generate(1)
        .map(|fs| fs.truth)
        .unwrap_or_else(|_| cadaddy_mepfix::truth_for(1))
}

/// Build a set of synthetic markups placed on known elements of a generated fixture set.
fn synthetic_markups_on_truth(t: &Truth) -> Vec<(String, Vec<SyntheticMarkup>)> {
    let mut per_sheet = Vec::new();
    for sh in &t.sheets {
        let level = match &sh.level {
            Some(l) => l.as_str(),
            None => continue,
        };
        let mut markups = Vec::new();
        // Cloud on the first segment of this level
        if let Some(seg) = t.segments.iter().find(|s| s.level == level) {
            let bldg = [
                (seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5,
                (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5,
            ];
            let paper = building_to_paper(sh, bldg);
            markups.push(SyntheticMarkup {
                kind: "cloud".into(),
                text: "Move duct 2 feet north per field decision".into(),
                author: "Foreman A".into(),
                date: "2026-10-20".into(),
                paper_in: paper,
                paper_rect_in: [
                    [paper[0] - 1.0, paper[1] - 1.0],
                    [paper[0] + 1.0, paper[1] + 1.0],
                ],
            });
        }
        // Arrow on the first electrical device of this level
        if let Some(dev) = t.elec_devices.iter().find(|d| d.level == level) {
            let paper = building_to_paper(sh, [dev.at_ft[0], dev.at_ft[1]]);
            markups.push(SyntheticMarkup {
                kind: "arrow".into(),
                text: "Relocate receptacle to opposite wall".into(),
                author: "Electrician B".into(),
                date: "2026-10-20".into(),
                paper_in: paper,
                paper_rect_in: [
                    [paper[0] - 0.5, paper[1] - 0.5],
                    [paper[0] + 0.5, paper[1] + 0.5],
                ],
            });
        }
        // Callout on the first terminal of this level
        if let Some(term) = t.terminals.iter().find(|tm| tm.level == level) {
            let paper = building_to_paper(sh, [term.at_ft[0], term.at_ft[1]]);
            markups.push(SyntheticMarkup {
                kind: "callout".into(),
                text: "Verified diffuser location in field".into(),
                author: "Inspector C".into(),
                date: "2026-10-21".into(),
                paper_in: paper,
                paper_rect_in: [
                    [paper[0] - 0.3, paper[1] - 0.3],
                    [paper[0] + 0.3, paper[1] + 0.3],
                ],
            });
        }
        // Note on a conduit
        if let Some(cond) = t.elec_conduits.iter().find(|c| c.level == level) {
            let mid = [
                (cond.from_ft[0] + cond.to_ft[0]) * 0.5,
                (cond.from_ft[1] + cond.to_ft[1]) * 0.5,
            ];
            let paper = building_to_paper(sh, mid);
            markups.push(SyntheticMarkup {
                kind: "note".into(),
                text: "Change conduit type from EMT to RGS per RFI-42".into(),
                author: "Engineer D".into(),
                date: "2026-10-21".into(),
                paper_in: paper,
                paper_rect_in: [
                    [paper[0] - 0.3, paper[1] - 0.3],
                    [paper[0] + 0.3, paper[1] + 0.3],
                ],
            });
        }
        if !markups.is_empty() {
            per_sheet.push((sh.number.clone(), markups));
        }
    }
    per_sheet
}

#[test]
fn full_workflow_synthetic_markups() {
    let t = fixture_truth();
    let markup_sets = synthetic_markups_on_truth(&t);
    assert!(!markup_sets.is_empty(), "should have markups on at least one sheet");

    let mut record = AsBuiltRecord::new(&t.project);
    let mut change_counter = 0usize;

    // Import markups from each sheet
    for (sheet_num, markups) in &markup_sets {
        let Some(sh) = t.sheets.iter().find(|s| s.number == *sheet_num) else {
            continue;
        };
        let mapped = import_markups(&t, sh, markups);
        for m in &mapped {
            // Every mapped markup should have the right sheet
            assert_eq!(m.sheet, *sheet_num);
            // Every mapped markup should have a non-empty level
            assert!(!m.level.is_empty());
            // Building coordinates should be finite
            assert!(m.building_ft[0].is_finite() && m.building_ft[1].is_finite());

            // Convert to change
            change_counter += 1;
            let change_id = format!("C-{change_counter}");
            if let Some(change) = markup_to_change(m, &t, &change_id) {
                assert_eq!(change.markup_id.as_deref(), Some(m.id.as_str()));
                record.add_change(change);
            }
        }
        // Store mapped markups
        record.markups.extend(mapped);
    }

    assert!(!record.changes.is_empty(), "should have generated some changes");
    assert!(!record.markups.is_empty(), "should have imported some markups");

    // Compare design vs as-built
    let deviations = compare_design_vs_asbuilt(&t, &record);
    assert_eq!(
        deviations.len(),
        record.changes.iter().filter(|c| c.status != ChangeStatus::Rejected).count()
    );

    // Apply changes to produce as-built truth
    let ab = asbuilt_truth(&t, &record);
    // The as-built truth should still be valid (no NaN positions, etc.)
    for seg in &ab.segments {
        assert!(seg.path_ft[0][0].is_finite());
        assert!(seg.path_ft[1][0].is_finite());
    }
    for dev in &ab.elec_devices {
        assert!(dev.at_ft[0].is_finite());
    }

    // Generate overlay marks for each level
    for level in &t.building.levels {
        let marks = asbuilt_overlay(&record, &level.code);
        for m in &marks {
            assert!(!m.change_id.is_empty());
            assert!(!m.label.is_empty());
        }
    }

    // IFC properties for each change
    for change in &record.changes {
        let props = asbuilt_ifc_properties(change);
        assert!(props.contains_key("AsBuiltChangeId"));
        assert!(props.contains_key("AsBuiltStatus"));
        assert!(props.contains_key("AsBuiltDescription"));
    }

    // Summary
    let summary = summarize(&record);
    assert_eq!(summary.total_changes, record.changes.len());
    // At least one level should have changes
    assert!(!summary.levels.is_empty());

    // Serde round-trip of the full record
    let json = serde_json::to_string_pretty(&record).unwrap();
    let back: AsBuiltRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(record.changes.len(), back.changes.len());
    assert_eq!(record.markups.len(), back.markups.len());
}

#[test]
fn markups_on_multiple_seeds() {
    for seed in 1..=5 {
        let t = match cadaddy_mepfix::generate(seed) {
            Ok(fs) => fs.truth,
            Err(_) => continue,
        };
        let markup_sets = synthetic_markups_on_truth(&t);
        let mut record = AsBuiltRecord::new(&t.project);
        let mut n = 0usize;
        for (sheet_num, markups) in &markup_sets {
            let Some(sh) = t.sheets.iter().find(|s| s.number == *sheet_num) else {
                continue;
            };
            for m in import_markups(&t, sh, markups) {
                n += 1;
                if let Some(c) = markup_to_change(&m, &t, &format!("C-{n}")) {
                    record.add_change(c);
                }
            }
        }
        // Every seed with plan sheets should produce some changes
        if !markup_sets.is_empty() {
            assert!(
                !record.changes.is_empty(),
                "seed {seed}: expected changes from markups"
            );
        }
        // As-built truth should not panic
        let _ = asbuilt_truth(&t, &record);
    }
}

#[test]
fn cloud_on_segment_produces_move() {
    let t = fixture_truth();
    let Some(sh) = t.sheets.iter().find(|s| s.level.is_some()) else {
        return;
    };
    let level = sh.level.as_deref().unwrap_or("1");
    let Some(seg) = t.segments.iter().find(|s| s.level == level) else {
        return;
    };
    let bldg = [
        (seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5,
        (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5,
    ];
    let paper = building_to_paper(sh, bldg);
    let markup = SyntheticMarkup {
        kind: "cloud".into(),
        text: "Move this duct run 3 feet east".into(),
        author: "PM".into(),
        date: "2026-10-21".into(),
        paper_in: paper,
        paper_rect_in: [[paper[0] - 0.5, paper[1] - 0.5], [paper[0] + 0.5, paper[1] + 0.5]],
    };
    let mapped = import_markups(&t, sh, &[markup]);
    assert_eq!(mapped.len(), 1);
    assert!(mapped[0].near_elements.contains(&seg.id));

    let change = markup_to_change(&mapped[0], &t, "C-1");
    assert!(change.is_some());
    let c = change.unwrap();
    assert!(matches!(c.kind, ChangeKind::Moved { .. }));
    assert_eq!(c.element_kind, "segment");
}

#[test]
fn arrow_on_device_produces_move() {
    let t = fixture_truth();
    let Some(sh) = t.sheets.iter().find(|s| s.level.is_some()) else {
        return;
    };
    let level = sh.level.as_deref().unwrap_or("1");
    let Some(dev) = t.elec_devices.iter().find(|d| d.level == level) else {
        return;
    };
    let paper = building_to_paper(sh, [dev.at_ft[0], dev.at_ft[1]]);
    let markup = SyntheticMarkup {
        kind: "arrow".into(),
        text: "Move switch here".into(),
        author: "EC".into(),
        date: "2026-10-21".into(),
        paper_in: paper,
        paper_rect_in: [[paper[0] - 0.3, paper[1] - 0.3], [paper[0] + 0.3, paper[1] + 0.3]],
    };
    let mapped = import_markups(&t, sh, &[markup]);
    assert_eq!(mapped.len(), 1);
    assert!(mapped[0].near_elements.contains(&dev.id));

    let change = markup_to_change(&mapped[0], &t, "C-1");
    assert!(change.is_some());
    let c = change.unwrap();
    assert!(matches!(c.kind, ChangeKind::Moved { .. }));
    // The nearest element might be a segment crossing the same point; the device is in the list
    assert!(mapped[0].near_elements.contains(&dev.id));
}

#[test]
fn callout_verified_produces_field_verified() {
    let t = fixture_truth();
    let Some(sh) = t.sheets.iter().find(|s| s.level.is_some()) else {
        return;
    };
    let level = sh.level.as_deref().unwrap_or("1");
    let Some(term) = t.terminals.iter().find(|tm| tm.level == level) else {
        return;
    };
    let paper = building_to_paper(sh, [term.at_ft[0], term.at_ft[1]]);
    let markup = SyntheticMarkup {
        kind: "callout".into(),
        text: "Verified: diffuser at correct location".into(),
        author: "QC".into(),
        date: "2026-10-21".into(),
        paper_in: paper,
        paper_rect_in: [[paper[0] - 0.3, paper[1] - 0.3], [paper[0] + 0.3, paper[1] + 0.3]],
    };
    let mapped = import_markups(&t, sh, &[markup]);
    assert_eq!(mapped.len(), 1);
    assert!(mapped[0].near_elements.contains(&term.id));

    let change = markup_to_change(&mapped[0], &t, "C-1");
    assert!(change.is_some());
    let c = change.unwrap();
    assert!(matches!(c.kind, ChangeKind::FieldVerified { .. }));
    // The nearest element might be a segment; the terminal is in the list
    assert!(mapped[0].near_elements.contains(&term.id));
}

#[test]
fn delete_markup_produces_deletion() {
    let t = fixture_truth();
    let Some(dev) = t.elec_devices.first() else {
        return;
    };
    let markup = MappedMarkup {
        id: "MK-DEL".into(),
        kind: "cloud".into(),
        text: "Delete this receptacle - not needed".into(),
        author: "PM".into(),
        date: "2026-10-21".into(),
        sheet: "E-101".into(),
        paper_pt: [5.0, 5.0],
        building_ft: [dev.at_ft[0], dev.at_ft[1]],
        level: dev.level.clone(),
        near_elements: vec![dev.id.clone()],
        bounds_ft: [[dev.at_ft[0] - 1.0, dev.at_ft[1] - 1.0], [dev.at_ft[0] + 1.0, dev.at_ft[1] + 1.0]],
    };
    let change = markup_to_change(&markup, &t, "C-DEL");
    assert!(change.is_some());
    let c = change.unwrap();
    assert!(matches!(c.kind, ChangeKind::Deleted));

    // Apply it
    let mut rec = AsBuiltRecord::new(&t.project);
    rec.add_change(c);
    let ab = asbuilt_truth(&t, &rec);
    assert!(ab.elec_devices.iter().all(|d| d.id != dev.id));
}

#[test]
fn type_change_markup() {
    let t = fixture_truth();
    let Some(dev) = t.elec_devices.first() else {
        return;
    };
    let markup = MappedMarkup {
        id: "MK-TYPE".into(),
        kind: "cloud".into(),
        text: "Change type to Type C fixture".into(),
        author: "Eng".into(),
        date: "2026-10-21".into(),
        sheet: "E-101".into(),
        paper_pt: [5.0, 5.0],
        building_ft: [dev.at_ft[0], dev.at_ft[1]],
        level: dev.level.clone(),
        near_elements: vec![dev.id.clone()],
        bounds_ft: [[dev.at_ft[0] - 1.0, dev.at_ft[1] - 1.0], [dev.at_ft[0] + 1.0, dev.at_ft[1] + 1.0]],
    };
    let change = markup_to_change(&markup, &t, "C-TYPE");
    assert!(change.is_some());
    let c = change.unwrap();
    assert!(matches!(c.kind, ChangeKind::TypeChanged { .. }));
}

#[test]
fn asbuilt_record_json_round_trip_full() {
    let t = fixture_truth();
    let mut rec = AsBuiltRecord::new(&t.project);
    // Add various change types
    let changes = [
        ChangeKind::Moved { from_ft: [10.0, 20.0, 8.0], to_ft: [12.0, 20.0, 8.0] },
        ChangeKind::Rerouted {
            design_path_ft: [[5.0, 5.0, 8.0], [15.0, 5.0, 8.0]],
            asbuilt_path_ft: [[5.0, 7.0, 8.0], [15.0, 7.0, 8.0]],
        },
        ChangeKind::TypeChanged { design_type: "EMT".into(), asbuilt_type: "RGS".into() },
        ChangeKind::SizeChanged { design_size: "3/4\"".into(), asbuilt_size: "1\"".into() },
        ChangeKind::Added,
        ChangeKind::Deleted,
        ChangeKind::FieldVerified { at_ft: [25.0, 30.0, 4.0] },
        ChangeKind::HeightChanged { design_ft: 10.5, asbuilt_ft: 11.0 },
        ChangeKind::Note,
    ];
    for (i, kind) in changes.into_iter().enumerate() {
        rec.add_change(AsBuiltChange {
            id: format!("C-{i}"),
            element_id: format!("e-{i}"),
            element_kind: "segment".into(),
            kind,
            status: [
                ChangeStatus::Pending,
                ChangeStatus::FieldDecision,
                ChangeStatus::RfiAnswer,
                ChangeStatus::ChangeOrder,
                ChangeStatus::Rejected,
            ][i % 5],
            author: format!("Author {i}"),
            date: "2026-10-21".into(),
            reason: format!("Reason {i}"),
            notes: format!("Notes {i}"),
            level: "1".into(),
            at_ft: [i as f64, 0.0],
            markup_id: if i % 2 == 0 { Some(format!("MK-{i}")) } else { None },
        });
    }
    let json = serde_json::to_string_pretty(&rec).unwrap();
    let back: AsBuiltRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec, back);
}

#[test]
fn coordinate_transform_consistency() {
    // Test that building_to_paper and paper_to_building are inverses on multiple sheets
    let t = fixture_truth();
    for sh in &t.sheets {
        if sh.level.is_none() {
            continue;
        }
        let level = sh.level.as_deref().unwrap_or("1");
        for seg in t.segments.iter().filter(|s| s.level == level).take(5) {
            let bldg = [
                (seg.path_ft[0][0] + seg.path_ft[1][0]) * 0.5,
                (seg.path_ft[0][1] + seg.path_ft[1][1]) * 0.5,
            ];
            let paper = building_to_paper(sh, bldg);
            let back = paper_to_building(sh, paper);
            let err = ((back[0] - bldg[0]).powi(2) + (back[1] - bldg[1]).powi(2)).sqrt();
            assert!(
                err < 0.01,
                "sheet {}: round trip error {err:.4} for bldg {bldg:?}",
                sh.number
            );
        }
    }
}

#[test]
fn mixed_statuses_in_comparison() {
    let t = fixture_truth();
    let mut rec = AsBuiltRecord::new(&t.project);
    // Add changes with different statuses
    let statuses = [
        ChangeStatus::FieldDecision,
        ChangeStatus::Rejected,
        ChangeStatus::RfiAnswer,
        ChangeStatus::ChangeOrder,
        ChangeStatus::Pending,
    ];
    for (i, status) in statuses.iter().enumerate() {
        rec.add_change(AsBuiltChange {
            id: format!("C-{i}"),
            element_id: format!("e-{i}"),
            element_kind: "segment".into(),
            kind: ChangeKind::Note,
            status: *status,
            author: String::new(),
            date: String::new(),
            reason: format!("Status test {i}"),
            notes: String::new(),
            level: "1".into(),
            at_ft: [0.0, 0.0],
            markup_id: None,
        });
    }
    let devs = compare_design_vs_asbuilt(&t, &rec);
    // Rejected should be excluded
    assert_eq!(devs.len(), 4);
}
