//! Q6518: a plan's scale is the note under its own view's title; a detail's note never scales the
//! plan beside it. A plan whose note doesn't read takes its scale from evidence: its dimension
//! strings, the fit of its linework onto its floor's plan, then the plans of its trade on paper
//! of its size; a scale its floor's plan contradicts is asked about with the candidates listed.
//! Made-up sheets of the generated set only: its title block's scale taken off, details at their
//! own scale drawn beside the plan (titled, or a bare note under a number bubble), the plan's own
//! note or every scale lettering taken off (a set lettered in strokes that don't read).

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, HAlign, Space};
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::{draw, layout};
use serde_json::{Value, json};

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// The details drawn beside a sheet's plan.
#[derive(Clone, Copy, Debug)]
enum Details {
    /// each titled (`CEILING DETAIL 1`) over its `SCALE:` note
    Titled,
    /// a bare `3" = 1'-0"` under a number bubble, no title
    Bare,
}

/// What is done to one sheet of the generated set.
#[derive(Clone, Copy, Debug, Default)]
struct Change {
    /// the title block's `SCALE:` line taken off (a set whose title block says nothing)
    title_block: bool,
    /// the plan's own `SCALE:` line, under its title, taken off
    plan_note: bool,
    /// every scale lettering taken off (no note, no scale bar, no dimension)
    all: bool,
    details: Option<(Details, usize, &'static str)>,
}

const AS_PRINTED: Change = Change { title_block: false, plan_note: false, all: false, details: None };

/// Details beside the plan, the title block silent.
fn with_details(kind: Details, n: usize, scale: &'static str) -> Change {
    Change { title_block: true, details: Some((kind, n, scale)), ..AS_PRINTED }
}

/// The generated set's sheets `only`, each changed as said, as one PDF in a new folder.
fn set(tag: &str, only: &[(&str, Change)]) -> std::path::PathBuf {
    let (t, st) = cadaddy_mepfix::hotel_planned(42, HotelSize::Small);
    let kinds: Vec<(String, layout::Kind)> = layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let mut pages = Vec::new();
    for sh in t.sheets.iter() {
        let Some((_, c)) = only.iter().find(|(n, _)| *n == sh.number) else { continue };
        let kind = &kinds.iter().find(|(n, _)| *n == sh.number).expect("a generated sheet").1;
        let (mut drawing, _, _) = draw::draw(&t, &st, sh, kind);
        let gone: Vec<_> = drawing
            .model
            .iter()
            .filter(|e| match &e.kind {
                EntityKind::Text(x) => {
                    let u = x.value.to_ascii_uppercase();
                    (c.title_block && u.starts_with("SCALE") && e.common.layer == "TITLE")
                        || (c.plan_note && u.starts_with("SCALE") && e.common.layer == "TEXT")
                        || (c.all && (u.contains("SCALE") || x.value.contains('\'') || x.value.contains('"')))
                }
                _ => false,
            })
            .map(|e| e.handle)
            .collect();
        for h in gone {
            drawing.model.remove(h);
        }
        if let Some((details, n, scale)) = c.details {
            let mut pen = draw::Pen::new(sh, &st);
            let [w, h] = sh.paper_in;
            for i in 0..n {
                let (x, y) = (w * 0.30 + i as f64 * 3.4, h * 0.10);
                let (a, b) = (pen.p([x, y]), pen.p([x + 2.6, y + 1.6]));
                pen.rect("A-DETAIL", a, b);
                for k in 1..5 {
                    let yy = y + 0.3 * f64::from(k);
                    pen.pline("A-DETAIL", [x + 0.2, yy], [x + 2.4, yy + 0.05]);
                }
                pen.circle("TEXT", pen.p([x + 0.2, y - 0.35]), pen.pl(0.2));
                pen.ptext("TEXT", [x + 0.2, y - 0.4], 0.12, &format!("{}", i + 2), HAlign::Center);
                match details {
                    Details::Titled => {
                        pen.ptext("TEXT", [x + 0.55, y - 0.35], 0.16, &format!("CEILING DETAIL {}", i + 1), HAlign::Left);
                        pen.ptext("TEXT", [x + 0.55, y - 0.6], 0.1, &format!("SCALE: {scale} = 1'-0\""), HAlign::Left);
                    }
                    Details::Bare => pen.ptext("TEXT", [x + 0.55, y - 0.6], 0.1, &format!("{scale} = 1'-0\""), HAlign::Left),
                }
            }
            drawing.model.push_many(pen.d.model.iter().map(|e| (**e).clone()).collect());
            for l in pen.d.layers {
                if !drawing.layers.iter().any(|x| x.name == l.name) {
                    drawing.layers.push(l);
                }
            }
        }
        let pdf = cadaddy_io::pdf::pdf(&drawing, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("sheet pdf");
        pages.push((pdf, format!("{} {}", sh.number, sh.title), String::new()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_detailscale_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("set");
    dir
}

/// The job (auto, to the model) on the set: its state.
fn job(dir: &std::path::Path) -> Value {
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}));
    let _ = std::fs::remove_dir_all(dir);
    st.expect("job.start");
    let agent = s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).unwrap_or_default();
    serde_json::from_str(&agent).expect("job state")
}

/// How the job read sheet `n`'s scale.
fn scale_of(state: &Value, n: &str) -> Value {
    state["guided_job"]["sheet_scales"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == n)).cloned().unwrap_or_default()
}

