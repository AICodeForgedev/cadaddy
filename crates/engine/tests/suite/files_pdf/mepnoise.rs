//! MEP noise (Q6022) on a made-up printed set: the air devices a mechanical plan reads after
//! the PDF import are its tagged ones (never more than the plan draws), and every elbow the
//! takeoff counts is a standard fitting.

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
fn a_printed_mechanical_plan_reads_its_air_devices_and_standard_elbows() {
    real_pdf_reader();
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let only = ["A-101", "M-101"];
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let level = set.truth.sheets.iter().find(|s| s.number == "M-101").and_then(|s| s.level.clone()).expect("M-101 level");
    let drawn = set
        .truth
        .terminals
        .iter()
        .filter(|t| t.level == level && ["supply_diffuser", "return_grille", "exhaust_grille"].contains(&t.kind.as_str()))
        .count();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_mepnoise_{}_{nanos}", std::process::id()));
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
    let job: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state");
    let read = job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|s| s["name"] == "mep_read")).expect("mep_read")["result"].clone();
    let m101 = read["sheets"].as_array().and_then(|a| a.iter().find(|r| r["sheet"] == "M-101")).cloned().expect("M-101 read");
    let terminals = m101["terminals"].as_u64().unwrap_or(0) as usize;
    eprintln!("M-101: {terminals} air devices read of {drawn} drawn; {m101}");
    assert!(drawn > 0);
    assert!(terminals <= drawn, "more air devices than the plan draws: {terminals} > {drawn}");
    assert!(terminals * 2 >= drawn, "the tagged air devices are read: {terminals} of {drawn}");
    let q = s.execute("mep.qty", &json!({})).expect("mep.qty");
    let fittings = q["fittings"].as_array().cloned().unwrap_or_default();
    eprintln!("{}", serde_json::to_string(&fittings).unwrap_or_default());
    for f in &fittings {
        let ty = f["type"].as_str().unwrap_or("");
        if ty.starts_with("elbow") {
            assert!(["elbow 22.5°", "elbow 45°", "elbow 90°"].contains(&ty), "{f}");
        }
    }
}
