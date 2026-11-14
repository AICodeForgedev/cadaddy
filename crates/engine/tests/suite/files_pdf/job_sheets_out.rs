//! Q6577: once a guided job's build has read the print sheets, the sheets go out of memory
//! (parked as the .cdy file stores them, [`cadaddy_engine::sheetstore`]): the takeoff, estimate
//! and clash read the model's elements, never the sheets' linework. Before the fix the job kept
//! every imported sheet in memory to its end (about 1.1 GB on a four-storey set).
//!
//! Nothing the job keeps holds them either (undo, the drawing as saved, the build's record of the
//! drawing it left), the job's results are what they are with the sheets in memory, a headless
//! session has them back when the job's command ends, a save writes them, and the build's record
//! still knows the plan as a whole: a build after the job keeps the model.

use crate::helper::{run, scratch, serial};
use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
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

/// The made-up office's floor plan and electrical plan (seed 1), run unattended to `goal`.
fn job(goal: &str) -> (Session, std::path::PathBuf) {
    let set = cadaddy_mepfix::generate(1).expect("generate");
    let dir = scratch(&format!("job-sheets-out-{goal}"));
    let prints = dir.join("prints");
    std::fs::create_dir_all(&prints).expect("prints folder");
    for name in ["A-101.pdf", "E-101.pdf"] {
        std::fs::write(prints.join(name), set.file(name).expect("sheet")).expect("write");
    }
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Drawing1", None);
    let mut st = run(&mut s, "job.start", json!({"pdfs": prints.display().to_string(), "goal": goal, "auto": true, "checkpoint": false}));
    for _ in 0..50 {
        if st["done"] == json!(true) {
            break;
        }
        st = run(&mut s, "job.next", json!({}));
    }
    assert_eq!(st["done"], json!(true), "{st}");
    (s, dir)
}

/// Objects of the plan (no extended data: the sheets' linework and lettering) in model space.
fn plain(d: &Drawing) -> usize {
    d.model.iter().filter(|e| e.common.xdata.is_empty()).count()
}

/// What the job's audit says it put out of memory once its build had read the sheets.
fn put_out(s: &Session) -> u64 {
    let agent: Value = s.doc().ok().and_then(|d| d.agent.as_deref().and_then(|t| serde_json::from_str(t).ok())).unwrap_or_default();
    let audit = agent["guided_job"]["audit"].as_array().cloned().unwrap_or_default();
    audit.iter().filter(|a| a["event"] == "sheets_out").filter_map(|a| a["details"]["objects"].as_u64()).sum()
}

#[test]
fn a_jobs_later_stages_hold_the_sheets_out_of_memory() {
    let _g = serial();
    real_pdf_reader();
    let (mut s, dir) = job("clash");

    // Once the build had read them, the sheets went out of memory, and no later stage (takeoff,
    // estimate, clash) loaded them again.
    let out = put_out(&s);
    assert!(out > 1000, "the job kept its sheets in memory after its build ({out} objects out)");
    assert_eq!(s.whole_set_loads.0, 0, "loaded again by {:?}", s.whole_set_loads.1);
    // Nothing the session keeps held them meanwhile: the drawing as saved, its undo and redo
    // steps, the drawing the build's record keeps.
    {
        let st = s.state().expect("state");
        let mut kept: Vec<(String, std::sync::Arc<Drawing>)> = vec![("the drawing as saved".into(), st.saved.clone())];
        kept.extend(st.undo.iter().chain(st.redo.iter()).map(|x| (format!("undo step {}", x.label), x.doc.clone())));
        let built = st.model_records.last().and_then(|r| r.built.as_ref()).map(|b| b.0.clone());
        assert!(built.is_some(), "the job's build is recorded with the drawing it left");
        kept.extend(built.map(|b| ("the build's record".to_string(), b)));
        for (what, d) in kept {
            assert!((plain(&d) as u64) < out / 10, "{what} holds the sheets: {} plan objects ({out} went out)", plain(&d));
        }
    }
    // The job's command over, a headless session holds everything again (an export, an agent's
    // selection read the drawing as it is).
    assert!(!s.has_parked_sheets(), "the sheets are still out of memory after the job");
    let total = s.doc().expect("doc").model.len();
    assert!(plain(s.doc().expect("doc")) as u64 >= out, "the sheets' objects are back");

    // Saved, the sheets are in the file: opened again, every object is there.
    let path = dir.join("built.cdy");
    run(&mut s, "saveas", json!({"path": path.display().to_string()}));
    let mut again = Session::empty();
    run(&mut again, "open", json!({"path": path.display().to_string()}));
    assert_eq!(again.doc().expect("doc").model.len(), total, "the saved model holds every object");

    // The takeoff the job made with the sheets out is the takeoff with them in.
    let stage = run(&mut s, "job.explain", json!({"stage": "takeoff"}));
    let now = run(&mut s, "bim.takeoff", json!({"group_by": "level", "keep": false}));
    let lines = |v: &Value| v["lines"].as_array().cloned().unwrap_or_default();
    assert!(!lines(&now).is_empty(), "{now}");
    assert_eq!(lines(&stage["result"]), lines(&now), "the job's takeoff differs from the one with the sheets in memory");

    // The build's record knows the plan as a whole: the model is kept by a build after the job.
    let built = run(&mut s, "model.build", json!({}));
    assert_eq!(built["kept"], json!(true), "the model was built again after the job: {}", built["rebuild"]);
    let _ = std::fs::remove_dir_all(&dir);
}
