//! Q6361: a legend/notes sheet is no plan, whatever its title or number says. A made-up set: the
//! generator's first floor plan (no switches drawn on it) and a hand-drawn electrical sheet of
//! symbol legends, abbreviations and general notes whose title block reads as a plan. Its legend
//! samples (`S`, `F`, `OS`) are never devices, and its general note on how to circuit exit signs
//! builds no exit sign.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Circle, Common, Drawing, EntityKind, HAlign, Line, Space, Text, VAlign};
use cadaddy_engine::geom::{Bounds2, Vec2, Vec3};
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

fn pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn text(d: &mut Drawing, at: [f64; 2], h: f64, s: &str) {
    let t = Text {
        insert: Vec3::new(at[0], at[1], 0.0),
        align_pt: None,
        height: h,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    };
    d.add(&Space::Model, Common { layer: "TEXT".into(), ..Common::default() }, EntityKind::Text(t)).expect("text");
}

fn line(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
    let l = Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) };
    d.add(&Space::Model, Common { layer: "OBJECT".into(), ..Common::default() }, EntityKind::Line(l)).expect("line");
}

fn circle(d: &mut Drawing, c: [f64; 2], r: f64) {
    let k = Circle { center: Vec3::new(c[0], c[1], 0.0), radius: r };
    d.add(&Space::Model, Common { layer: "OBJECT".into(), ..Common::default() }, EntityKind::Circle(k)).expect("circle");
}

fn rect(d: &mut Drawing, x0: f64, y0: f64, x1: f64, y1: f64) {
    for (a, b) in [([x0, y0], [x1, y0]), ([x1, y0], [x1, y1]), ([x1, y1], [x0, y1]), ([x0, y1], [x0, y0])] {
        line(d, a, b);
    }
}

/// A table at (`x`, `top`): its heading, a SYMBOL | DESCRIPTION (or ABBR | MEANING) header and a
/// ruled row per entry, each row's sample symbol (a small circle lettered with its code) in the
/// first column when `symbols`.
fn table(d: &mut Drawing, x: f64, top: f64, heading: &str, heads: [&str; 2], rows: &[(&str, &str)], symbols: bool) {
    let (row, first, width) = (0.3, 0.8, 5.2);
    text(d, [x, top + 0.25], 0.18, heading);
    text(d, [x + 0.08, top - 0.22], 0.1, heads[0]);
    text(d, [x + first + 0.1, top - 0.22], 0.1, heads[1]);
    let bottom = top - row * (rows.len() + 1) as f64;
    rect(d, x, bottom, x + width, top);
    line(d, [x + first, bottom], [x + first, top]);
    for (i, (code, meaning)) in rows.iter().enumerate() {
        let y = top - row * (i + 1) as f64;
        line(d, [x, y], [x + width, y]);
        let mid = y - row / 2.0;
        if symbols {
            circle(d, [x + 0.4, mid], 0.1);
            text(d, [x + 0.34, mid - 0.04], 0.08, code);
        } else {
            text(d, [x + 0.1, mid - 0.05], 0.1, code);
        }
        text(d, [x + first + 0.1, mid - 0.05], 0.1, meaning);
    }
}

