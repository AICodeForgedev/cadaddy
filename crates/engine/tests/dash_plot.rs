//! -PLOT: AutoCAD's command-line plot questions in their order, both with and without the
//! detailed configuration, ending in the same plot as PLOT.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::cmd::plotters::{PlotterStore, set_plotter_store};
use serde_json::{Value, json};

/// A plotter that writes the options it was given, as JSON.
fn fake_plot(_: &cadaddy_doc::Drawing, space: &cadaddy_doc::Space, opts: &Value) -> Result<Vec<u8>, String> {
    let mut o = opts.clone();
    o["space"] = json!(space.tab_name());
    Ok(o.to_string().into_bytes())
}

fn session() -> Session {
    set_io(IoHooks { read: |_, _| Err("no reader".into()), write: |_, _| Err("no writer".into()), plot: Some(fake_plot), read_pdf: None });
    set_plotter_store(PlotterStore::memory());
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    s
}

fn prompt(s: &Session) -> String {
    s.current_prompt().map(|p| p.display()).unwrap_or_default()
}

fn out_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("cadaddy-dash-plot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).display().to_string()
}

/// What the plotter was told for the plot written to `path`.
fn told(path: &str) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Answer each question in turn, checking it is the one expected.
fn answer(s: &mut Session, steps: &[(&str, &str)]) {
    for (question, reply) in steps {
        let p = prompt(s);
        assert!(p.starts_with(question), "expected `{question}…`, got `{p}`");
        s.cmdline(reply).unwrap();
    }
}

#[test]
fn the_short_sequence_plots_with_the_defaults() {
    let mut s = session();
    let path = out_path("short.pdf");
    s.cmdline("-plot").unwrap();
    answer(
        &mut s,
        &[
            ("Detailed plot configuration? [Yes/No] <No>:", ""),
            ("Enter a layout name or [?] <Model>:", ""),
            ("Enter a page setup name", ""),
            ("Enter an output device name or [?] <CADaddy PDF>:", ""),
            ("Enter file name <", &path),
            ("Save changes to page setup? [Yes/No] <No>:", ""),
            ("Proceed with plot? [Yes/No] <Yes>:", ""),
        ],
    );
    assert!(s.running.is_none());
    let o = told(&path);
    assert_eq!(o["space"], "Model");
    assert_eq!(o["output"], "pdf");
    assert_eq!(o["fit"], true);
    assert_eq!(o["upsideDown"], false);
    assert!(s.log.iter().any(|l| l.contains("Plot written to")), "{:?}", s.log);
}

#[test]
fn the_detailed_sequence_reaches_the_plotter() {
    let mut s = session();
    s.execute("setvar", &json!({"name": "MEASUREMENT", "value": 1})).unwrap();
    let path = out_path("detailed");
    s.cmdline("-PLOT").unwrap();
    answer(
        &mut s,
        &[
            ("Detailed plot configuration?", "Y"),
            ("Enter a layout name or [?] <Model>:", "Model"),
            ("Enter an output device name or [?]", "cadaddy png"),
            ("Enter paper size or [?]", "A3"),
            ("Enter paper units [Inches/Millimeters]", "M"),
            ("Enter drawing orientation [Portrait/Landscape]", "P"),
            ("Plot upside down? [Yes/No] <No>:", "Y"),
            ("Enter plot area [Display/Extents/Limits/View/Window] <Display>:", "W"),
            ("Enter lower left corner of window", "0,0"),
            ("Enter upper right corner of window", "100,50"),
            ("Enter plot scale (Plotted Millimeters=Drawing Units) or [Fit] <Fit>:", "1=2"),
            ("Enter plot offset (x,y) or [Center] <Center>:", "10,20"),
            ("Plot with plot styles? [Yes/No]", "Y"),
            ("Enter plot style table name or [?] (enter . for none)", "monochrome.ctb"),
            ("Plot with lineweights? [Yes/No]", "N"),
            ("Enter shade plot setting [As displayed/legacy Wireframe/legacy Hidden/Visual styles/Rendered] <As displayed>:", "As displayed"),
            ("Enter file name <", &path),
            ("Save changes to page setup?", "N"),
            ("Proceed with plot?", "Y"),
        ],
    );
    assert!(s.running.is_none(), "{}", prompt(&s));
    // The device's extension is added to the typed name.
    let o = told(&format!("{path}.png"));
    assert_eq!(o["output"], "png");
    assert_eq!(o["landscape"], false);
    assert_eq!(o["upsideDown"], true);
    assert_eq!(o["fit"], false);
    assert!((o["scale"].as_f64().unwrap() - 0.5).abs() < 1e-12, "{o}");
    assert_eq!(o["center"], false);
    assert_eq!(o["offset"], json!([10.0, 20.0]));
    assert_eq!(o["window"], json!([[0.0, 0.0], [100.0, 50.0]]));
    assert_eq!(o["plotStyleTable"], "monochrome.ctb");
    assert_eq!(o["lineweights"], false);
    // A3 from the PNG device: its size in millimetres.
    let (w, h) = (o["width"].as_f64().unwrap(), o["height"].as_f64().unwrap());
    assert!((w.min(h) - 297.0).abs() < 1e-9 && (w.max(h) - 420.0).abs() < 1e-9, "{o}");
}

