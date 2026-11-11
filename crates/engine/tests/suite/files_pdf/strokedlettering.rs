//! Q6516: a set whose lettering is plotted as strokes only (vector text with no searchable text
//! under it): the sheet titles, the schedules' rows and the plans' tags are read off the strokes,
//! each sheet says how its lettering read, and a schedule sheet that can't be read is asked for
//! with its picture and takes its tables as an answer.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, LwPolyline, Space};
use cadaddy_engine::geom::{PolyVertex, Vec2};
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

/// The made-up set's sheets the tests print: a cover with the title block, a floor plan, the
/// lighting and power plans and the electrical schedules (fixture and panel schedules).
const SET: &[&str] = &["G-001", "A-101", "E-101", "E-201", "E-601"];

/// Every lettering of `d` on `layer` redrawn as a scrawl no stroke font draws (a plot font the
/// OCR can't read), one per character.
fn scrawl(d: &mut Drawing, layer: &str) {
    let texts: Vec<(cadaddy_engine::doc::Handle, Vec2, f64, usize)> = d
        .model
        .iter()
        .filter(|e| e.common.layer.as_str() == layer)
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some((e.handle, t.insert.xy(), t.height, t.value.chars().count())),
            _ => None,
        })
        .collect();
    for (h, at, height, n) in texts {
        d.model.remove(h);
        for i in 0..n {
            let x = at.x + i as f64 * height * 0.9;
            let vertices = [(0.0, 0.0), (0.7, 0.9), (0.1, 0.6), (0.8, 0.2), (0.3, 1.0), (0.6, 0.0)]
                .iter()
                .map(|(u, v)| PolyVertex { p: Vec2::new(x + u * height * 0.7, at.y + v * height), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
                .collect();
            let common = Common { layer: layer.into(), ..Common::default() };
            let line = LwPolyline { vertices, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false };
            let _ = d.add(&Space::Model, common, EntityKind::LwPolyline(line));
        }
    }
}

/// `SET` lettered in strokes only (no searchable text, no bookmarks, no document title: a sheet's
/// number and title are only on its page) as one merged PDF in a new folder. `scrawled`: the
/// sheet whose content lettering (not its title block) is drawn in a font the OCR can't read.
fn job_folder(tag: &str, scrawled: Option<&str>) -> (cadaddy_mepfix::truth::Truth, std::path::PathBuf) {
    let (mut t, st, kinds) = cadaddy_mepfix::office_planned(7);
    t.sheets.retain(|s| SET.contains(&s.number.as_str()));
    let mut pages: Vec<(Vec<u8>, String, String)> = Vec::new();
    for sh in t.sheets.clone() {
        let Some((_, kind)) = kinds.iter().find(|k| k.0 == sh.number) else { continue };
        let (mut drawing, _, _) = cadaddy_mepfix::draw::draw(&t, &st, &sh, kind);
        if scrawled == Some(sh.number.as_str()) {
            scrawl(&mut drawing, "TEXT");
        }
        let opts = cadaddy_io::pdf::PdfOptions { searchable: false, title: String::new(), ..cadaddy_mepfix::pdf_options(&sh, &st) };
        let pdf = cadaddy_io::pdf::pdf(&drawing, &Space::Model, &opts).expect("plot");
        pages.push((pdf, String::new(), String::new()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_stroked_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    (t, dir)
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

fn stage<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == name)).unwrap_or_else(|| panic!("stage {name}"))
}

/// Start the job and take each question's default, but a table question's, which `table`
/// answers (and the question is kept). Returns the job's last status and the table questions.
fn run_job(s: &mut Session, dir: &std::path::Path, table: &dyn Fn(&Value) -> Value) -> (Value, Vec<Value>) {
    let mut st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    let mut asked = Vec::new();
    for _ in 0..12 {
        if st["done"] == json!(true) {
            break;
        }
        let next = s.execute("job.next", &json!({})).expect("job.next");
        let q = next["question"].clone();
        let Some(id) = q["id"].as_str().map(str::to_string) else { break };
        let answer = if q["question"].as_str().is_some_and(|t| t.starts_with("Tables to read: ")) {
            asked.push(q.clone());
            table(&q)
        } else {
            q["default"].clone()
        };
        st = s.execute("job.answer", &json!({"id": id, "answer": answer})).expect("job.answer");
        if st["done"] != json!(true) {
            st = s.execute("job.status", &json!({})).expect("job.status");
        }
    }
    (st, asked)
}

/// The panels the made-up set schedules, their rows, and its luminaire types.
fn truth_tables(t: &cadaddy_mepfix::truth::Truth) -> (usize, usize, std::collections::BTreeSet<String>) {
    let panels: Vec<&cadaddy_mepfix::truth::ElecPanel> = t.elec_panels.iter().filter(|p| p.kind == "panel" && !p.schedule.is_empty()).collect();
    let rows = panels.iter().map(|p| p.schedule.len()).sum();
    let types = t
        .elec_devices
        .iter()
        .filter(|d| d.kind == "light_fixture" && !t.elec_spec.unscheduled.contains(&d.type_tag))
        .map(|d| d.type_tag.clone())
        .collect();
    (panels.len(), rows, types)
}

#[test]
fn a_set_lettered_in_strokes_reads_its_titles_schedules_and_tags() {
    let _g = serial();
    real_pdf_reader();
    let (truth, dir) = job_folder("read", None);
    let mut s = Session::new();
    let (st, asked) = run_job(&mut s, &dir, &|_| json!("Leave as read"));
    assert_eq!(st["done"], json!(true), "{st}");
    let job = job_state(&s);
    let classify = stage(&job, "classify")["result"].clone();
    let spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    let read = spec["mep"]["panel_schedules"].as_array().cloned().unwrap_or_default();
    // What was read against what the set prints: titles, panels, circuits (whole rows: number,
    // description, breaker and poles), fixture types, the luminaires by their marks.
    let titles = truth
        .sheets
        .iter()
        .filter(|sh| {
            classify["sheets"].as_array().is_some_and(|a| a.iter().any(|e| e["number"] == sh.number.as_str() && e["title"] == sh.title.as_str()))
        })
        .count();
    let (panels, rows, types) = truth_tables(&truth);
    let circuits: usize = read.iter().map(|p| p["circuits"].as_array().map_or(0, Vec::len)).sum();
    let mut rows_right = 0usize;
    for p in truth.elec_panels.iter().filter(|p| p.kind == "panel") {
        let Some(got) = read.iter().find(|r| r["panel"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(&p.tag))) else { continue };
        for row in &p.schedule {
            let c = got["circuits"].as_array().and_then(|a| a.iter().find(|c| c["number"] == row.circuit.as_str()));
            let right = c.is_some_and(|c| {
                c["description"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(&row.description))
                    && c["breaker"].as_str().is_some_and(|b| b.trim_end_matches('A') == row.breaker_amps)
                    && c["poles"].as_u64().map(|n| n.to_string()) == Some(row.poles.clone())
            });
            if !right {
                eprintln!("not read whole: panel {} row {row:?}: {c:?}", p.tag);
            }
            rows_right += usize::from(right);
        }
    }
    let fixtures: std::collections::BTreeSet<String> =
        spec["mep"]["fixture_schedule"].as_array().into_iter().flatten().filter_map(|f| f["tag"].as_str().map(str::to_string)).collect();
    let devices = stage(&job, "devices")["result"]["devices"]["by_type"].clone();
    eprintln!(
        "titles {titles} of {}; panels {} of {panels}; circuits {circuits}, rows right {rows_right} of {rows}; fixture types {:?} of {types:?}; devices by type {devices}",
        truth.sheets.len(),
        read.len(),
        fixtures
    );
    // The sheet index: every sheet's number and title off its title block's strokes.
    assert_eq!(titles, truth.sheets.len(), "{}", classify["sheets"]);
    // Each sheet says how its lettering read: all strokes, read by the OCR alone, and sure.
    let lettering = classify["lettering"]["sheets"].as_array().cloned().unwrap_or_default();
    let schedules = lettering.iter().find(|l| l["sheet"] == "E-601").unwrap_or_else(|| panic!("{lettering:?}"));
    assert!(schedules["ocr_share"].as_f64().is_some_and(|v| v > 0.95), "{schedules}");
    assert!(schedules["words"].as_u64().is_some_and(|n| n > 300), "{schedules}");
    assert!(schedules["cells"].as_u64().is_some_and(|n| n > 300) && schedules["cells_unsure"] == 0, "{schedules}");
    assert!(schedules["confidence"].as_f64().is_some_and(|c| c >= 0.9) && schedules["unsure"] == false, "{schedules}");
    assert_eq!(classify["lettering"]["in_strokes"].as_u64(), Some(truth.sheets.len() as u64), "{}", classify["lettering"]);
    // The schedules' rows, cell by cell: every panel, its circuits whole (number, description,
    // breaker, poles), every fixture type.
    assert_eq!(read.len(), panels, "{read:#?}");
    assert!(rows_right * 20 >= rows * 19, "rows right {rows_right} of {rows}: {read:#?}");
    assert_eq!(fixtures, types);
    // The plans' marks by their symbols: the luminaires of every scheduled type.
    for tag in &types {
        assert!(devices[tag.as_str()].as_u64().is_some_and(|n| n > 0), "{tag}: {devices}");
    }
    // Read sure, nothing is asked.
    assert!(asked.is_empty(), "{asked:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_schedule_sheet_lettered_in_an_unreadable_font_is_asked_for_and_answered() {
    let _g = serial();
    real_pdf_reader();
    let (_truth, dir) = job_folder("ask", Some("E-601"));
    let mut s = Session::new();
    let answer = json!({
        "panels": [{"panel": "L1", "circuits": [
            {"number": "1", "description": "LIGHTING", "breaker": "20", "poles": 1},
            {"number": "3", "description": "LIGHTING", "breaker": "20", "poles": 1}]}],
        "fixtures": [{"tag": "A", "description": "2X4 RECESSED TROFFER", "lamp": "LED 40W", "installation": "RECESSED"}],
    });
    let (st, asked) = run_job(&mut s, &dir, &|_| answer.clone());
    assert_eq!(st["done"], json!(true), "{st}");
    // One blocking question for the sheet, with its picture: its tables are what the job needs.
    assert_eq!(asked.len(), 1, "{asked:#?}");
    let q = &asked[0];
    assert!(q["question"].as_str().is_some_and(|t| t.contains("E-601 ELECTRICAL SCHEDULES could not be read")), "{q}");
    assert_eq!(q["always_asked"], true, "{q}");
    assert_eq!(q["sheet"], "E-601", "{q}");
    let crop = q["crop"].as_str().unwrap_or_else(|| panic!("a picture: {q}"));
    assert!(std::fs::metadata(crop).is_ok_and(|m| m.len() > 1000), "{crop}");
    assert!(q["evidence"]["values"]["lettering"]["ocr_share"].as_f64().is_some_and(|v| v > 0.9), "{q}");
    // The answer's tables are the job's, stored with the answer as their source.
    let spec = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    let panels = spec["mep"]["panel_schedules"].as_array().cloned().unwrap_or_default();
    let l1 = panels.iter().find(|p| p["panel"] == "L1").unwrap_or_else(|| panic!("{panels:#?}"));
    assert_eq!((l1["source"].as_str(), l1["header"]["source"].as_str()), (Some("E-601"), Some("answer, from the sheet image")), "{l1}");
    assert_eq!(l1["circuits"].as_array().map(Vec::len), Some(2), "{l1}");
    let fixtures = spec["mep"]["fixture_schedule"].as_array().cloned().unwrap_or_default();
    let a = fixtures.iter().find(|f| f["tag"] == "A").unwrap_or_else(|| panic!("{fixtures:#?}"));
    assert_eq!(a["source"], "E-601 (answer, from the sheet image)", "{a}");
    // The answer is a learned rule: it shows in the job and is kept for a resume.
    let job = job_state(&s);
    assert!(job["guided_job"]["rules"].as_array().is_some_and(|r| r.iter().any(|r| r["key"] == "tables:E-601")), "{}", job["guided_job"]["rules"]);
    let _ = std::fs::remove_dir_all(&dir);
}
