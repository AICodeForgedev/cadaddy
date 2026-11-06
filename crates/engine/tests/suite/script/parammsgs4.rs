//! Battle test 7 (B7-019, B7-020 partly): wrong values an agent gives are refused by name, and
//! the parameter lists the refusals offer are the command's own.

use cadaddy_engine::{
    Session, command_specs, documented_params, find_command, in_job_lane, offered_params, param_words, params_doc, takes_open_params, unknown_params,
};
use serde_json::{Value, json};

/// Writes every command's parameter list to `$PARAMS_DUMP` (to compare the doc reader before and
/// after a change): `id, open, names, offered, reach, marked, words`, tab-separated.
#[test]
#[ignore]
fn dump_documented_params() {
    let Ok(path) = std::env::var("PARAMS_DUMP") else { return };
    let mut out = String::new();
    for spec in command_specs() {
        let names = documented_params(params_doc(spec));
        let offered = offered_params(spec);
        let words = param_words(params_doc(spec));
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:?}\t{}\t{}\n",
            spec.id,
            takes_open_params(spec),
            names.join(","),
            offered.join(","),
            spec.reach(),
            spec.access.is_some(),
            words.join(",")
        ));
    }
    std::fs::write(path, out).unwrap();
}

/// The error an agent's call gets (the call must fail).
fn refusal(s: &mut Session, cmd: &str, params: Value) -> String {
    match s.execute_agent(cmd, &params) {
        Ok(v) => panic!("{cmd} {params} ran: {v}"),
        Err(e) => e.to_string(),
    }
}

fn known(cmd: &str, key: &str) -> bool {
    unknown_params(find_command(cmd).unwrap(), &json!({ key: 1 })).is_empty()
}

