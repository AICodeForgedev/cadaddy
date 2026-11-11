//! The sheet-index checkpoint (Q6517): right after classify the job writes its reading of every
//! sheet as one packet (pictures, views, scales, tables) with the flags it raises itself and
//! stops for it; one answer corrects many sheets and the build after it reads them as corrected.
//! A made-up set with a plan typed as notes, a plan whose scale note is misprinted and a plan
//! sheet that also prints a detail at another scale.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, EntityKind, HAlign, Space, Text, VAlign};
use cadaddy_engine::geom::Vec3;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

const SEED: u64 = 42;
/// The plan typed as no plan, the plan whose note is misprinted, the sheet that also prints a
/// detail.
const NOTES: &str = "A-102";
const MISSCALED: &str = "M-102";
const MIXED: &str = "A-101";
/// The detail's rectangle on the mixed sheet, sheet inches (lower-left corner, upper-right).
const DETAIL: [f64; 4] = [13.0, 2.4, 21.0, 4.4];

fn text(d: &mut cadaddy_engine::doc::Drawing, at: [f64; 2], h: f64, s: &str) {
    let t = Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    let c = Common { layer: "TEXT".into(), ..Common::default() };
    d.add(&Space::Model, c, EntityKind::Text(t)).expect("text");
}

fn line(d: &mut cadaddy_engine::doc::Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
    let c = Common { layer: layer.into(), ..Common::default() };
    d.add(&Space::Model, c, EntityKind::Line(cadaddy_engine::doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }))
        .expect("line");
}

