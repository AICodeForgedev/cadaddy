//! VISUALSTYLES: the Realistic style's ambient occlusion, soft shadows and tone curve, set as
//! JSON or typed, kept with the drawing; and plots in a visual style (PLOT's `shadePlot`, -PLOT's
//! shade plot answers) reaching the plotter.

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
    set_io(IoHooks {
        read: |_, _| Err("no reader".into()),
        write: |_, _| Err("no writer".into()),
        plot: Some(fake_plot),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    set_plotter_store(PlotterStore::memory());
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    s
}

fn prompt(s: &Session) -> String {
    s.current_prompt().map(|p| p.display()).unwrap_or_default()
}

fn out_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("cadaddy-realistic-look-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).display().to_string()
}

/// What the plotter was told for the plot written to `path`.
fn told(path: &str) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn visualstyles_sets_the_realistic_look() {
    let mut s = session();
    // Read only: the defaults.
    let r = s.execute("visualstyles", &json!({})).unwrap();
    assert_eq!(r, json!({"ambientOcclusion": 1.0, "aoRadius": 0.0, "softShadows": 1.0, "toneMap": "filmic"}));
    let r = s.execute("visualstyles", &json!({"ambientOcclusion": 0.5, "aoRadius": 300, "softShadows": 2.5, "toneMap": "Neutral"})).unwrap();
    assert_eq!(r, json!({"ambientOcclusion": 0.5, "aoRadius": 300.0, "softShadows": 2.5, "toneMap": "neutral"}));
    let look = &s.doc().unwrap().sun.look;
    assert_eq!((look.ambient_occlusion, look.ao_radius, look.soft_shadows), (0.5, 300.0, 2.5));
    assert_eq!(look.tone_map, cadaddy_doc::lighting::ToneMap::Neutral);
    // It is a change to the drawing: undo puts it back.
    s.execute("u", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().sun.look.ambient_occlusion, 1.0);
    // SUNPROPERTIES reports it too.
    let sun = s.execute("sunproperties", &json!({})).unwrap();
    assert_eq!(sun["look"]["toneMap"], "filmic");
}

#[test]
fn bad_and_hostile_values_are_refused() {
    let mut s = session();
    for p in [
        json!({"ambientOcclusion": -1}),
        json!({"ambientOcclusion": 2.5}),
        json!({"ambientOcclusion": "lots"}),
        json!({"aoRadius": -3}),
        json!({"aoRadius": 1e300}),
        json!({"softShadows": 11}),
        json!({"softShadows": -0.5}),
        json!({"toneMap": "sepia"}),
        json!({"ambientOcclusion": 1e308, "softShadows": f64::MAX}),
    ] {
        assert!(s.execute("visualstyles", &p).is_err(), "{p}");
    }
    assert_eq!(s.doc().unwrap().sun.look, cadaddy_doc::lighting::Look::default());
}

#[test]
fn visualstyles_typed_at_the_command_line() {
    let mut s = session();
    s.cmdline("vsm").unwrap();
    assert!(prompt(&s).starts_with("Enter an option or [Ambient occlusion/Radius/Soft shadows/Tone map/eXit] <eXit>:"), "{}", prompt(&s));
    s.cmdline("A").unwrap();
    assert!(prompt(&s).starts_with("Ambient occlusion strength (0 off … 2) <1>:"), "{}", prompt(&s));
    s.cmdline("1.5").unwrap();
    s.cmdline("S").unwrap();
    s.cmdline("4").unwrap();
    s.cmdline("T").unwrap();
    assert!(prompt(&s).starts_with("Tone curve or [Filmic/Reinhard/Neutral/Linear] <Filmic>:"), "{}", prompt(&s));
    s.cmdline("R").unwrap();
    // A bad value says so and asks again for the option.
    s.cmdline("R").unwrap();
    s.cmdline("-2").unwrap();
    assert!(s.log.iter().any(|l| l.contains("`aoRadius` must be from 0")), "{:?}", s.log);
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
    let look = &s.doc().unwrap().sun.look;
    assert_eq!((look.ambient_occlusion, look.ao_radius, look.soft_shadows), (1.5, 0.0, 4.0));
    assert_eq!(look.tone_map, cadaddy_doc::lighting::ToneMap::Reinhard);
}

#[test]
fn shade_plot_puts_model_space_in_a_visual_style() {
    let mut s = session();
    // The plan in 2D Wireframe plots its vectors…
    let plain = out_path("plain.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": plain})).unwrap();
    assert!(told(&plain).get("shade").is_none());
    // …unless a shade plot asks for a visual style.
    let real = out_path("real.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": real, "shadePlot": "realistic"})).unwrap();
    assert_eq!(told(&real)["shade"]["style"], "Realistic");
    let rendered = out_path("rendered.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": rendered, "shadePlot": "Rendered"})).unwrap();
    assert_eq!(told(&rendered)["shade"]["style"], "Realistic");
    let hidden = out_path("hidden.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": hidden, "shadePlot": "legacy Hidden"})).unwrap();
    assert_eq!(told(&hidden)["shade"]["style"], "Hidden");
    // Wireframe in the plan stays the vector plot; as displayed is the default.
    for (name, sp) in [("wire.pdf", "wireframe"), ("shown.pdf", "As displayed")] {
        let path = out_path(name);
        s.execute("plot", &json!({"layout": "Model", "path": path, "shadePlot": sp})).unwrap();
        assert!(told(&path).get("shade").is_none(), "{sp}");
    }
    assert!(s.execute("plot", &json!({"layout": "Model", "path": out_path("x.pdf"), "shadePlot": "sketchy"})).is_err());
    // In a 3D view shown Realistic, a Shaded shade plot overrides it.
    s.execute("vscurrent", &json!({"style": "realistic"})).unwrap();
    s.execute("view.dir", &json!({"preset": "swiso"})).unwrap();
    let shown = out_path("shown3d.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": shown})).unwrap();
    assert_eq!(told(&shown)["shade"]["style"], "Realistic");
    let shaded = out_path("shaded3d.pdf");
    s.execute("plot", &json!({"layout": "Model", "path": shaded, "shadePlot": "shaded"})).unwrap();
    assert_eq!(told(&shaded)["shade"]["style"], "Shaded");
}