/// Battle test 7's object-valued sweep (sweep2.py) on this branch's base: each command and
/// parameter that ran with `{"a": [1]}` (301 outside the job, BIM and MEP lane).
const OBJECT_ACCEPTS: &[(&str, &str)] = &[
    ("checkexchange", "format"),
    ("mredo", "all"),
    ("select", "window"),
    ("select", "lasso"),
    ("select", "mode"),
    ("select", "aperture"),
    ("-vports", "save"),
    ("-vports", "restore"),
    ("-vports", "delete"),
    ("qvlayout", "thumbnails"),
    ("purge", "type"),
    ("purge", "blocks"),
    ("eattext", "blocks"),
    ("eattext", "wildcards"),
    ("eattext", "tags"),
    ("eattext", "Color"),
    ("draworder", "reference"),
    ("texttofront.text", "moved"),
    ("light.set", "changed"),
    ("agent.begin", "label"),
    ("redline", "layouts"),
    ("tframes", "imageframe"),
    ("tframes", "wipeoutframe"),
    ("groups.list", "groups"),
    ("groups.list", "name"),
    ("groups.list", "selectable"),
    ("groups.list", "unnamed"),
    ("groups.list", "members"),
    ("resume", "resumed"),
    ("hatch.patterns", "text"),
    ("drawing.summary", "extents"),
    ("drawing.summary", "layouts"),
    ("drawing.summary", "layers"),
    ("drawing.summary", "blocks"),
    ("drawing.summary", "texts"),
    ("drawing.summary", "titleBlocks"),
    ("drawing.summary", "references"),
    ("commandhistory", "clear"),
    ("commandhistory", "lines"),
    ("commandhistory", "unscriptable"),
    ("commandhistory", "first_at"),
    ("commandhistory", "message"),
    ("constraintsettings", "infer"),
    ("constraintsettings", "autoTypes"),
    ("constraintsettings", "tangentIntersect"),
    ("constraintsettings", "perpendicularIntersect"),
    ("constraintsettings", "barsVisible"),
    ("renderenvironment", "background"),
    ("renderenvironment", "kind"),
    ("renderenvironment", "image"),
    ("renderenvironment", "color"),
    ("renderenvironment", "bottom"),
    ("renderenvironment", "top"),
    ("etransmit", "layout"),
    ("camera.list", "cameras"),
    ("camera.list", "name"),
    ("camera.list", "target"),
    ("camera.list", "lens"),
    ("camera.list", "fieldOfView"),
    ("scalelistedit", "name"),
    ("scalelistedit", "ratio"),
    ("texttofront.dimensions", "moved"),
    ("lightlist", "lights"),
    ("lightlist", "name"),
    ("lightlist", "kind"),
    ("lightlist", "target"),
    ("getsel", "wildcard"),
    ("getsel", "selected"),
    ("laywalk", "show"),
    ("layout", "template"),
    ("layout", "saveas"),
    ("layout.list", "name"),
    ("layout.list", "tabOrder"),
    ("layout.list", "page"),
    ("layout.list", "viewports"),
    ("pagesetup", "layout"),
    ("pagesetup", "b"),
    ("pagesetup", "r"),
    ("plotstyle", "CPLOTSTYLE"),
    ("plotstamp", "layout"),
    ("sysvarmonitor", "reset"),
    ("-linetype", "current"),
    ("-linetype", "load"),
    ("-linetype", "reload"),
    ("tablestyle", "name"),
    ("tablestyle", "textHeight"),
    ("tablestyle", "title"),
    ("tablestyle", "header"),
    ("tablestyle", "current"),
    ("tablestyle", "cellStyle"),
    ("drawing.query", "Circle"),
    ("drawing.query", "current"),
    ("drawing.query", "model"),
    ("countlist", "current"),
    ("countlist", "model"),
    ("activity", "clear"),
    ("activity", "command"),
    ("activity", "seq"),
    ("actions", "name"),
    ("actions", "macros"),
    ("3darray", "rowSpacing"),
    ("3darray", "columnSpacing"),
    ("document.list", "active"),
    ("document.list", "drawings"),
    ("document.list", "title"),
    ("document.list", "entities"),
    ("qselect", "append"),
    ("qselect", "exclude"),
    ("zoom", "mode"),
    ("zoom", "dynamic"),
    ("vports.named", "name"),
    ("vports.named", "layout"),
    ("texttofront.leaders", "moved"),
    ("sunproperties", "date"),
    ("sunproperties", "time"),
    ("lightbake", "clear"),
    ("layerstate.list", "name"),
    ("layerstate.list", "layers"),
    ("layerstate.list", "restore"),
    ("layerstate.list", "offOthers"),
    ("layout.new", "name"),
    ("mvsetup", "create"),
    ("mvsetup", "model"),
    ("vports.2", "arrangement"),
    ("plot", "printer"),
    ("plot", "layout"),
    ("plot", "paper"),
    ("plot", "landscape"),
    ("stylesmanager", "get"),
    ("stylesmanager", "new"),
    ("stylesmanager", "set"),
    ("stylesmanager", "delete"),
    ("stylesmanager", "load"),
    ("stylesmanager", "save"),
    ("stylesmanager", "name"),
    ("dimstyle.list", "name"),
    ("mleaderstyle", "leaderType"),
    ("osnap", "modes"),
    ("help", "command"),
    ("help", "alias"),
    ("markups", "markups"),
    ("markups", "id"),
    ("markups", "status"),
    ("markups", "kind"),
    ("markups", "author"),
    ("markups", "text"),
    ("standards", "clear"),
    ("constraints.inspect", "parameters"),
    ("constraints.inspect", "settings"),
    ("lisp.dialog", "dialog"),
    ("3dosnap", "modes"),
    ("3dosnap", "midpoint"),
    ("3dosnap", "perpendicular"),
    ("plugins", "enabled"),
    ("plugins", "grants"),
    ("plugins", "files"),
    ("plugins", "network"),
    ("document.close", "saveAs"),
    ("aliasedit", "text"),
    ("aliasedit", "alias"),
    ("ucsman", "restore"),
    ("ucsman", "delete"),
    ("ucsman", "rename"),
    ("ucsman", "name"),
    ("qselect.info", "applyTo"),
    ("qselect.info", "type"),
    ("navswheel", "wedges"),
    ("navswheel", "label"),
    ("navswheel", "command"),
    ("xref", "xrefs"),
    ("xref", "name"),
    ("xref", "foundAt"),
    ("xref", "type"),
    ("xref", "status"),
    ("xref", "Unloaded"),
    ("texttofront.all", "moved"),
    ("materials", "materials"),
    ("materials", "id"),
    ("materials", "name"),
    ("materials", "current"),
    ("part.new", "name"),
    ("tour.rooms", "tour"),
    ("tour.rooms", "speed"),
    ("tour.rooms", "look"),
    ("tour.rooms", "pause"),
    ("lsp", "kind"),
    ("spell", "word"),
    ("spell", "cell"),
    ("spell", "misspelled"),
    ("reverse", "polylines"),
    ("reverse", "splines"),
    ("layer.filter.list", "name"),
    ("layer.filter.list", "type"),
    ("layer.filter.list", "parent"),
    ("layer.filter.list", "rules"),
    ("layer.filter.list", "members"),
    ("layer.unreconciled", "layers"),
    ("layer.unreconciled", "layereval"),
    ("layer.unreconciled", "evaluated"),
    ("vports.3", "arrangement"),
    ("-plot", "device"),
    ("-plot", "plotArea"),
    ("-plot", "view"),
    ("-plot", "paper"),
    ("preview", "layout"),
    ("preview", "paper"),
    ("preview", "landscape"),
    ("preview", "fit"),
    ("preview", "lineweights"),
    ("linetype", "current"),
    ("linetype", "load"),
    ("linetype", "wildcards"),
    ("count", "block"),
    ("count", "model"),
    ("count", "all"),
    ("isodraft", "plane"),
    ("dsettings", "snapunit"),
    ("dsettings", "gridunit"),
    ("dsettings", "gridmajor"),
    ("render", "preset"),
    ("render", "output"),
    ("reinit", "text"),
    ("undo", "control"),
    ("layout.set", "name"),
    ("3dwalk", "mode"),
    ("dimoverride", "text"),
    ("overkill", "layer"),
    ("overkill", "linetype"),
    ("overkill", "lineweight"),
    ("-xref", "bind"),
    ("-xref", "detach"),
    ("-xref", "unload"),
    ("-xref", "reload"),
    ("-xref", "attach"),
    ("hatchtoback", "moved"),
    ("visualstyles", "toneMap"),
    ("part.list", "parts"),
    ("agent.journal", "clear"),
    ("exportpdf", "layout"),
    ("exportpdf", "paper"),
    ("exportpdf", "landscape"),
    ("exportpdf", "fit"),
    ("publish", "format"),
    ("publish", "pages"),
    ("publish", "data"),
    ("publish", "drawing"),
    ("units", "insunits"),
    ("isoplane", "plane"),
    ("laytrans", "standards"),
    ("laytrans", "save"),
    ("laytrans", "load"),
    ("document.bytes", "format"),
    ("document.bytes", "dwg"),
    ("document.bytes", "data"),
    ("view", "window"),
    ("view", "delete"),
    ("view", "settings"),
    ("view", "names"),
    ("3dfly", "mode"),
    ("qvdrawing", "thumbnails"),
    ("mlstyle", "name"),
    ("mlstyle", "elements"),
    ("mlstyle", "color"),
    ("mlstyle", "linetype"),
    ("mlstyle", "fill"),
    ("dataextraction", "blocks"),
    ("dataextraction", "wildcards"),
    ("dataextraction", "tags"),
    ("dataextraction", "Color"),
    ("dimregen", "updated"),
    ("dimregen", "regenerated"),
    ("tour.list", "views"),
    ("tour.list", "tours"),
    ("tour.list", "name"),
    ("tour.list", "stops"),
    ("tour.list", "speed"),
    ("tour.list", "look"),
    ("tour.list", "pause"),
    ("showurls", "links"),
    ("showurls", "url"),
    ("showurls", "target"),
    ("revert", "reverted"),
    ("image", "images"),
    ("image", "name"),
    ("image", "found"),
    ("image", "references"),
    ("chspace", "viewport"),
    ("plottermanager", "get"),
    ("plottermanager", "new"),
    ("plottermanager", "set"),
    ("plottermanager", "delete"),
    ("plottermanager", "load"),
    ("plottermanager", "save"),
    ("plottermanager", "name"),
    ("ucs", "z"),
    ("ucs", "object"),
    ("ucs", "set"),
    ("colorbook", "text"),
    ("parameters", "name"),
    ("parameters", "expr"),
    ("parameters", "delete"),
];