/// Every question the job asked.
fn questions(state: &Value) -> Vec<Value> {
    let stages = state["guided_job"]["stages"].as_array().cloned().unwrap_or_default();
    stages.iter().flat_map(|st| st["completeness"].as_array().cloned().unwrap_or_default()).collect()
}

/// 3/16" = 1'-0": the generated plans' scale, drawing units a sheet inch.
const PLANS: f64 = 64.0;

#[test]
fn a_plans_scale_is_its_own_views_note_never_its_details() {
    let _g = serial();
    real_pdf_reader();
    // The ceiling plan beside three 3" details, the mechanical plan beside a 1/2" one; neither
    // title block prints a scale. Read at the details' note they came in at 4x and 24x.
    let dir = set(
        "own",
        &[
            ("A-101", AS_PRINTED),
            ("A-201", with_details(Details::Titled, 3, "3\"")),
            ("M-101", with_details(Details::Titled, 1, "1/2\"")),
            ("E-101", AS_PRINTED),
        ],
    );
    let state = job(&dir);
    for n in ["A-201", "M-101"] {
        let sc = scale_of(&state, n);
        assert_eq!(sc["scale"].as_f64(), Some(PLANS), "{n}: {sc}");
        let src = sc["source"].as_str().unwrap_or("");
        assert!(src.contains("the scale the sheet's plans print"), "{n}: {src}");
    }
}

#[test]
fn a_plan_whose_only_notes_are_its_details_is_scaled_from_its_floors_plan() {
    let _g = serial();
    real_pdf_reader();
    // Its own note gone too: the details' 3" is no plan's, so the sheet is read without its
    // notes and takes its scale from the floor plan its linework lies on.
    let none = Change { plan_note: true, ..with_details(Details::Titled, 3, "3\"") };
    let dir = set("details_only", &[("A-101", AS_PRINTED), ("A-201", none), ("M-101", none), ("E-101", AS_PRINTED)]);
    let state = job(&dir);
    for n in ["A-201", "M-101"] {
        let sc = scale_of(&state, n);
        assert_eq!(sc["scale"].as_f64(), Some(PLANS), "{n}: {sc}");
        assert!(sc["source"].as_str().is_some_and(|s| s.starts_with("same-floor plan")), "{n}: {sc}");
    }
}

