//! E-LUM-SCHEDULE (Q6179) on a made-up job ("Project A"): Validate reads the light fixture
//! schedule again from the sheets and the lighting submittals again from the job's folders, and
//! checks the luminaires and the build's lighting RFIs against them.
//!
//! Rooms 11 to 13. The schedule (sheet X-601, drawn with row lines, columns TYPE, DESCRIPTION,
//! MANUFACTURER, CATALOG NUMBER, LAMP, INPUT VA, VOLTS, MOUNTING) lists T1, T2, T3 and T5:
//! - T1 (two in 11): the model carries its row; the submittal's AT24L35-MVOLT is the row's
//!   AT-24-L40 series with other options. Passes; the build's maker RFI about it is `extra`.
//! - T2 (in 12): the model says 26 VA at 277 V where the row says 13 VA at 120 V (the build's
//!   stored schedule says what the model does: Validate doesn't read it). Fails, citing the row.
//! - T3 (in 12): its latest submittal revision gives another maker's product. Fails, matched by
//!   the build's catalog RFI.
//! - T5: scheduled, on no plan, its only submittal rejected. Fails twice, no RFI: `missing`.
//!   (A later revision of T1 sent back doesn't replace T1's.)
//! - T9 (in 13): on the plan, no row. The luminaire is unsure; the type fails, matched by the
//!   build's RFI.
//!
//! A second set puts the schedule on a page of the job's PDFs the drawing doesn't hold: two halves
//! side by side, no row lines, another title and column order.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Source};
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