/// An electrical legend and notes sheet, 36" × 24": symbol legends, abbreviations and general
/// notes (one saying how to circuit exit signs), its title block reading as a plan.
fn legend_sheet() -> Vec<u8> {
    let mut d = Drawing::default();
    rect(&mut d, 0.5, 0.5, 35.5, 23.5);
    let switches: Vec<(&str, &str)> = vec![
        ("S", "SINGLE POLE TOGGLE SWITCH, 20A, MOUNT AT 48 IN TO TOP"),
        ("S3", "THREE-WAY SWITCH, 20A, MOUNT AT 48 IN TO TOP"),
        ("S4", "FOUR-WAY SWITCH, 20A, MOUNT AT 48 IN TO TOP"),
        ("SD", "DIMMER SWITCH, MATCH THE LAMP TYPE OF THE FIXTURE"),
        ("SK", "KEY OPERATED SWITCH, KEYED ALIKE WITH THE OWNER"),
        ("OS", "CEILING MOUNTED OCCUPANCY SENSOR, DUAL TECHNOLOGY"),
        ("VS", "WALL MOUNTED VACANCY SENSOR SWITCH, MANUAL ON"),
        ("F", "FAN SPEED CONTROL SWITCH, MATCH THE FAN MOTOR"),
        ("T", "LINE VOLTAGE THERMOSTAT, BY THE MECHANICAL TRADE"),
        ("J", "JUNCTION BOX, 4 IN SQUARE WITH A BLANK COVER"),
        ("D", "DUPLEX RECEPTACLE, 20A, MOUNT AT 18 IN TO CENTER"),
        ("G", "GFCI DUPLEX RECEPTACLE, 20A, WEATHER RESISTANT"),
        ("Q", "QUADRUPLEX RECEPTACLE, 20A, MOUNT AT 18 IN TO CENTER"),
        ("FB", "FLUSH FLOOR BOX WITH POWER AND DATA, COORDINATE"),
        ("DT", "DATA OUTLET, TWO CAT 6 JACKS, MOUNT AT 18 IN"),
        ("TV", "TELEVISION OUTLET WITH POWER AND DATA ABOVE"),
        ("PB", "PUSH BUTTON STATION FOR THE DOOR OPERATOR"),
        ("CR", "CARD READER, BOX BY THE ELECTRICAL TRADE"),
    ];
    table(&mut d, 1.0, 21.5, "ELECTRICAL SYMBOL LEGEND", ["SYMBOL", "DESCRIPTION"], &switches, true);
    let alarm: Vec<(&str, &str)> = vec![
        ("FS", "FIRE ALARM PULL STATION, MOUNT AT 48 IN"),
        ("FH", "FIRE ALARM HORN STROBE, MOUNT AT 80 IN"),
        ("FV", "FIRE ALARM STROBE ONLY, CEILING MOUNTED"),
        ("SM", "SMOKE DETECTOR, CEILING MOUNTED"),
        ("HD", "HEAT DETECTOR, CEILING MOUNTED"),
        ("DD", "DUCT SMOKE DETECTOR, BY THE MECHANICAL TRADE"),
        ("FA", "FIRE ALARM ANNUNCIATOR PANEL AT THE ENTRY"),
        ("MH", "MAGNETIC DOOR HOLDER, WALL MOUNTED"),
    ];
    table(&mut d, 7.5, 21.5, "FIRE ALARM SYMBOLS", ["SYMBOL", "DESCRIPTION"], &alarm, true);
    let abbr: Vec<(&str, &str)> = vec![
        ("AFF", "ABOVE FINISHED FLOOR"),
        ("EC", "EMPTY CONDUIT WITH A PULL STRING"),
        ("EMT", "ELECTRICAL METALLIC TUBING"),
        ("GFI", "GROUND FAULT CIRCUIT INTERRUPTER"),
        ("NIC", "NOT IN CONTRACT, BY THE OWNER"),
        ("TYP", "TYPICAL OF THE LIKE LOCATIONS"),
        ("UNO", "UNLESS NOTED OTHERWISE ON PLANS"),
        ("WP", "WEATHERPROOF, IN USE COVER"),
    ];
    table(&mut d, 7.5, 17.5, "ABBREVIATIONS", ["ABBR", "MEANING"], &abbr, false);
    text(&mut d, [14.0, 21.75], 0.18, "GENERAL NOTES");
    let notes = [
        "1. ALL WORK SHALL COMPLY WITH THE ADOPTED ELECTRICAL CODE AND THE LOCAL AMENDMENTS.",
        "2. COORDINATE THE LOCATIONS OF ALL DEVICES WITH THE CASEWORK AND THE FURNITURE LAYOUT.",
        "3. CIRCUIT EXIT SIGNS TO THE LOCAL LIGHTING CIRCUIT AHEAD OF ANY SWITCH.",
        "4. MOUNTING HEIGHTS ARE TO THE CENTER OF THE DEVICE UNLESS NOTED OTHERWISE.",
        "5. VERIFY THE EQUIPMENT CONNECTIONS WITH THE APPROVED SUBMITTALS BEFORE ROUGH IN.",
        "6. SEAL ALL PENETRATIONS OF RATED ASSEMBLIES WITH A LISTED FIRESTOP SYSTEM.",
        "7. LABEL EACH DEVICE PLATE WITH ITS PANEL AND CIRCUIT NUMBER.",
        "8. PROVIDE A GREEN INSULATED EQUIPMENT GROUND IN EVERY BRANCH CIRCUIT.",
    ];
    for (i, n) in notes.iter().enumerate() {
        text(&mut d, [14.0, 21.2 - 0.35 * i as f64], 0.12, n);
    }
    text(&mut d, [14.0, 17.75], 0.18, "SPECIFICATIONS");
    for i in 0..24 {
        text(
            &mut d,
            [14.0, 17.2 - 0.3 * f64::from(i)],
            0.1,
            "PROVIDE MATERIALS NEW, LISTED AND LABELED FOR THE PURPOSE, INSTALLED PER THE MAKER'S PRINTED INSTRUCTIONS.",
        );
    }
    // The title block, lower right: its title reads as a plan.
    rect(&mut d, 30.5, 0.5, 35.5, 6.0);
    text(&mut d, [30.8, 5.4], 0.12, "DRAWING TITLE");
    text(&mut d, [30.8, 4.9], 0.2, "ELECTRICAL PLAN");
    text(&mut d, [30.8, 2.4], 0.12, "SHEET NO.");
    text(&mut d, [30.8, 1.2], 0.6, "E-001");
    let o = cadaddy_io::pdf::PdfOptions {
        paper_mm: Some((36.0 * 25.4, 24.0 * 25.4)),
        landscape: Some(true),
        fit: Some(true),
        window: Some(Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(36.0, 24.0))),
        margins: Some([0.0; 4]),
        title: "Legends".into(),
        stamp: Some(false),
        ..cadaddy_io::pdf::PdfOptions::default()
    };
    cadaddy_io::pdf::pdf(&d, &Space::Model, &o).expect("plot")
}