/// Battle test 7's naninf7.py: each command and parameter that took 'nan', 'inf' or '-inf' as text
/// (outside the job, BIM and MEP lane).
const NAN_ACCEPTS: &[(&str, &str)] = &[
    ("-vports", "join"),
    ("-vports", "left"),
    ("3darray", "y"),
    ("chprop", "lineweight"),
    ("dataextraction", "Lineweight"),
    ("dwgprops", "value"),
    ("eattext", "Lineweight"),
    ("exportpdf", "lineweights"),
    ("hatch", "origin"),
    ("hatchedit", "origin"),
    ("mvsetup", "cols"),
    ("overkill", "lineweight"),
    ("parameters", "value"),
    ("plugins", "plugin"),
    ("preview", "lineweights"),
    ("properties.set", "lineweight"),
    ("publish", "lineweights"),
    ("qselect", "value"),
    ("scalelistedit", "ratio"),
    ("setbylayer", "lineweight"),
    ("stylesmanager", "join"),
    ("stylesmanager", "lineweight"),
    ("tablestyle", "margin"),
    ("tour.list", "duration"),
    ("ucs", "origin"),
    ("ucs", "z"),
    ("view", "origin"),
    ("view", "z"),
    ("xplode", "lineweight"),
];

#[test]
fn objects_where_a_value_goes_are_refused_by_name() {
    let mut s = Session::new();
    for (cmd, key) in OBJECT_ACCEPTS {
        let e = refusal(&mut s, cmd, json!({ *key: {"a": [1]} }));
        assert!(e.contains(&format!("`{key}`")), "{cmd} {key}: {e}");
    }
    // What the refusal says: the parameter and what it takes.
    let e = refusal(&mut s, "zoom", json!({"mode": {"a": [1]}}));
    assert!(e.contains("`mode` must be a single value, not an object"), "{e}");
    let e = refusal(&mut s, "mlstyle", json!({"name": "M", "elements": {"a": [1]}}));
    assert!(e.contains("`elements` must be a list, not an object"), "{e}");
    let e = refusal(&mut s, "zoom", json!({"mode": ["in"]}));
    assert!(e.contains("`mode` must be a single value, not a list"), "{e}");
    // An object whose doc shows its fields gets them checked.
    let e = refusal(&mut s, "renderenvironment", json!({"background": {"a": [1]}}));
    assert!(e.contains("`background` has no field `a`; it takes `kind`, `color`"), "{e}");
    let e = refusal(&mut s, "qvdrawing", json!({"thumbnails": {"wide": 3}}));
    assert!(e.contains("`thumbnails` has no field `wide`; it takes `width`, `height`"), "{e}");
    // `cellStyle: {name, …} (see tablestyle.cellstyle)`: that command's parameters.
    let e = refusal(&mut s, "tablestyle", json!({"name": "T", "cellStyle": {"a": 1}}));
    assert!(e.contains("`cellStyle` has no field `a`"), "{e}");
    // The right shapes still run.
    s.execute_agent("qvdrawing", &json!({"thumbnails": {"width": 40, "height": 30}})).unwrap();
    s.execute_agent("renderenvironment", &json!({"background": {"kind": "solid", "color": [0.2, 0.2, 0.2]}})).unwrap();
    s.execute_agent("tablestyle", &json!({"name": "T", "cellStyle": {"name": "Big", "textHeight": 0.5}})).unwrap();
    s.execute_agent("zoom", &json!({"mode": "extents"})).unwrap();
    // `attribs?: {TAG: value}` is a map: any tag is a key.
    s.execute_agent("symbol", &json!({"name": "pl_water_closet", "at": [10, 0], "attribs": {"MARK": "WC-3"}})).unwrap();
}