#[test]
fn inches_scale_and_offset_become_paper_units() {
    let mut s = session();
    s.execute("setvar", &json!({"name": "MEASUREMENT", "value": 1})).unwrap();
    let path = out_path("inches.pdf");
    // A plot script: one answer per line, the device name with its space on one line.
    let script = format!("-PLOT\nY\nModel\nCADaddy PDF\nLetter\nI\nL\nN\nE\n1=48\n1,0.5\nN\nY\nA\n{path}\nN\nY\n");
    s.script(&script).unwrap();
    assert!(s.running.is_none(), "{}", prompt(&s));
    let o = told(&path);
    // Metric drawing (paper unit mm): 1 inch = 48 units ⟹ 25.4/48 mm per unit.
    assert!((o["scale"].as_f64().unwrap() - 25.4 / 48.0).abs() < 1e-9, "{o}");
    assert_eq!(o["offset"], json!([25.4, 12.7]));
    assert_eq!(o["plotArea"], "extents");
    assert_eq!(o["plotStyleTable"], "None", "plot styles off");
}

#[test]
fn wrong_answers_ask_again_and_question_marks_list() {
    let mut s = session();
    s.cmdline("-plot").unwrap();
    s.cmdline("maybe").unwrap();
    assert!(prompt(&s).starts_with("Detailed plot configuration?"));
    s.cmdline("y").unwrap();
    s.cmdline("NoSuchLayout").unwrap();
    assert!(prompt(&s).starts_with("Enter a layout name"));
    assert!(s.log.iter().any(|l| l.contains("Layout \"NoSuchLayout\" not found.")));
    s.cmdline("?").unwrap();
    assert!(s.log.iter().any(|l| l.starts_with("Layouts: Model, Layout1")), "{:?}", s.log);
    s.cmdline("").unwrap();
    s.cmdline("?").unwrap();
    assert!(s.log.iter().any(|l| l.contains("CADaddy SVG")));
    s.cmdline("No Such Plotter").unwrap();
    assert!(prompt(&s).starts_with("Enter an output device name"));
    s.cmdline("").unwrap();
    s.cmdline("Z9").unwrap();
    assert!(prompt(&s).starts_with("Enter paper size"));
    s.cmdline("").unwrap();
    assert!(prompt(&s).starts_with("Enter paper units"));
    // Escape leaves without plotting.
    s.cancel();
    assert!(s.running.is_none());
}

#[test]
fn a_layout_asks_the_paper_space_questions_and_saves_its_page_setup() {
    let mut s = session();
    let path = out_path("layout.svg");
    s.cmdline("-plot").unwrap();
    answer(
        &mut s,
        &[
            ("Detailed plot configuration?", "Y"),
            ("Enter a layout name or [?] <Model>:", "layout1"),
            ("Enter an output device name or [?]", "CADaddy SVG"),
            ("Enter paper size or [?]", "A3"),
            ("Enter paper units", ""),
            ("Enter drawing orientation", "L"),
            ("Plot upside down?", ""),
            ("Enter plot area [Display/Extents/Layout/View/Window] <Layout>:", ""),
            ("Enter plot scale", "1:1"),
            ("Enter plot offset (x,y) <0.00,0.00>:", ""),
            ("Plot with plot styles?", "N"),
            ("Plot with lineweights?", "Y"),
            ("Scale lineweights with plot scale? [Yes/No] <No>:", ""),
            ("Plot paper space first? [Yes/No] <No>:", ""),
            ("Hide paperspace objects? [Yes/No] <No>:", ""),
            ("Enter file name <", &path),
            ("Save changes to page setup?", "Y"),
            ("Proceed with plot?", ""),
        ],
    );
    assert!(s.running.is_none(), "{}", prompt(&s));
    let o = told(&path);
    assert_eq!(o["space"], "Layout1");
    assert_eq!(o["output"], "svg");
    assert_eq!(o["plotArea"], "layout");
    let page = &s.doc().unwrap().layout("Layout1").unwrap().page;
    assert_eq!(page.device, "CADaddy SVG");
    assert!(page.paper.contains("A3"), "{}", page.paper);
    assert!(page.landscape);
    assert!(!page.scale_to_fit);
}

#[test]
fn the_json_form_plots_like_plot() {
    let mut s = session();
    let r = s.execute("-plot", &json!({"device": "CADaddy PDF", "upsideDown": true})).unwrap();
    let o: Value = serde_json::from_slice(&cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(o["upsideDown"], true);
    assert_eq!(o["output"], "pdf");
    // A named view as the plot area.
    s.execute("view", &json!({"option": "save", "name": "V1"})).ok();
    if s.doc().unwrap().views.iter().any(|v| v.name == "V1") {
        let r = s.execute("-plot", &json!({"plotArea": "view", "view": "v1"})).unwrap();
        let o: Value = serde_json::from_slice(&cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap()).unwrap()).unwrap();
        assert!(o["window"].is_array(), "{o}");
    }
    assert!(s.execute("-plot", &json!({"plotArea": "view", "view": "nope"})).is_err());
}