const RULE: &str = "E-LUM-SCHEDULE";
/// The plan's origin on the drawing (inches).
const OX: f64 = 1200.0;
const OY: f64 = 1000.0;
/// One sheet: 36" × 24" at 1/8" = 1'-0", in drawing inches.
const SW: f64 = 3456.0;
const SH: f64 = 2304.0;
/// The schedule sheet, beside the plan.
const X601: f64 = 6000.0;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// A building point (feet) on the plan.
fn at(p: [f64; 2]) -> [f64; 2] {
    [OX + p[0] * 12.0, OY + p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let v = [at(a), at([b[0], a[1]]), at(b), at([a[0], b[1]])];
    s.execute("pline", &json!({"vertices": v, "closed": true})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// The fixture schedule drawn on X-601 with its grid: a title, a header row and a row per type.
fn draw_schedule(s: &mut Session) {
    layer(s, "E-SCHED");
    let xs = [6100.0, 6130.0, 6230.0, 6300.0, 6400.0, 6440.0, 6480.0, 6520.0, 6600.0];
    let headers = ["TYPE", "DESCRIPTION", "MANUFACTURER", "CATALOG NUMBER", "LAMP", "INPUT VA", "VOLTS", "MOUNTING"];
    let rows: [[&str; 8]; 5] = [
        ["T1", "2X4 TROFFER", "ACME", "AT-24-L40", "LED", "38", "277", "RECESSED"],
        ["T2", "WALL SCONCE", "ACME", "WS-9", "LED", "13", "120", "WALL"],
        ["T3", "DOWNLIGHT", "ACME", "DL-6-L15", "LED", "15", "120", "RECESSED"],
        ["T5", "PENDANT", "ACME", "PD-2", "LED", "40", "120", "PENDANT"],
        ["", "NOTE: NO SUBSTITUTIONS WITHOUT WRITTEN APPROVAL", "", "", "", "", "", ""],
    ];
    text(s, [6300.0, 2000.0], "LIGHT FIXTURE SCHEDULE", 6.0);
    let (top, header_bottom, row_h) = (1990.0, 1972.0, 15.0);
    let bottom = header_bottom - row_h * rows.len() as f64;
    for x in xs {
        s.execute("line", &json!({"points": [[x, top], [x, bottom]]})).unwrap();
    }
    for line_y in std::iter::once(top).chain((0..=rows.len()).map(|i| header_bottom - row_h * i as f64)) {
        s.execute("line", &json!({"points": [[xs[0], line_y], [xs[8], line_y]]})).unwrap();
    }
    for (i, h) in headers.iter().enumerate() {
        text(s, [xs[i] + 2.0, 1980.0], h, 4.0);
    }
    for (r, row) in rows.iter().enumerate() {
        let ty = header_bottom - row_h * r as f64 - 10.0;
        for (i, v) in row.iter().enumerate().filter(|(_, v)| !v.is_empty()) {
            text(s, [xs[i] + 2.0, ty], v, 4.0);
        }
    }
}

/// Device blocks at building points (feet), each with a TYPE attribute.
fn place(s: Session, devs: &[([f64; 2], &str)]) -> (Session, Vec<String>) {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key("DEV") {
        let mut b = Block::new("DEV");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-4.0, 0.0, 0.0), b: Vec3::new(4.0, 0.0, 0.0) })));
        d.blocks.insert("DEV".into(), Arc::new(b));
    }
    let mut hs = Vec::new();
    for (p, ty) in devs {
        let attribs = vec![Attrib {
            tag: "TYPE".into(),
            text: Text { value: ty.to_string(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        }];
        let q = at(*p);
        let ins = Insert {
            block: "DEV".into(),
            insert: Vec3::new(q[0], q[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "E-LITE".into();
        d.model.push(e);
        hs.push(h.hex());
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    (s2, hs)
}

/// Store properties on objects, as the job's lighting step does.
fn set_props(s: Session, sets: &[(&str, &str, &str)]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    for (handle, key, value) in sets {
        let h = cadaddy_engine::doc::Handle::parse_hex(handle).unwrap();
        d.modify_entity(h, |e| {
            let mut store = props::read(&e.common.xdata);
            store.set(key, Prop::new(json!(value), Source::computed()));
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        })
        .unwrap();
    }
    let mut s2 = Session::empty();
    s2.open_drawing(d, "Sample Job", None);
    s2
}

fn rfi(id: &str, topic: &str, about: &str) -> Value {
    json!({
        "id": id, "kind": "conflict", "severity": "medium", "about": about, "issue": format!("{about}: the documents disagree"),
        "assumed": "the submittal", "status": "open", "topic": format!("lighting: {topic}"),
        "sides": [{"document": "X-601 LIGHT FIXTURE SCHEDULE type", "says": "one thing"}, {"document": "Lighting Package.txt p.1", "says": "another"}]
    })
}

fn spec(drawn: bool) -> Value {
    let lum = json!({"ceiling": true, "aff_ft": 9.0, "size_ft": [0.5, 0.5, 0.3]});
    let sym = |mark: &str, p: [f64; 2]| json!({"sheet": "A-101", "level": "L1", "kind": "luminaire", "mark": mark, "at": p});
    let mut sheets = vec![json!({
        "number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": [0.0, 0.0, SW, SH],
        "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}
    })];
    if drawn {
        sheets.push(
            json!({"number": "X-601", "title": "LIGHTING SCHEDULES", "discipline": "E", "source": "model", "rect": [X601, 0.0, X601 + SW, SH]}),
        );
    }
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [OX, OY], "region": [0, 0, SW, SH]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "E-LITE", "role": "device"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Exterior", "thickness_in": 6}, {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}],
        "rooms": [{"name": "Office", "number": "11"}, {"name": "Office", "number": "12"}, {"name": "Storage", "number": "13"}],
        "devices": {"by_type": {"T1": lum, "T2": lum, "T3": lum, "T9": lum}},
        "sheets": sheets,
        "prints": {"version": 1, "symbols": [sym("T1", [5.0, 6.0]), sym("T1", [8.0, 6.0]), sym("T2", [20.0, 6.0]), sym("T3", [22.0, 8.0]), sym("T9", [45.0, 6.0])]},
        // What the build stored of the schedule: Validate never reads it (T2 here agrees with the model).
        "mep": {"fixture_schedule": [{"tag": "T2", "manufacturer": "ACME", "model": "WS-9", "lamp": "LED 26VA", "va": 26.0, "voltage": "277", "source": "X-601 LIGHT FIXTURE SCHEDULE"}]},
        "rfis": [
            rfi("RFI-L1", "catalog number", "lighting type T3"),
            rfi("RFI-L2", "plan type with no schedule row", "lighting type T9"),
            rfi("RFI-L3", "maker", "lighting type T1"),
            rfi("RFI-L4", "CCT", "lighting type T2"),
            rfi("RFI-L5", "switch leg with no fixture", "room 11 leg c")
        ]
    })
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validatefixtures-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The made-up job's submittals, in revision folders as a project export keeps them; a pump
/// submittal that isn't lighting beside them.
fn submittals(root: &Path) {
    write(
        &root.join("Interior Lighting/Rev 1-0/Lighting Package.txt"),
        "--- page 1 of 3 ---\nType : T1\nManufacturer : ACME LIGHTING\nModel Number : AT24L35-MVOLT\n--- page 2 of 3 ---\nType : T2\nManufacturer : ACME LIGHTING\nModel Number : WS-9-LED\n--- page 3 of 3 ---\nType : T3\nManufacturer : ACME LIGHTING\nModel Number : DL-6-L15\n",
    );
    write(
        &root.join("Interior Lighting/Rev 2-0/Lighting Package.txt"),
        "--- page 1 of 1 ---\nType : T3\nManufacturer : OTHER CO\nModel Number : ZX-500\n",
    );
    // A later revision of T1 sent back: the earlier one stands.
    write(
        &root.join("Interior Lighting/Rev 3-0/Lighting Package.txt"),
        "--- page 1 of 1 ---\nType : T1\nManufacturer : OTHER CO\nModel Number : QQ-1\nREVISE AND RESUBMIT\n",
    );
    // T5's only submittal, sent back.
    write(
        &root.join("Pendants/Rev 1-0/Light Pendant.txt"),
        "--- page 1 of 1 ---\nType : T5\nManufacturer : ACME LIGHTING\nModel Number : PD-2\nREJECTED\n",
    );
    write(&root.join("Pumps/Rev 1-0/Pump Package.txt"), "--- page 1 of 1 ---\nType : P1\nManufacturer : PUMPCO\nModel Number : PC-100\n");
}

/// A PDF of letter pages, each its lines (`[x0, y0, x1, y1]` in points) and texts (`(x, y, size, text)`).
fn pdf(pages: &[(Vec<[f64; 4]>, Vec<(f64, f64, f64, &str)>)]) -> Vec<u8> {
    let mut objs: Vec<String> =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), String::new(), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
    let mut kids = Vec::new();
    for (lines, texts) in pages {
        let mut c = String::from("0 0 0 RG 0.5 w\n");
        for l in lines {
            c.push_str(&format!("{} {} m {} {} l S\n", l[0], l[1], l[2], l[3]));
        }
        for (x, y, size, t) in texts {
            let esc = t.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
            c.push_str(&format!("BT /F1 {size} Tf {x} {y} Td ({esc}) Tj ET\n"));
        }
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
        let content = objs.len();
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {content} 0 R >>"));
        kids.push(format!("{} 0 R", objs.len()));
    }
    objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len());
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The schedule as a page of the set: two halves side by side, each with its header (VOLTAGE,
/// MARK, MFR, MODEL, VA), column lines but no row lines, titled LUMINAIRE SCHEDULE.
fn schedule_page() -> Vec<u8> {
    let xs = [40.0, 90.0, 130.0, 190.0, 260.0, 300.0, 350.0, 390.0, 450.0, 520.0, 560.0];
    let lines: Vec<[f64; 4]> = xs.iter().map(|x| [*x, 700.0, *x, 560.0]).collect();
    let mut texts: Vec<(f64, f64, f64, &str)> = vec![(250.0, 712.0, 10.0, "LUMINAIRE SCHEDULE")];
    let header = ["VOLTAGE", "MARK", "MFR", "MODEL", "VA"];
    for (i, h) in header.iter().chain(header.iter()).enumerate() {
        texts.push((xs[i] + 2.0, 690.0, 6.0, h));
    }
    let left: [[&str; 5]; 3] = [["277", "T1", "ACME", "AT-24-L40", "38"], ["120", "T2", "ACME", "WS-9", "13"], ["", "", "", "", ""]];
    let right: [[&str; 5]; 3] = [["120", "T3", "ACME", "DL-6-L15", "15"], ["120", "T5", "ACME", "PD-2", "40"], ["", "", "", "", ""]];
    for (r, (l, rr)) in left.iter().zip(right.iter()).enumerate() {
        let y = 670.0 - 14.0 * r as f64;
        for (i, v) in l.iter().chain(rr.iter()).enumerate().filter(|(_, v)| !v.is_empty()) {
            texts.push((xs[i] + 2.0, y, 6.0, v));
        }
    }
    pdf(&[(lines, texts)])
}

/// The guided job: its submittals folder and, for the page set, its sheet index.
fn set_job(s: &mut Session, subs: &Path, pages: Option<&Path>) {
    let mut job = json!({
        "id": "made-up-job", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": "/made-up/prints",
        "stages": [], "audit": [], "question_seq": 0, "submittals": [subs.to_string_lossy()], "rules": []
    });
    if let Some(p) = pages {
        job["pdfs_path"] = json!(p.parent().unwrap().to_string_lossy());
        job["sheets"] = json!({"sheets": [
            {"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "type": "plan", "file": "set.pdf", "page": 1},
            {"number": "X-602", "title": "ELECTRICAL SCHEDULES", "discipline": "E", "type": "schedule", "file": p.to_string_lossy(), "page": 1}
        ]});
    }
    s.doc_mut().unwrap().agent = Some(json!({"guided_job": job}).to_string());
}

struct Job {
    s: Session,
    t1: [String; 2],
    t2: String,
    t3: String,
    t9: String,
    root: PathBuf,
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The job: `drawn` the schedule drawn on X-601 in the drawing, else on a page of the job's PDFs.
fn job(name: &str, drawn: bool, with_job: bool) -> Job {
    hooks();
    let root = dir(name);
    submittals(&root.join("subs"));
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [60.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [59.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    for x in [15.0, 30.0] {
        s.execute("line", &json!({"points": [at([x, 0.5]), at([x, 11.5])]})).unwrap();
    }
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, at([5.0, 2.0]), "11", 6.0);
    text(&mut s, at([20.0, 2.0]), "12", 6.0);
    text(&mut s, at([45.0, 2.0]), "13", 6.0);
    if drawn {
        draw_schedule(&mut s);
    }
    let (s, h) = place(s, &[([5.0, 6.0], "T1"), ([8.0, 6.0], "T1"), ([20.0, 6.0], "T2"), ([22.0, 8.0], "T3"), ([45.0, 6.0], "T9")]);
    let mut s = s;
    let r = s.execute("bspec.set", &json!({"spec": spec(drawn), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s.execute("bim.source", &json!({"handles": h, "status": "fixed", "source": "A-101", "kind": "luminaire"})).unwrap();
    let mut sets: Vec<(&str, &str, &str)> = Vec::new();
    for t1 in [&h[0], &h[1]] {
        sets.extend([
            (t1.as_str(), "basis_of_design", "ACME AT-24-L40"),
            (t1, "lamp", "LED"),
            (t1, "fixture_va", "38"),
            (t1, "voltage", "277"),
            (t1, "installation", "RECESSED"),
        ]);
    }
    sets.extend([
        (h[2].as_str(), "basis_of_design", "ACME WS-9"),
        (&h[2], "lamp", "LED"),
        (&h[2], "fixture_va", "26"),
        (&h[2], "voltage", "277"),
        (&h[2], "installation", "WALL"),
        (&h[3], "basis_of_design", "ACME DL-6-L15"),
        (&h[3], "lamp", "LED"),
        (&h[3], "fixture_va", "15"),
        (&h[3], "voltage", "120"),
        (&h[3], "installation", "RECESSED"),
    ]);
    let mut s = set_props(s, &sets);
    if with_job {
        let page = (!drawn).then(|| {
            let p = root.join("prints/X-602 Schedules.pdf");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, schedule_page()).unwrap();
            p
        });
        set_job(&mut s, &root.join("subs"), page.as_deref());
    }
    Job { s, t1: [h[0].clone(), h[1].clone()], t2: h[2].clone(), t3: h[3].clone(), t9: h[4].clone(), root }
}

fn screen(s: &mut Session) -> (Value, Vec<Value>) {
    let p = |c: Option<u64>| {
        let mut p = json!({"rules": [RULE], "all": true, "limit": 50});
        if let Some(c) = c {
            p["cursor"] = json!(c);
        }
        p
    };
    let r = s.execute("validate.prescreen", &p(None)).unwrap();
    let mut fs: Vec<Value> = r["findings"].as_array().cloned().unwrap_or_default();
    let mut cursor = r["next"]["cursor"].as_u64();
    while let Some(c) = cursor {
        let more = s.execute("validate.prescreen", &p(Some(c))).unwrap();
        fs.extend(more["findings"].as_array().cloned().unwrap_or_default());
        cursor = more["next"]["cursor"].as_u64();
    }
    assert!(fs.iter().all(|f| f["rule"] == RULE), "{r}");
    (r, fs)
}

fn of_handle<'a>(fs: &'a [Value], handle: &str) -> &'a Value {
    fs.iter()
        .find(|f| f["measured"]["check"] == "row" && f["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(handle)))
        .unwrap_or_else(|| panic!("no row finding for {handle}: {fs:#?}"))
}

fn of_type<'a>(fs: &'a [Value], ty: &str, check: &str, verdict: &str) -> &'a Value {
    fs.iter().find(|f| f["handle"].is_null() && f["tag"] == ty && f["measured"]["check"] == check && f["verdict"] == verdict).unwrap_or_else(|| {
        let have: Vec<String> =
            fs.iter().filter(|f| f["tag"] == ty).map(|f| format!("{} {}: {}", f["measured"]["check"], f["verdict"], f["issue"])).collect();
        panic!("no {verdict} {check} finding for type {ty}; have {have:#?}")
    })
}

#[test]
fn the_rule_is_built_in_and_counts_every_verdict() {
    let r = Session::new().execute("validate.rules", &json!({"id": RULE})).unwrap();
    assert_eq!(r["rules"][0]["id"], RULE, "{r}");
    assert_eq!(r["rules"][0]["enabled"], true, "{r}");
    let mut j = job("built-in", true, true);
    let (r, _) = screen(&mut j.s);
    for v in ["pass", "fail", "unsure", "missing", "extra"] {
        assert!(r["checked"][RULE][v].is_u64(), "{v} counted: {}", r["checked"]);
    }
}

#[test]
fn a_luminaire_carrying_its_row_passes_and_its_series_submittal_matches() {
    let mut j = job("match", true, true);
    let (r, fs) = screen(&mut j.s);
    for h in &j.t1 {
        let f = of_handle(&fs, h);
        assert_eq!(f["verdict"], "pass", "{f}");
        assert!(f["issue"].as_str().unwrap().contains("X-601 LIGHT FIXTURE SCHEDULE, type T1"), "{f}");
        let fields: Vec<&str> = f["measured"]["fields"].as_array().unwrap().iter().filter_map(|x| x["status"].as_str()).collect();
        assert_eq!(fields, ["agrees"; 5], "basis of design, lamp, VA, voltage, installation: {f}");
    }
    // The submittal's AT24L35-MVOLT is the row's AT-24-L40 series with other options.
    let p = of_type(&fs, "T1", "submittal", "pass");
    assert_eq!(p["measured"]["submittal"]["catalog"], "AT24L35-MVOLT", "{p}");
    assert!(p["measured"]["submittal"]["others"].to_string().contains("(revise)"), "a later revision sent back doesn't replace it: {p}");
    assert!(p["measured"]["submittal"]["cite"].as_str().unwrap().ends_with("p.1"), "the page it says so on: {p}");
    // The table: T1's plan and model counts, row and product.
    let t1 = r["fixtures"]["types"].as_array().unwrap().iter().find(|t| t["type"] == "T1").unwrap();
    assert_eq!((t1["plan"].as_u64(), t1["model"].as_u64()), (Some(2), Some(2)), "{t1}");
    assert_eq!(t1["verdicts"]["submittal"], "pass", "{t1}");
    assert_eq!(t1["luminaires"]["pass"], 2, "{t1}");
    // The non-lighting submittal wasn't read.
    let files = r["fixtures"]["submittals"]["files"].to_string();
    assert!(!files.contains("Pump"), "{files}");
}

#[test]
fn a_va_and_voltage_mismatch_fails_citing_the_row_not_the_builds_copy() {
    let mut j = job("va", true, true);
    let (_, fs) = screen(&mut j.s);
    let f = of_handle(&fs, &j.t2);
    assert_eq!(f["verdict"], "fail", "{f}");
    let issue = f["issue"].as_str().unwrap();
    assert!(issue.contains("X-601 LIGHT FIXTURE SCHEDULE, type T2"), "{issue}");
    assert!(issue.contains("va 26") && issue.contains("13 VA"), "{issue}");
    assert!(issue.contains("voltage 277") && issue.contains("says 120"), "{issue}");
    let differs: Vec<&str> =
        f["measured"]["fields"].as_array().unwrap().iter().filter(|x| x["status"] == "differs").filter_map(|x| x["field"].as_str()).collect();
    assert_eq!(differs, ["va", "voltage"], "{f}");
    // The build's stored schedule (26 VA, 277 V) isn't what Validate read.
    assert_eq!(f["measured"]["row"]["va"], 13.0, "{f}");
}

#[test]
fn a_schedule_vs_submittal_mismatch_fails_and_matches_the_builds_rfi() {
    let mut j = job("catalog", true, true);
    let (r, fs) = screen(&mut j.s);
    let f = of_type(&fs, "T3", "submittal", "fail");
    let issue = f["issue"].as_str().unwrap();
    assert!(issue.contains("OTHER CO ZX-500") && issue.contains("X-601 LIGHT FIXTURE SCHEDULE, type T3"), "{issue}");
    assert!(issue.contains("NO SUBSTITUTIONS"), "the schedule's note said beside it: {issue}");
    // The latest revision wins; the earlier one is listed.
    assert_eq!(f["measured"]["submittal"]["revision"], "2-0", "{f}");
    assert!(f["measured"]["submittal"]["others"].to_string().contains("p.3"), "{f}");
    assert_eq!(f["measured"]["rfi"], "RFI-L1", "the build's RFI confirms it: {f}");
    assert!(!fs.iter().any(|x| x["tag"] == "T3" && x["measured"]["check"] == "submittal" && x["verdict"] == "missing"));
    assert!(r["fixtures"]["rfis"]["confirmed"].as_array().unwrap().contains(&json!("RFI-L1")), "{}", r["fixtures"]["rfis"]);
    // The luminaire itself carries its row: the documents disagree, not the model.
    assert_eq!(of_handle(&fs, &j.t3)["verdict"], "pass");
}

#[test]
fn types_on_no_plan_and_plan_types_on_no_schedule_are_listed() {
    let mut j = job("lists", true, true);
    let (r, fs) = screen(&mut j.s);
    // T5: scheduled, on no plan, its only submittal sent back; no RFI for either.
    let f = of_type(&fs, "T5", "on_plan", "fail");
    assert_eq!(f["severity"], "low", "{f}");
    of_type(&fs, "T5", "on_plan", "missing");
    let s = of_type(&fs, "T5", "submittal", "fail");
    assert!(s["issue"].as_str().unwrap().contains("sent back (rejected)"), "{s}");
    of_type(&fs, "T5", "submittal", "missing");
    // T9: on the plan, no row: the luminaire is unsure (never a pass), the type fails, the
    // build's RFI confirms it.
    let u = of_handle(&fs, &j.t9);
    assert_eq!(u["verdict"], "unsure", "{u}");
    assert!(u["issue"].as_str().unwrap().contains("no row"), "{u}");
    let t9 = of_type(&fs, "T9", "scheduled", "fail");
    assert_eq!(t9["measured"]["rfi"], "RFI-L2", "{t9}");
    let t5 = r["fixtures"]["types"].as_array().unwrap().iter().find(|t| t["type"] == "T5").unwrap();
    assert_eq!((t5["plan"].as_u64(), t5["model"].as_u64()), (Some(0), Some(0)), "{t5}");
}

#[test]
fn every_lighting_rfi_is_confirmed_disputed_or_listed() {
    let mut j = job("rfis", true, true);
    let (r, fs) = screen(&mut j.s);
    // The maker RFI about T1, whose submittal passes: extra, with Validate's working.
    let x = of_type(&fs, "T1", "submittal", "extra");
    assert_eq!(x["measured"]["rfi"], "RFI-L3", "{x}");
    assert_eq!(x["measured"]["working"][0]["validate"], "pass", "{x}");
    // A CCT RFI: a field Validate doesn't recompute, listed with its sides.
    let c = fs.iter().find(|f| f["measured"]["rfi"] == "RFI-L4").unwrap();
    assert_eq!(c["verdict"], "unsure", "{c}");
    assert_eq!(c["measured"]["sides"].as_array().unwrap().len(), 2, "{c}");
    // The controls RFI isn't this rule's: counted, not matched.
    assert!(r["unmeasured"][RULE].to_string().contains("not a fixture type"), "{}", r["unmeasured"]);
    let rfis = &r["fixtures"]["rfis"];
    assert_eq!(rfis["disputed"], json!(["RFI-L3"]), "{rfis}");
    assert_eq!(rfis["not_recomputed"], json!(["RFI-L4"]), "{rfis}");
    let mut confirmed: Vec<&str> = rfis["confirmed"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    confirmed.sort_unstable();
    assert_eq!(confirmed, ["RFI-L1", "RFI-L2"], "{rfis}");
}

#[test]
fn a_two_sided_schedule_on_a_page_the_drawing_does_not_hold_is_read() {
    let mut j = job("page", false, true);
    let (r, fs) = screen(&mut j.s);
    let tables = &r["fixtures"]["schedule"];
    assert!(tables["pages_read"].to_string().contains("X-602"), "{tables}");
    let f = of_handle(&fs, &j.t2);
    assert_eq!(f["verdict"], "fail", "{f}");
    assert!(f["issue"].as_str().unwrap().contains("X-602 LUMINAIRE SCHEDULE, type T2"), "{f}");
    // T3 and T5 are on the page's right half.
    assert_eq!(of_type(&fs, "T3", "submittal", "fail")["measured"]["row"]["source"], "X-602 LUMINAIRE SCHEDULE, type T3");
    of_type(&fs, "T5", "on_plan", "fail");
    // This schedule gives no lamp or mounting: those fields aren't compared, T1 still passes.
    assert_eq!(of_handle(&fs, &j.t1[0])["verdict"], "pass");
}

#[test]
fn with_no_job_the_drawn_schedule_is_read_and_the_submittal_is_unsure() {
    let mut j = job("nojob", true, false);
    let (r, fs) = screen(&mut j.s);
    assert_eq!(of_handle(&fs, &j.t2)["verdict"], "fail");
    let u = of_type(&fs, "T1", "submittal", "unsure");
    assert!(u["issue"].as_str().unwrap().contains("no lighting submittal was read"), "{u}");
    assert!(r["unmeasured"][RULE].to_string().contains("no job"), "{}", r["unmeasured"]);
}

#[test]
fn hostile_rfis_and_cells_do_not_panic() {
    let mut j = job("hostile", true, true);
    let mut sp = spec(true);
    sp["rfis"] = json!([
        rfi("H1", "catalog number", "lighting type "),
        rfi("H2", "maker", "lighting types ,,, ,"),
        rfi("H3", "", "lighting type \u{0}\u{7f}"),
        rfi("H4", "plan type with no schedule row", &format!("lighting type {}", "X".repeat(5_000)))
    ]);
    j.s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    text(&mut j.s, [6102.0, 1900.0], &"9".repeat(4_000), 4.0);
    text(&mut j.s, [6442.0, 1900.0], "1e400 VA", 4.0);
    let r = j.s.execute("validate.prescreen", &json!({"rules": [RULE], "all": true}));
    assert!(r.is_ok(), "{r:?}");
}
