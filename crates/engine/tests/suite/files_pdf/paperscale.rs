//! A plan whose scale isn't read (Q6360). The made-up hotel's level 1 floor plan with its scale
//! lettering taken off (no note, no scale bar), beside the level's lighting plan that prints its
//! scale: the floor plan takes the lighting plan's scale (their outlines agree at one ratio) and
//! the level comes out at full size. Alone, it is always asked for its scale (auto mode too)
//! and never built at the sheet's own size; the answer builds it at that scale.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, EntityKind, Space, entity_bounds};
use cadaddy_engine::geom::Bounds2;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

const SEED: u64 = 42;
pub(crate) const FLOOR: &str = "A-101";
pub(crate) const LIGHTING: &str = "E-101";

pub(crate) fn pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// Lettering that gives a sheet its scale: a scale note, a scale bar's feet, a dimension string.
fn scale_lettering(t: &str) -> bool {
    let u = t.to_ascii_uppercase();
    u.contains("SCALE") || t.contains('\'') || t.contains('"')
}

/// The made-up hotel's `keep` sheets as one merged PDF, those in `strip` with every lettering
/// that gives a scale taken off. The folder, and the floor plan's scale (drawing units a sheet
/// inch).
fn set(tag: &str, keep: &[&str], strip: &[&str]) -> (std::path::PathBuf, f64) {
    set_lettering(tag, keep, strip, |e| matches!(&e.kind, EntityKind::Text(x) if scale_lettering(&x.value)))
}

/// [`set`], taking off the objects `gone` picks on the `strip` sheets.
pub(crate) fn set_lettering(tag: &str, keep: &[&str], strip: &[&str], gone: fn(&Entity) -> bool) -> (std::path::PathBuf, f64) {
    let (t, st) = cadaddy_mepfix::hotel_planned(SEED, HotelSize::Small);
    let kinds: Vec<(String, cadaddy_mepfix::layout::Kind)> =
        cadaddy_mepfix::layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let mut pages = Vec::new();
    let mut floor_scale = 0.0;
    for sh in t.sheets.iter().filter(|s| keep.contains(&s.number.as_str())) {
        let Some((_, kind)) = kinds.iter().find(|(n, _)| *n == sh.number) else { continue };
        let (mut d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, sh, kind);
        if sh.number == FLOOR {
            floor_scale = 1.0 / sh.paper_from_model.scale;
        }
        if strip.contains(&sh.number.as_str()) {
            let gone: Vec<_> = d.model.iter().filter(|e| gone(e)).map(|e| e.handle).collect();
            for h in gone {
                d.model.remove(h);
            }
        }
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("plot");
        pages.push((pdf, format!("{} {}", sh.number, sh.title), String::new()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_paperscale_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    (dir, floor_scale)
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

/// How the job read sheet `n`'s scale.
fn scale_of(s: &Session, n: &str) -> Value {
    job_state(s)["guided_job"]["sheet_scales"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == n)).cloned().unwrap_or_default()
}

/// The plan area (sq ft) the built walls span.
fn walls_area(s: &Session) -> f64 {
    let d = s.doc().expect("doc");
    let mut b = Bounds2::EMPTY;
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")) {
        b = b.union(&entity_bounds(d, e, 0));
    }
    if b.is_empty() { 0.0 } else { b.width() * b.height() / 144.0 }
}

fn start(dir: &std::path::Path) -> Value {
    json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false, "specs": false, "sheet_pictures": false})
}

/// The open questions of the job, as (id, text, always asked).
fn open_questions(s: &mut Session) -> Vec<(String, String, bool)> {
    let st = s.execute("job.status", &json!({})).expect("status");
    let mut out = Vec::new();
    if let Some(q) = st.get("question").filter(|q| q.is_object()) {
        out.push((
            q["id"].as_str().unwrap_or("").to_string(),
            q["question"].as_str().unwrap_or("").to_string(),
            q["always_asked"].as_bool().unwrap_or(false),
        ));
    }
    out
}