#[test]
fn nan_and_inf_text_is_refused_where_a_number_goes() {
    let mut s = Session::new();
    for (cmd, key) in NAN_ACCEPTS {
        for v in ["nan", "inf", "-inf"] {
            let e = refusal(&mut s, cmd, json!({ *key: v }));
            assert!(e.contains(&format!("`{key}`")), "{cmd} {key} {v}: {e}");
        }
    }
    let e = refusal(&mut s, "tablestyle", json!({"name": "T", "margin": "nan"}));
    assert!(e.contains("`margin` must be a finite number, not \"nan\""), "{e}");
    let e = refusal(&mut s, "ucs", json!({"option": "origin", "origin": "inf"}));
    assert!(e.contains("`origin` must be a finite number"), "{e}");
    let e = refusal(&mut s, "preview", json!({"lineweights": "nan"}));
    assert!(e.contains("`lineweights` must be true or false"), "{e}");
    // A name that happens to spell it is still a name.
    s.execute_agent("layer.new", &json!({"name": "nan"})).unwrap();
    // qselect: a value with no property to compare it with selected everything.
    let e = refusal(&mut s, "qselect", json!({"value": 5}));
    assert!(e.contains("`value` needs `property`"), "{e}");
}

#[test]
fn setvar_says_a_non_finite_value_is_not_a_number() {
    let mut s = Session::new();
    for v in [json!("nan"), json!("inf"), json!("-inf")] {
        let e = s.execute("setvar", &json!({"name": "THICKNESS", "value": v})).unwrap_err().to_string();
        assert!(e.contains("THICKNESS must be a finite number."), "{v}: {e}");
        assert!(!e.contains("or more"), "{e}");
    }
    let e = s.execute("setvar", &json!({"name": "TEXTSIZE", "value": "inf"})).unwrap_err().to_string();
    assert!(e.contains("TEXTSIZE must be a finite number."), "{e}");
    // Out of range is still out of range.
    let e = s.execute("setvar", &json!({"name": "TEXTSIZE", "value": -1})).unwrap_err().to_string();
    assert!(e.contains("TEXTSIZE must be greater than 0."), "{e}");
}