/// The made-up hotel's first two levels (architectural, mechanical and plumbing plans) as one
/// merged PDF, with the three misreadings planted. Returns the folder and the hotel's truth.
fn planted_set(tag: &str) -> (std::path::PathBuf, cadaddy_mepfix::truth::Truth) {
    let (mut t, st) = cadaddy_mepfix::hotel_planned(SEED, HotelSize::Small);
    let kinds: Vec<(String, cadaddy_mepfix::layout::Kind)> =
        cadaddy_mepfix::layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let keep = ["A-101", "A-102", "M-101", "M-102", "P-101", "P-102"];
    if let Some(sh) = t.sheets.iter_mut().find(|s| s.number == NOTES) {
        sh.title = "LEVEL 2 FLOOR PLAN NOTES".into();
    }
    let mut pages = Vec::new();
    for sh in t.sheets.iter().filter(|s| keep.contains(&s.number.as_str())) {
        let Some((_, kind)) = kinds.iter().find(|(n, _)| *n == sh.number) else { continue };
        let (mut d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, sh, kind);
        let m = |p: [f64; 2]| cadaddy_mepfix::layout::paper_to_model(sh, p);
        let per_in = 1.0 / sh.paper_from_model.scale;
        if sh.number == MISSCALED {
            // The scale notes misprinted as a detail's: the reader takes 3" = 1'-0".
            let notes: Vec<_> =
                d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Text(x) if x.value.starts_with("SCALE:"))).map(|e| e.handle).collect();
            for h in notes {
                d.model.modify(h, |e| {
                    if let EntityKind::Text(x) = &mut e.kind {
                        x.value = "SCALE: 3\" = 1'-0\"".into();
                    }
                });
            }
        }
        if sh.number == MIXED {
            // A boxed wall detail at another scale, with its own title and note, beside the plan.
            let [x0, y0, x1, y1] = DETAIL;
            for (a, b) in [([x0, y0], [x1, y0]), ([x1, y0], [x1, y1]), ([x1, y1], [x0, y1]), ([x0, y1], [x0, y0])] {
                line(&mut d, "A-WALL", m(a), m(b));
            }
            for k in [0.3, 0.42] {
                line(&mut d, "A-WALL", m([x0 + 0.5, y0 + 1.0 + k]), m([x1 - 0.5, y0 + 1.0 + k]));
            }
            text(&mut d, m([x0, y0 - 0.45]), 0.18 * per_in, "TYPICAL WALL DETAIL");
            text(&mut d, m([x0, y0 - 0.75]), 0.1 * per_in, "SCALE: 1 1/2\" = 1'-0\"");
        }
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("plot");
        pages.push((pdf, format!("{} {}", sh.number, sh.title), String::new()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_sheetcheck_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    (dir, t)
}

fn pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

/// The rooms built on `level`, by number.
fn room_numbers(s: &mut Session, level: &str) -> Vec<String> {
    let r = s.execute("rooms.list", &json!({"level": level, "outline": false})).expect("rooms.list");
    r["rooms"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["number"].as_str()).filter(|n| !n.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn flags_of(v: &Value) -> Vec<(String, String, String)> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["kind"].as_str().unwrap_or("").to_string(),
                        f["sheet"].as_str().unwrap_or("").to_string(),
                        f["level"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The three corrections an agent reading the pictures gives: the notes-typed sheet is level 2's
/// plan, the misprinted sheet is at its peers' 3/16" scale, the mixed sheet's plan is its left
/// part (the detail below it left out).
fn corrections() -> Value {
    json!({
        NOTES: {"type": "plan", "level": "Level 2", "title": "LEVEL 2 FLOOR PLAN"},
        MISSCALED: {"scale": "3/16\" = 1'-0\""},
        MIXED: {"plan_view": PLAN_VIEW},
        // A schedule the readers didn't read, given as the reader's rows.
        "P-101": {"tables": {"panel_schedules": [{"panel": "PX1", "circuits": [{"number": "1", "description": "WATER HEATER", "breaker": "30", "poles": 2}]}]}},
    })
}

/// The mixed sheet's plan (grids and all), sheet inches: the detail lies under it.
const PLAN_VIEW: [f64; 4] = [4.8, 4.8, 25.0, 18.0];

#[test]
fn the_flags_name_each_misreading_and_one_answer_fixes_them_for_the_build() {
    let _g = serial();
    pdf_reader();
    let (dir, t) = planted_set("fix");
    let ckpt = dir.join("job.cdy");
    let start = json!({"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": ckpt.display().to_string(), "specs": false});
    let mut s = Session::new();
    let st = s.execute("job.start", &start).expect("job.start");
    // The job stops at the checkpoint, right after classify, before levels.
    assert_eq!(st["current_stage"], "classify", "{st}");
    let q = &st["question"];
    assert!(q["question"].as_str().is_some_and(|t| t.starts_with("Check the sheet index")), "{q}");
    let qid = q["id"].as_str().expect("id").to_string();
    let ev = &q["evidence"]["values"];
    // The packet as JSON with a picture of each page and a contact sheet.
    let packet: Value = serde_json::from_str(&std::fs::read_to_string(ev["packet"].as_str().expect("packet path")).expect("packet")).expect("json");
    assert!(std::path::Path::new(ev["contact_sheet"].as_str().expect("contact sheet")).is_file(), "{ev}");
    let sheets = packet["sheets"].as_array().cloned().unwrap_or_default();
    assert_eq!(sheets.len(), 6, "{packet}");
    for sh in &sheets {
        let pic = sh["pictures"][0].as_str().unwrap_or("");
        assert!(pic.ends_with(".png") && std::path::Path::new(pic).is_file(), "{sh}");
        assert!(sh["scale"]["import"].as_f64().is_some() && sh["scale"]["source"].is_string() && sh["paper"].is_array(), "{sh}");
    }
    let mixed = sheets.iter().find(|x| x["number"] == MIXED).cloned().unwrap_or_default();
    let detail = mixed["views"].as_array().and_then(|v| v.iter().find(|x| x["title"] == "TYPICAL WALL DETAIL")).cloned().unwrap_or_default();
    let r: Vec<f64> = detail["rect"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    assert!(r.len() == 4 && (r[0] - DETAIL[0]).abs() < 0.5 && (r[3] - DETAIL[3]).abs() < 0.5, "the detail's rectangle in sheet inches: {detail}");
    assert_eq!(detail["scale"].as_f64(), Some(8.0), "its own note: {detail}");
    // The flags name all three misreadings (and the level they leave without a floor plan).
    let flags = flags_of(&packet["flags"]);
    for want in [("plan_title_not_plan", NOTES, ""), ("plan_off_scale", MISSCALED, ""), ("mixed_sheet", MIXED, ""), ("no_floor_plan", "", "2")] {
        assert!(flags.iter().any(|f| (f.0.as_str(), f.1.as_str(), f.2.as_str()) == want), "{want:?} in {flags:?}");
    }
    assert_eq!(flags.len(), 4, "nothing else is flagged: {flags:?}");

    // Wrong answers are messages: the job is as it was.
    for bad in [
        json!({"Z-999": {"type": "plan"}}),
        json!({MIXED: {"plan_view": [20.0, 4.0, 10.0, 9.0]}}),
        json!({MISSCALED: {"scale": 0}}),
        json!({MISSCALED: {"scale": -64}}),
    ] {
        let e = s.execute("job.sheets.set", &bad).expect_err("refused");
        assert!(!e.to_string().is_empty());
        let e = s.execute("job.answer", &json!({"id": qid, "answer": bad})).expect_err("refused");
        assert!(e.to_string().contains(&qid), "{e}");
    }
    assert_eq!(s.execute("job.status", &json!({})).expect("status")["current_stage"], "classify");

    // One call corrects all three: learned as rules, shown in job.explain, the job goes on.
    let r = s.execute("job.sheets.set", &corrections()).expect("job.sheets.set");
    assert!(r["effect"].as_str().is_some_and(|e| e.contains(NOTES) && e.contains(MISSCALED) && e.contains(MIXED)), "{r}");
    let rules = s.execute("job.rules", &json!({})).expect("rules");
    let keys: Vec<&str> = rules["rules"].as_array().map(|a| a.iter().filter_map(|x| x["key"].as_str()).collect()).unwrap_or_default();
    for n in [NOTES, MISSCALED, MIXED] {
        assert!(keys.contains(&format!("sheet:{n}").as_str()), "{keys:?}");
    }
    let explain = s.execute("job.explain", &json!({"stage": "classify"})).expect("explain");
    let fixed: Vec<&str> = explain["result"]["sheet_index"]["corrections"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["sheet"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(fixed.len(), 4, "{explain}");
    assert!(flags_of(&explain["result"]["sheet_index"]["flags"]).is_empty(), "the corrected reading raises no flag: {explain}");
    assert_ne!(s.execute("job.status", &json!({})).expect("status")["current_stage"], "classify");

    // The corrections survive a resume that reads the prints again (classify included).
    let mut again = Session::new();
    let mut p = start.clone();
    p["refresh"] = json!("classify");
    again.execute("job.start", &p).expect("resume");
    let state = job_state(&again);
    let index = &state["guided_job"]["index"];
    let a102 = index["sheets"].as_array().and_then(|a| a.iter().find(|x| x["number"] == NOTES)).cloned().unwrap_or_default();
    assert_eq!((a102["type"].as_str(), a102["level"].as_str()), (Some("plan"), Some("2")), "{a102}");
    assert_ne!(state["guided_job"]["stages"][1]["status"], "paused", "the checkpoint keeps its answer");
    drop(again);

    // The build after it: level 2 built from its plan, the misprinted sheet at its scale, the
    // mixed sheet's plan alone.
    let done = s.execute("job.set_mode", &json!({"auto": true})).expect("auto");
    assert_eq!(done["done"], true, "{done}");
    let state = job_state(&s);
    let g = &state["guided_job"];
    let truth_scale = t
        .sheets
        .iter()
        .find(|x| x.number == MISSCALED)
        .and_then(|x| x.scale_text.clone())
        .and_then(|x| cadaddy_io::pdf_read::parse_scale(&x))
        .expect("truth scale");
    let sc = g["sheet_scales"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == MISSCALED)).cloned().unwrap_or_default();
    assert_eq!((sc["scale"].as_f64(), sc["source"].as_str()), (Some(truth_scale), Some("option")), "{sc}");
    let reg = g["sheet_regions"].as_array().and_then(|a| a.iter().find(|x| x["number"] == MIXED)).cloned().unwrap_or_default();
    let k = g["sheet_scales"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == MIXED)).and_then(|x| x["scale"].as_f64()).unwrap_or(0.0);
    let x0 = reg["rect"][0].as_f64().unwrap_or(f64::NAN);
    assert!((reg["view"]["rect"][0].as_f64().unwrap_or(0.0) - (x0 + PLAN_VIEW[0] * k)).abs() < 1e-6, "{reg}");
    for (level, code) in [("Level 1", "1"), ("Level 2", "2")] {
        let truth: Vec<String> = t.building.rooms.iter().filter(|r| r.level == code).map(|r| r.number.clone()).collect();
        let built = room_numbers(&mut s, level);
        let right = built.iter().filter(|n| truth.contains(n)).count();
        assert!(built.iter().all(|n| truth.contains(n)), "{level}: every numbered room is one of the plan's: {built:?} / {truth:?}");
        assert!(right * 3 >= truth.len() * 2, "{level}: most of the plan's rooms are built: {built:?} / {truth:?}");
        eprintln!("corrected: {level} rooms {right} of the plan's {} ({} numbered built)", truth.len(), built.len());
    }
    // The answered panel schedule is the build's, its source the answer.
    let spec = s.execute("bspec.get", &json!({})).expect("bspec.get");
    let px = spec["spec"]["mep"]["panel_schedules"].as_array().and_then(|a| a.iter().find(|p| p["panel"] == "PX1")).cloned().unwrap_or_default();
    assert_eq!(px["source"], "P-101 (answer, from the sheet image)", "{}", spec["spec"]["mep"]["panel_schedules"]);
    assert_eq!(px["circuits"][0]["description"], "WATER HEATER");
    let rep = s.execute("job.report", &json!({})).expect("report");
    assert_eq!(rep["sheet_index"]["checked"], true, "{}", rep["sheet_index"]);
    assert!(rep["sheet_index"]["unchecked"].as_array().is_some_and(Vec::is_empty), "{}", rep["sheet_index"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_runs_through_on_the_unchecked_reading_and_says_so() {
    let _g = serial();
    pdf_reader();
    let (dir, _t) = planted_set("auto");
    let mut s = Session::new();
    let st = s
        .execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false, "specs": false}))
        .expect("job.start");
    assert_eq!(st["done"], true, "{st}");
    let rep = s.execute("job.report", &json!({})).expect("report");
    let si = &rep["sheet_index"];
    assert_eq!(si["checked"], false, "{si}");
    let unchecked: Vec<&str> = si["unchecked"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    assert_eq!(unchecked.len(), 4, "{si}");
    assert!(unchecked.iter().all(|u| u.starts_with("built on an unchecked reading")), "{unchecked:?}");
    assert!(
        unchecked.iter().any(|u| u.contains(MISSCALED)) && unchecked.iter().any(|u| u.contains(MIXED)) && unchecked.iter().any(|u| u.contains(NOTES)),
        "{unchecked:?}"
    );
    assert_eq!(si["levels_not_built"], json!(["2"]), "{si}");
    // A finished job of no checkpoint leaves no packet in the temporary folder.
    let packet = std::path::PathBuf::from(si["packet"].as_str().expect("packet path"));
    assert!(packet.parent().is_some_and(|d| !d.exists()), "{}", packet.display());
    let state = job_state(&s);
    let sc = state["guided_job"]["sheet_scales"].as_array().and_then(|a| a.iter().find(|x| x["sheet"] == MISSCALED)).cloned().unwrap_or_default();
    eprintln!(
        "unchecked: {MISSCALED} at {} ({}); level 1 rooms {}; level 2 rooms {}",
        sc["scale"],
        sc["source"],
        room_numbers(&mut s, "Level 1").len(),
        room_numbers(&mut s, "Level 2").len()
    );
    assert!(room_numbers(&mut s, "Level 2").is_empty(), "no floor plan, no level 2 rooms");
    // Told to go on (sheet_check: false): it doesn't stop, and says the same.
    let mut s = Session::new();
    let st = s
        .execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "sheet_check": false, "sheet_pictures": false, "checkpoint": false, "specs": false}))
        .expect("job.start");
    assert_eq!(st["current_stage"], "levels", "{st}");
    let rep = s.execute("job.report", &json!({})).expect("report");
    assert_eq!(
        (rep["sheet_index"]["checked"].as_bool(), rep["sheet_index"]["answered_by"].as_str()),
        (Some(false), Some("go_on")),
        "{}",
        rep["sheet_index"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
