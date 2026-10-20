//! PLOTSTAMP and batch plotting: the stamp's text is in the plotted PDF where a sheet carries it
//! (a layout's page setup, model space's setting, or forced), not where it doesn't; PUBLISH plots
//! a folder of drawings to one PDF each, going on past one that fails.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, base64_decode, set_io};
use cadaddy_engine::fields::{self, LocalTime};
use serde_json::{Value, json};

fn clock() -> Option<LocalTime> {
    Some(LocalTime { year: 2026, month: 10, day: 19, hour: 9, minute: 5, second: 0, weekday: 1 })
}

fn hooks() {
    fields::set_clock(Some(clock));
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
}

/// A drawing with a line in model space and on Layout1.
fn session() -> Session {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    s
}

/// The PDF of a plot, uncompressed so its text can be searched.
fn plot(s: &mut Session, p: Value) -> String {
    let mut p = p;
    p["compress"] = json!(false);
    let r = s.execute("plot", &p).unwrap();
    String::from_utf8_lossy(&base64_decode(r["data"].as_str().unwrap()).unwrap()).into_owned()
}

fn layout(s: &Session) -> String {
    s.doc().unwrap().layouts.first().unwrap().name.clone()
}

#[test]
fn the_stamp_is_plotted_where_a_sheet_carries_it() {
    let mut s = session();
    let l = layout(&s);
    // Off by default.
    assert!(!plot(&mut s, json!({"layout": l})).contains("/FS"));
    // On for the layout, with the scale and paper and a field of our own.
    let r = s
        .execute(
            "plotstamp",
            &json!({"layout": l, "on": true, "paperSize": true, "plotScale": true, "userFields": ["Issued for bid"], "corner": "topRight"}),
        )
        .unwrap();
    assert_eq!(r["layouts"], json!([l]), "{r}");
    let pdf = plot(&mut s, json!({"layout": l}));
    let title = s.state().unwrap().title.clone();
    // As a PDF string: parentheses escaped.
    let paper = s.doc().unwrap().layouts[0].page.paper.replace('(', "\\(").replace(')', "\\)");
    let line = format!("({title}, {l}, 2026-10-19 09:05, None, {paper}, 1:1, Issued for bid) Tj");
    assert!(
        pdf.contains(&line),
        "stamp `{line}` not in the PDF: {:?}",
        pdf.lines().filter(|l| l.contains("Tj") || l.contains("/FS")).collect::<Vec<_>>()
    );
    assert!(pdf.contains("/BaseFont /Helvetica"));
    // Model space follows its own setting; forcing works both ways.
    assert!(!plot(&mut s, json!({"layout": "Model"})).contains("/FS"));
    s.execute("plotstamp", &json!({"layout": "Model", "on": true, "font": "courier", "dateTime": false})).unwrap();
    let pdf = plot(&mut s, json!({"layout": "Model"}));
    assert!(pdf.contains("/BaseFont /Courier"));
    assert!(pdf.contains(&format!("({title}, Model, None, ")), "no date, model space's own paper next");
    assert!(pdf.contains("Fit 1:"), "a fitted model plot says so");
    assert!(!plot(&mut s, json!({"layout": "Model", "stamp": false})).contains("/FS"));
    // Several sheets in one PDF: each its own layout name.
    let pdf = plot(&mut s, json!({"layouts": ["Model", l], "stamp": true}));
    assert!(pdf.contains(&format!("({title}, Model,")) && pdf.contains(&format!("({title}, {l},")));
}

#[test]
fn settings_are_checked_and_kept_with_the_drawing() {
    let mut s = session();
    let l = layout(&s);
    for bad in [
        json!({"height": 0}),
        json!({"height": f64::MAX}),
        json!({"offset": [1]}),
        json!({"offset": [1e9, 0]}),
        json!({"font": "Comic Sans"}),
        json!({"corner": "middle"}),
        json!({"units": "furlongs"}),
        json!({"userFields": [1, 2]}),
        json!({"userFields": ["a", "b", "c", "d", "e", "f", "g", "h", "i"]}),
        json!({"on": "yes"}),
        json!({"layout": "Nope", "on": true}),
    ] {
        assert!(s.execute("plotstamp", &bad).is_err(), "{bad}");
    }
    let r = s.execute("plotstamp", &json!({"layout": l, "on": true, "offset": [0.5, 0.25], "units": "inches", "height": 0.1})).unwrap();
    assert_eq!(r["settings"]["offsetMm"], json!([12.7, 6.35]));
    assert!((r["settings"]["heightMm"].as_f64().unwrap() - 2.54).abs() < 1e-9);
    // Through DXF and .cdy and back.
    let d = s.doc().unwrap().clone();
    for name in ["x.dxf", "x.cdy"] {
        let back = cadaddy_io::read(&cadaddy_io::write(&d, name).unwrap(), name).unwrap();
        assert_eq!(back.plot_stamp, d.plot_stamp, "{name}");
        assert!(back.layout(&l).unwrap().page.plot_stamp, "{name}");
    }
    // `allLayouts` and `reset`.
    s.execute("layout", &json!({"option": "new", "name": "Second"})).ok();
    let r = s.execute("plotstamp", &json!({"allLayouts": false, "reset": true})).unwrap();
    assert_eq!(r["layouts"], json!([]));
    assert_eq!(r["settings"]["offsetMm"], json!([2.0, 2.0]));
}

