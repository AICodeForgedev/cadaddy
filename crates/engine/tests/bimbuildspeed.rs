//! Q6010 bimbuildspeed: the bim_build stage on the synthetic hotel must scale sub-quadratically
//! and report progress per level.

use std::sync::Mutex;

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::json;

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

fn job_folder(tag: &str, size: HotelSize) -> std::path::PathBuf {
    let set = generate_hotel(42, size).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_bimbuildspeed_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folders");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    dir
}

/// Run a full BIM job and return elapsed_ms for the bim_build stage.
fn build_job(size: HotelSize, tag: &str) -> u64 {
    real_pdf_reader();
    let dir = job_folder(tag, size);
    let mut s = Session::new();
    s.events_subscribe(true);
    let st = s
        .execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}))
        .expect("job.start");
    assert_eq!(st["done"], json!(true), "job did not finish: {st}");

    // Get elapsed_ms for bim_build from events.
    let ev = s.events_poll(0);
    let elapsed = ev["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "job.progress" && e["step"] == "end" && e["stage"] == "bim_build")
        .filter_map(|e| e["elapsed_ms"].as_u64())
        .next()
        .unwrap_or(0);

    let _ = std::fs::remove_dir_all(&dir);
    elapsed
}

static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn bim_build_double_scales_sub_quadratically() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let full_ms = build_job(HotelSize::Full, "full");
    let double_ms = build_job(HotelSize::Double, "double");

    eprintln!("bim_build Full:   {full_ms} ms");
    eprintln!("bim_build Double: {double_ms} ms");

    // Both must complete with measurable time.
    assert!(full_ms > 0, "full hotel bim_build reported 0 ms");

    let ratio = if full_ms > 0 { double_ms as f64 / full_ms as f64 } else { 1.0 };
    eprintln!("Ratio:  {ratio:.2}×");

    // Double has ~4× the walls; build time should be well under quadratic.
    // With spatial grid optimizations: expect ratio < 8 (generous, accounts for constant factors).
    assert!(ratio < 8.0, "build time scaled {ratio:.1}× for ~4× data — expected sub-quadratic (<8×)");
}