#[test]
fn a_do_not_scale_plan_with_no_note_takes_its_floors_scale() {
    let _g = serial();
    real_pdf_reader();
    // The lighting and power plans lettered in strokes that don't read (no scale lettering at
    // all): DO NOT SCALE sheets left at 1x were never fitted, asked or reported as paper size.
    let none = Change { all: true, ..AS_PRINTED };
    let dir = set("do_not_scale", &[("A-101", AS_PRINTED), ("E-101", none), ("E-201", none), ("M-101", AS_PRINTED)]);
    let state = job(&dir);
    for n in ["E-101", "E-201"] {
        let sc = scale_of(&state, n);
        assert_eq!(sc["scale"].as_f64(), Some(PLANS), "{n}: {sc}");
        let src = sc["source"].as_str().unwrap_or("");
        assert!(src.starts_with("same-floor plan") && src.contains("was paper size"), "{n}: {src}");
    }
    // Each plan's scale and where it came from, in the job's report.
    let plans = state["guided_job"]["sheet_scales"].as_array().map_or(0, Vec::len);
    assert_eq!(plans, 4);
}

#[test]
fn a_plan_with_no_evidence_but_its_peers_takes_their_scale_and_is_asked() {
    let _g = serial();
    real_pdf_reader();
    // The first floor's lighting plan with no scale lettering and no other plan of its floor; the
    // lighting plans of the floors above print theirs.
    let none = Change { all: true, ..AS_PRINTED };
    let dir = set("peers", &[("E-101", none), ("E-102", AS_PRINTED), ("E-103", AS_PRINTED)]);
    let state = job(&dir);
    let sc = scale_of(&state, "E-101");
    assert_eq!(sc["scale"].as_f64(), Some(PLANS), "{sc}");
    let src = sc["source"].as_str().unwrap_or("");
    assert!(src.starts_with("peer plans") && src.contains("E-102") && src.contains("E-103"), "{src}");
    // Derived, said, and asked with the alternatives; the default keeps it.
    let qs = questions(&state);
    let q = qs.iter().find(|q| q["question"].as_str().is_some_and(|t| t.starts_with("Scale in doubt for plan sheet E-101")));
    let q = q.unwrap_or_else(|| panic!("asked: {qs:?}"));
    assert_eq!(q["default"], json!("Keep the scale the job took"), "{q}");
    assert!(q["choices"].as_array().is_some_and(|c| c.iter().any(|c| c == "3/16\" = 1'-0\"")), "{q}");
    let findings = state["guided_job"]["findings"].to_string();
    assert!(findings.contains("scaled from the plans of their trade and paper size"), "{findings}");
}

#[test]
fn a_plan_note_its_floors_plan_contradicts_is_overruled_and_asked() {
    let _g = serial();
    real_pdf_reader();
    // The mechanical plan's own note gone and three bare 3" notes beside it, under no title:
    // nothing tells they are details', so the sheet reads at 4x. Its peers print 3/16" and its
    // linework lies on the floor plan's at 3/16": that is its scale, and the note is asked about.
    let bare = Change { plan_note: true, ..with_details(Details::Bare, 3, "3\"") };
    let dir = set("contradicted", &[("A-101", AS_PRINTED), ("M-101", bare), ("E-101", AS_PRINTED), ("P-101", AS_PRINTED)]);
    let state = job(&dir);
    let sc = scale_of(&state, "M-101");
    assert_eq!(sc["scale"].as_f64(), Some(PLANS), "{sc}");
    let src = sc["source"].as_str().unwrap_or("");
    assert!(src.starts_with("same-floor plan") && src.contains("from its note"), "{src}");
    let qs = questions(&state);
    let q = qs.iter().find(|q| q["question"].as_str().is_some_and(|t| t.starts_with("Scale in doubt for plan sheet M-101")));
    let q = q.unwrap_or_else(|| panic!("asked: {qs:?}"));
    let choices: Vec<&str> = q["choices"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    assert!(choices.contains(&"3/16\" = 1'-0\"") && choices.contains(&"3\" = 1'-0\""), "both candidates: {choices:?}");
}