#[test]
fn a_folder_of_drawings_plots_to_one_pdf_each() {
    let dir = std::env::temp_dir().join(format!("cadaddy-batchplot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = session();
    s.execute("saveas", &json!({"path": dir.join("A-101.dxf").to_string_lossy()})).unwrap();
    s.execute("circle", &json!({"center": [10, 10], "radius": 5})).unwrap();
    s.execute("saveas", &json!({"path": dir.join("A-102.cdy").to_string_lossy()})).unwrap();
    std::fs::write(dir.join("broken.dwg"), b"AC1032 not really").unwrap();
    std::fs::write(dir.join("notes.txt"), b"skip me").unwrap();
    let out = dir.join("pdf");
    let r = s
        .execute(
            "publish",
            &json!({"folder": dir.to_string_lossy(), "out": out.to_string_lossy(), "layouts": ["Model"], "stamp": true, "compress": false}),
        )
        .unwrap();
    let done = r["drawings"].as_array().unwrap();
    assert_eq!(done.len(), 2, "{r}");
    assert_eq!(r["failed"].as_array().unwrap().len(), 1, "{r}");
    assert!(r["failed"][0]["drawing"].as_str().unwrap().ends_with("broken.dwg"));
    let mut pdfs: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    pdfs.sort();
    assert_eq!(pdfs, ["A-101.pdf", "A-102.pdf"]);
    // Each carries its own drawing's name in the stamp (compressed: inflate to look).
    for (pdf, name) in [("A-101.pdf", "A-101.dxf"), ("A-102.pdf", "A-102.cdy")] {
        let bytes = std::fs::read(out.join(pdf)).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let d = cadaddy_io::read(&bytes, pdf).unwrap();
        let texts: Vec<String> = d
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                cadaddy_doc::EntityKind::Text(t) => Some(t.value.clone()),
                cadaddy_doc::EntityKind::MText(t) => Some(t.contents.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.starts_with(&format!("{name}, Model, 2026-10-19 09:05"))), "{pdf}: {texts:?}");
    }
    // Nothing to plot in an empty folder.
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    assert!(s.execute("publish", &json!({"folder": dir.join("empty").to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_turned_stamp_and_the_log_file() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-stamplog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("plots.log");
    // The log needs a path, a `.log` one.
    assert!(s.execute("plotstamp", &json!({"logFile": true})).is_err());
    assert!(s.execute("plotstamp", &json!({"logFile": true, "logPath": dir.join("x.txt").to_string_lossy()})).is_err());
    let r =
        s.execute("plotstamp", &json!({"layout": "Model", "on": true, "vertical": true, "logFile": true, "logPath": log.to_string_lossy()})).unwrap();
    assert_eq!((r["logFile"].as_bool(), r["settings"]["vertical"].as_bool()), (Some(true), Some(true)), "{r}");
    // Turned a quarter: the text matrix runs up the sheet.
    let pdf = plot(&mut s, json!({"layout": "Model"}));
    assert!(pdf.lines().any(|l| l.contains("/FS") && l.contains(" 0 1 -1 0 ") && l.contains(" Tm (")), "a turned stamp");
    // A plot to a file adds a line to the log, each plot one more.
    let out = dir.join("plot.pdf");
    for _ in 0..2 {
        let r = s.execute("plot", &json!({"layout": "Model", "path": out.to_string_lossy()})).unwrap();
        assert_eq!(r["stampLog"].as_str(), Some(log.to_string_lossy().as_ref()), "{r}");
    }
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.starts_with("2026-10-19 09:05, ") && text.lines().all(|l| l.contains(", Model, ") && l.ends_with("plot.pdf")), "{text}");
    // Never saved with the drawing.
    let d = s.doc().unwrap().clone();
    for name in ["x.dxf", "x.cdy"] {
        let bytes = cadaddy_io::write(&d, name).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("plots.log"), "{name}");
        let back = cadaddy_io::read(&bytes, name).unwrap();
        let st = back.plot_stamp.unwrap();
        assert!(st.vertical && !st.log_file && st.log_path.is_empty(), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