/// Answer each question in turn, checking it is the one expected.
fn answer(s: &mut Session, steps: &[(&str, &str)]) {
    for (question, reply) in steps {
        let p = prompt(s);
        assert!(p.starts_with(question), "expected `{question}…`, got `{p}`");
        s.cmdline(reply).unwrap();
    }
}

/// -PLOT in detail up to the shade plot question, Model, the whole display.
fn up_to_shade(s: &mut Session) {
    s.cmdline("-plot").unwrap();
    answer(
        s,
        &[
            ("Detailed plot configuration?", "Y"),
            ("Enter a layout name or [?] <Model>:", ""),
            ("Enter an output device name or [?]", ""),
            ("Enter paper size or [?]", ""),
            ("Enter paper units", ""),
            ("Enter drawing orientation", ""),
            ("Plot upside down?", ""),
            ("Enter plot area", ""),
            ("Enter plot scale", ""),
            ("Enter plot offset", ""),
            ("Plot with plot styles?", ""),
            ("Enter plot style table name", ""),
            ("Plot with lineweights?", ""),
        ],
    );
}

#[test]
fn dash_plot_shade_plot_visual_styles_and_rendered() {
    let mut s = session();
    let path = out_path("vs.pdf");
    up_to_shade(&mut s);
    answer(
        &mut s,
        &[
            ("Enter shade plot setting [As displayed/legacy Wireframe/legacy Hidden/Visual styles/Rendered] <As displayed>:", "V"),
            ("Enter an option [Wireframe/Hidden/Realistic/Shaded/shaded with Edges] <Realistic>:", "E"),
            ("Enter file name <", &path),
            ("Save changes to page setup?", ""),
            ("Proceed with plot?", ""),
        ],
    );
    assert!(s.running.is_none());
    let o = told(&path);
    assert_eq!(o["shadePlot"], "shadedWithEdges");
    assert_eq!(o["shade"]["style"], "Shaded with Edges");

    let path = out_path("rendered.pdf");
    up_to_shade(&mut s);
    answer(
        &mut s,
        &[("Enter shade plot setting", "Rendered"), ("Enter file name <", &path), ("Save changes to page setup?", ""), ("Proceed with plot?", "")],
    );
    assert_eq!(told(&path)["shade"]["style"], "Realistic");
    assert!(s.log.iter().any(|l| l.contains("plots in the Realistic visual style")), "{:?}", s.log);
}