#[test]
fn refusals_offer_each_parameter_once_and_no_doc_words() {
    for spec in command_specs() {
        let offered = offered_params(spec);
        for (i, n) in offered.iter().enumerate() {
            assert!(!offered[..i].contains(n), "{} offers `{n}` twice: {offered:?}", spec.id);
            // `attdef {default?}` takes one called that.
            let doc_word = matches!(*n, "default" | "null" | "true" | "false" | "none" | "e" | "g") && !params_doc(spec).contains(&format!("{n}?"));
            assert!(!doc_word, "{} offers `{n}`: {offered:?}", spec.id);
        }
    }
    let offers = |cmd: &str| offered_params(find_command(cmd).unwrap());
    let has_none = |cmd: &str, words: &[&str]| {
        let o = offers(cmd);
        for w in words {
            assert!(!o.contains(w), "{cmd} offers `{w}`: {o:?}");
        }
    };
    has_none("purge", &["n", "blocks", "layers"]);
    has_none("-vports", &["MVIEW", "save", "left"]);
    has_none("renderenvironment", &["PNG", "JPEG", "null", "kind"]);
    has_none("tablestyle.cellstyle", &["Title", "Header", "TL"]);
    has_none("sectionplane", &["true"]);
    has_none("jpgout", &["default"]);
    has_none("saveas", &["DWG", "DXF"]);
    has_none("ucs", &["set", "z", "world"]);
    has_none("text", &["L"]);
    has_none("undo", &["all", "auto"]);
    has_none("zoom", &["dynamic", "realtime"]);
    has_none("3dosnap", &["midpoint", "perpendicular"]);
    has_none("camera", &["y"]);
    has_none("fourd.import", &["CSV", "TSV", "XLSX"]);
    for (cmd, real) in [("purge", "type"), ("undo", "mode"), ("undo", "on"), ("zoom", "center"), ("ucs", "origin"), ("-vports", "join")] {
        assert!(offers(cmd).contains(&real), "{cmd} offers {:?}", offers(cmd));
    }
    let e = refusal(&mut Session::new(), "purge", json!({"layers": 12345}));
    assert!(e.contains("`layers` is not a parameter of purge; it takes `type`, `names`"), "{e}");
    assert_eq!(e.matches("`at`").count(), 0, "{e}");
    let e = refusal(&mut Session::new(), "dimangular", json!({"bogus": 1}));
    assert_eq!(e.matches("`at`").count(), 1, "{e}");
}