/// Answer the job's other open questions (a stage of the made-up set that found nothing it
/// always asks about) with their defaults, to the end of the build; never one of Q6360's.
fn finish(s: &mut Session) -> Value {
    let mut st = s.execute("job.status", &json!({})).expect("status");
    for _ in 0..12 {
        if st["done"] == true {
            break;
        }
        let q = st["question"].clone();
        let text = q["question"].as_str().unwrap_or("");
        assert!(!text.starts_with("No scale for plan sheet") && !text.starts_with("Too small to be a building"), "{q}");
        let Some(id) = q["id"].as_str() else { break };
        let answer =
            q["default"].as_str().map(str::to_string).or_else(|| q["choices"][0].as_str().map(str::to_string)).unwrap_or_else(|| "Correct".into());
        s.execute("job.answer", &json!({"id": id, "answer": answer})).expect("answer");
        st = s.execute("job.status", &json!({})).expect("status");
    }
    st
}

#[test]
fn a_plan_with_no_scale_takes_it_from_its_floors_trade_plan() {
    let _g = serial();
    pdf_reader();
    // As printed: the floor plan reads its own note.
    let (dir, k) = set("printed", &[FLOOR, LIGHTING], &[]);
    let mut s = Session::new();
    s.execute("job.start", &start(&dir)).expect("job.start");
    let st = finish(&mut s);
    let printed = scale_of(&s, FLOOR);
    assert_eq!(printed["scale"].as_f64(), Some(k), "{printed} {st}");
    let full = walls_area(&s);
    assert!(full > 1000.0, "the floor as printed: {full} sq ft {st}");
    let _ = std::fs::remove_dir_all(&dir);

    // Its scale lettering gone: it takes the lighting plan's scale, and the level is full size.
    let (dir, _) = set("stripped", &[FLOOR, LIGHTING], &[FLOOR]);
    let mut s = Session::new();
    s.execute("job.start", &start(&dir)).expect("job.start");
    let st = finish(&mut s);
    let sc = scale_of(&s, FLOOR);
    assert_eq!(sc["scale"].as_f64(), Some(k), "{sc} {st}");
    let src = sc["source"].as_str().unwrap_or("");
    assert!(src.starts_with("same-floor plan E-101") && src.contains("was paper size"), "{src}");
    let area = walls_area(&s);
    assert!((area / full - 1.0).abs() <= 0.01, "full size: {area} sq ft against {full} as printed {st}");
    // Said in the findings, as derived.
    let findings = job_state(&s)["guided_job"]["findings"].clone();
    let derived = findings
        .as_array()
        .and_then(|a| a.iter().find(|f| f["description"].as_str().is_some_and(|d| d.contains("scaled from a plan of the same floor"))));
    assert!(derived.is_some_and(|f| f["description"].as_str().is_some_and(|d| d.contains(FLOOR) && d.contains(LIGHTING))), "{findings}");
    // No longer at paper size, and no question about its scale.
    assert!(!findings.to_string().contains("imported at paper size"), "{findings}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plan_alone_with_no_scale_is_always_asked() {
    let _g = serial();
    pdf_reader();
    let (dir, k) = set("alone", &[FLOOR], &[FLOOR]);
    let mut s = Session::new();
    let st = s.execute("job.start", &start(&dir)).expect("job.start");
    // Auto mode stops at it: never taken by default, nothing built at the sheet's size.
    assert_ne!(st["done"], true, "{st}");
    let open = open_questions(&mut s);
    let q = open.iter().find(|q| q.1.starts_with(&format!("No scale for plan sheet {FLOOR}"))).cloned();
    let Some((qid, _, always)) = q else { panic!("asked for its scale: {open:?} {st}") };
    assert!(always, "always asked");
    assert_eq!(walls_area(&s), 0.0, "not built at paper size");
    // The answer builds it at that scale.
    // (drawing units a sheet inch, as the question takes it)
    s.execute("job.answer", &json!({"id": qid, "answer": format!("{k}")})).expect("answer");
    finish(&mut s);
    let sc = scale_of(&s, FLOOR);
    assert_eq!(sc["scale"].as_f64(), Some(k), "{sc}");
    assert!(walls_area(&s) > 1000.0, "built at the scale given");
    let _ = std::fs::remove_dir_all(&dir);
}