/// The generator's first floor plan: walls and rooms, no switches.
fn floor_plan() -> (Vec<u8>, String, String) {
    let (t, st) = cadaddy_mepfix::hotel_planned(7, HotelSize::Small);
    let kinds: Vec<(String, cadaddy_mepfix::layout::Kind)> =
        cadaddy_mepfix::layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let sh = t.sheets.iter().find(|s| s.number == "A-101").expect("A-101 in the set");
    let (_, kind) = kinds.iter().find(|(k, _)| k == "A-101").expect("sheet kind");
    let (d, _, _) = cadaddy_mepfix::draw::draw(&t, &st, sh, kind);
    let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("plot");
    (pdf, format!("{} {}", sh.number, sh.title), String::new())
}

fn job_state(s: &Session) -> Value {
    let agent: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state");
    agent["guided_job"].clone()
}

#[test]
fn a_legend_sheet_titled_as_a_plan_builds_no_devices_and_a_wiring_note_no_exit_sign() {
    let _g = serial();
    pdf_reader();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_legendsheet_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages = vec![floor_plan(), (legend_sheet(), "E-001 Legends".to_string(), String::new())];
    std::fs::write(dir.join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    let job = job_state(&s);
    // 1. The legend sheet is read as legends and notes, not as a plan.
    let sheets = job["index"]["sheets"].as_array().cloned().unwrap_or_default();
    let legend = sheets.iter().find(|sh| sh["number"] == "E-001").cloned().unwrap_or(Value::Null);
    assert_eq!(legend["type"], json!("schedule"), "E-001 typed as legends and notes: {legend}");
    let plan = sheets.iter().find(|sh| sh["number"] == "A-101").cloned().unwrap_or(Value::Null);
    assert_eq!(plan["type"], json!("plan"), "the floor plan stays a plan: {plan}");
    // 2. Its samples are no devices: no switch, sensor or `F` in the model.
    let spec = s.execute("bspec.get", &json!({})).unwrap_or(Value::Null);
    let tags = spec["spec"]["tags"].as_array().or_else(|| spec["tags"].as_array()).cloned().unwrap_or_default();
    let field = |t: &Value, k: &str| t["fields"][k]["value"].as_str().or_else(|| t["fields"][k].as_str()).unwrap_or("").to_string();
    let devices: Vec<&Value> = tags.iter().filter(|t| t["id"].as_str().is_some_and(|i| i.starts_with("device:"))).collect();
    let switches = devices.iter().filter(|t| t["kind"] == "switch").count();
    let samples: Vec<String> = devices.iter().map(|t| field(t, "type")).filter(|ty| ["S", "F", "OS"].contains(&ty.as_str())).collect();
    assert_eq!(switches, 0, "switches in a model whose plan draws none: {devices:?}");
    assert!(samples.is_empty(), "legend samples built as devices: {samples:?}");
    let from_legend: Vec<String> = devices.iter().map(|t| field(t, "sheet")).filter(|sh| sh.starts_with("E-001")).collect();
    assert!(from_legend.is_empty(), "devices read off the legend sheet: {from_legend:?}");
    // 3. The note on how to circuit exit signs calls for none.
    let stage = job["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "bim_build")).cloned().unwrap_or(Value::Null);
    let docs = stage["result"]["doc_objects"].clone();
    let exit_signs = docs["by_kind"]["exit_sign"].as_object().map_or(0, |m| m.values().filter_map(Value::as_u64).sum::<u64>());
    assert_eq!(exit_signs, 0, "an exit sign built from a wiring note: {docs}");
    assert!(!docs["proposed"].to_string().to_ascii_uppercase().contains("EXIT SIGN"), "{docs}");
    assert!(
        docs["notes_calling_for_none"]["exit_sign"].as_str().is_some_and(|n| n.contains("CIRCUIT EXIT SIGNS")),
        "the wiring note is read and kept as one calling for none: {docs} ({})",
        st.to_string().chars().take(300).collect::<String>()
    );
}
