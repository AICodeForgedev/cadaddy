//! Q6498: a trade's plan sheet that prints its floor's plan and, beside it, two small enlarged
//! plans at twice the floor plan's scale (a power plan with its kitchen and restroom enlarged)
//! comes in at the floor plan's scale, and the devices it draws are placed as before. Read at
//! the enlarged plans' scale (the scale more of its plan views print), the floor's plan came in
//! at half its size: its fit by its tags was off its printed scale, its linework (drawn alike on
//! every trade sheet of the floor) told nothing, and its devices were placed nowhere.
//! Made-up sheets of the generated set only.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::Space;
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

/// `1/4"` for 0.25 sheet inches per foot.
fn inches(k: f64) -> String {
    [(0.125, "1/8\""), (0.1875, "3/16\""), (0.25, "1/4\""), (0.375, "3/8\""), (0.5, "1/2\""), (0.75, "3/4\"")]
        .iter()
        .find(|(v, _)| (k - v).abs() < 1e-9)
        .map_or_else(|| format!("{k}\""), |(_, s)| (*s).to_string())
}

/// The trades whose plans get the two enlarged plans.
const TRADES: [&str; 5] = ["E", "P", "M", "FA", "T"];

/// The generated set's sheets `only` as one PDF, each trade's plan ([`TRADES`]) with two enlarged
/// plans at twice its scale under its floor plan.
fn set_pdf(only: &[&str]) -> Vec<u8> {
    let (t, st) = cadaddy_mepfix::hotel_planned(42, HotelSize::Small);
    let kinds: Vec<(String, layout::Kind)> = layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let mut pages = Vec::new();
    for sh in t.sheets.iter().filter(|s| only.contains(&s.number.as_str())) {
        let kind = &kinds.iter().find(|(n, _)| *n == sh.number).expect("a generated sheet").1;
        let (mut drawing, _, _) = draw::draw(&t, &st, sh, kind);
        if TRADES.contains(&sh.discipline.as_str()) && sh.scale_text.is_some() && sh.title.contains("PLAN") {
            let mut pen = draw::Pen::new(sh, &st);
            let [w, h] = sh.paper_in;
            for (i, name) in ["ENLARGED PLAN - ROOM A", "ENLARGED PLAN - ROOM B"].iter().enumerate() {
                let (x, y) = (w * 0.35 + i as f64 * 4.2, h * 0.09);
                let (a, b) = (pen.p([x, y]), pen.p([x + 2.6, y + 1.2]));
                pen.rect("A-WALL", a, b);
                pen.ptext("TEXT", [x, y - 0.35], 0.14, name, cadaddy_doc::HAlign::Left);
                pen.ptext("TEXT", [x, y - 0.6], 0.1, &format!("SCALE: {} = 1'-0\"", inches(sh.scale * 2.0)), cadaddy_doc::HAlign::Left);
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
    cadaddy_io::pdf_merge::merge(&pages).expect("merge")
}

#[test]
fn a_plan_sheets_enlarged_views_dont_set_its_scale_and_its_devices_are_placed() {
    let _g = serial();
    real_pdf_reader();
    // The first floor's sheets: its floor plan and ceiling plan, every trade's plan of it, and
    // a framing plan of the floor above.
    let only = ["A-101", "A-201", "E-101", "E-201", "FA-101", "T-101", "M-101", "P-101", "FP-101", "S-102"];
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_enlargedviews_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("Sample Set.pdf"), set_pdf(&only)).expect("set");
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}));
    let _ = std::fs::remove_dir_all(&dir);
    let st = st.expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");

    // Every device read is placed onto the floor's plan.
    let devices = s.execute("job.explain", &json!({"stage": "devices"})).expect("explain devices");
    let read = devices["result"]["devices"]["count"].as_u64().unwrap_or(0);
    let built = s.execute("job.explain", &json!({"stage": "bim_build"})).expect("explain bim_build");
    let dv = &built["result"]["devices"];
    let (inserted, not_placed) = (dv["inserted"].as_u64().unwrap_or(0), dv["not_placed"].as_u64().unwrap_or(0));
    assert!(read > 20 && inserted > 20, "the trade plans draw devices: read {read}, {dv:#}");
    assert_eq!(not_placed, 0, "{dv:#}");

    // Each trade plan came in at its floor plan's scale (3/16": 64 drawing inches a sheet inch),
    // not at its enlarged plans' (3/8": 32).
    let agent = s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).unwrap_or_default();
    let state: Value = serde_json::from_str(&agent).unwrap_or_default();
    let scales = state["guided_job"]["sheet_scales"].as_array().cloned().unwrap_or_default();
    for n in ["E-101", "E-201", "FA-101", "T-101", "M-101", "P-101"] {
        let sc = scales.iter().find(|x| x["sheet"].as_str().is_some_and(|v| v.starts_with(n))).unwrap_or_else(|| panic!("{n} in {scales:?}"));
        assert_eq!(sc["scale"].as_f64(), Some(64.0), "{n}: {sc}");
    }
}