#[test]
fn parameters_named_in_notes_and_after_the_braces_still_count() {
    for (cmd, key) in [
        ("colorbook", "file"),
        ("colorbook", "text"),
        ("colorbook", "book"),
        ("colorbook", "color"),
        ("circle.cd", "p1"),
        ("circle.2p", "p3"),
        ("grip.stretch", "base"),
        ("tolerance.edit", "symbol"),
        ("tolerance.edit", "datumId"),
        ("filter", "save"),
        ("filter", "name"),
        ("filter", "list"),
        ("filter", "delete"),
        ("vpoint", "fit"),
        ("revcloud", "arcLength"),
        ("attdef", "default"),
        ("publishlist", "path"),
        ("publishlist", "sheets"),
        ("dim", "base"),
        ("dim", "points"),
        ("-group", "from"),
        ("import", "at"),
        ("import", "scale"),
        ("rhairun", "timeoutMs"),
        ("thicken", "keep"),
        ("options", "OSMODE"),
        ("shape", "list"),
        ("-plot", "device"),
        // `…the parameters of plot`: plot's own.
        ("-plot", "printer"),
        ("light.set", "intensity"),
        ("array", "rows"),
    ] {
        assert!(known(cmd, key), "`{key}` of {cmd}: {}", params_doc(find_command(cmd).unwrap()));
    }
}

#[test]
fn values_results_and_fields_given_as_parameters_are_refused() {
    for (cmd, key) in [
        // A value of a choice (`space?: all (default)|current|model|<layout>`).
        ("countlist", "current"),
        ("qselect", "append"),
        ("zoom", "dynamic"),
        ("ucs", "z"),
        ("-xref", "attach"),
        // A field of a value, given at the top.
        ("3darray", "count"),
        ("scalelistedit", "ratio"),
        ("renderenvironment", "kind"),
        ("filter", "property"),
        // A key of the result.
        ("drawing.summary", "extents"),
        ("checkexchange", "kept"),
        ("plugins", "plugin"),
        ("-plot", "bytes"),
    ] {
        assert!(!known(cmd, key), "`{key}` is not a parameter of {cmd}");
    }
    // `{} → {moved}`: it takes nothing.
    let mut s = Session::new();
    let e = refusal(&mut s, "hatchtoback", json!({"moved": 1}));
    assert!(e.contains("hatchtoback takes no parameters (got `moved`)"), "{e}");
    for cmd in ["camera.list", "revert", "texttofront.all", "layout.list"] {
        assert!(!known(cmd, "name"), "{cmd} takes nothing");
    }
    // The agent rails' own and the objects still go to every command.
    assert!(known("hatchtoback", "handles") && known("hatchtoback", "confirmed"));
    s.execute_agent("hatchtoback", &json!({})).unwrap();
}

#[test]
fn the_job_lane_keeps_its_looser_reading() {
    // The job, BIM and MEP commands' callers are checked in that lane: the guard takes what it
    // took before for them (a value word, an open doc), and checks no types of theirs here.
    assert!(in_job_lane("bim.bom") && in_job_lane("mep.qty") && in_job_lane("rooms.build") && !in_job_lane("plot"));
    assert!(known("bim.bom", "path"));
    assert!(takes_open_params(find_command("wallgen").unwrap()));
    assert!(unknown_params(find_command("mep.qty").unwrap(), &json!({"lb_per_sqft": "nan"})).is_empty());
}

#[test]
fn what_a_command_reaches_still_follows_its_file_values() {
    // `colorMap?: path`, `(file | text: …)`, `report?: path`: the doc reader no longer takes those
    // as names, the plug-in classification still sees the files.
    for cmd in ["material.new", "colorbook", "checkstandards", "laytrans", "wallgen", "-style"] {
        assert_eq!(format!("{:?}", find_command(cmd).unwrap().reach()), "Files", "{cmd}");
    }
    for cmd in ["select", "hatchtoback", "zoom"] {
        assert_eq!(format!("{:?}", find_command(cmd).unwrap().reach()), "Drawing", "{cmd}");
    }
}
