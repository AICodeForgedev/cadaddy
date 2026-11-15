//! Q6669: a mechanical plan printed without PDF layers reads its double-line ducts in the job.
//! The import puts the print's heavy pen (the trade's ductwork, units and air devices on a
//! mechanical plan) on its wall layer; the job's MEP read took nothing from it, or the pairs of
//! other lines. The made-up hotel's set (seed 42 prints flat, its ducts in a heavy pen, each
//! drawn as an outline with a dashed centreline and tagged `24/14`) goes through the whole job.

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
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

#[test]
fn a_flat_printed_mechanical_plan_reads_its_ducts_in_the_job() {
    real_pdf_reader();
    let seed = 42;
    let (_, style) = cadaddy_mepfix::hotel_planned(seed, HotelSize::Small);
    assert!(!style.pdf_layers, "the set prints without PDF layers");
    let mut set = generate_hotel(seed, HotelSize::Small).expect("generate hotel");
    let only = ["A-101", "M-101"];
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let level = set.truth.sheets.iter().find(|s| s.number == "M-101").and_then(|s| s.level.clone()).expect("M-101 level");
    let want: f64 = set
        .truth
        .segments
        .iter()
        .filter(|s| s.kind == "duct" && s.level == level && !s.vertical)
        .map(|s| {
            let [a, b] = s.path_ft;
            (b[0] - a[0]).hypot(b[1] - a[1])
        })
        .sum();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_flatducts_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let job: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state");
    let read = job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == "mep_read")).expect("mep_read")["result"].clone();
    let m101 = read["sheets"].as_array().and_then(|a| a.iter().find(|r| r["sheet"] == "M-101" && r["trade"] == "duct")).cloned().expect("M-101 read");
    let ft = m101["length_ft"].as_f64().unwrap_or(0.0);
    eprintln!("M-101: {ft:.1} ft of duct read, {want:.1} ft drawn; {m101}");
    assert!(want > 0.0);
    assert!((ft - want).abs() <= 0.15 * want, "M-101's ducts as drawn: {ft:.1} ft read of {want:.1} ft");
    let _ = std::fs::remove_dir_all(&dir);
}
