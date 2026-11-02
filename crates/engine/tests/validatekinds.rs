//! Validate's rules match job-built devices (Q6128): the PDF→BIM job builds its devices as kind
//! `device` with the plan tag as their mark, so G-HOST and G-HEIGHT once checked none of them.
//! A generic element is read as what the job's legend, its symbol table or the project's
//! `validate.kind.<mark>` rules say its mark is; the packet carries that kind and the trade of
//! its BIM category (an air terminal and an exhaust fan are mechanical); the prescreen lists
//! every rule it ran and the elements no rule could apply to.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, RawTag};
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

fn dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("cadaddy-vwalls-{}-{name}-{nanos}", std::process::id()))
}

fn read_json(p: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// The packet of an area: its manifest and objects.
fn packet(s: &mut Session, area: &str) -> (Value, Value) {
    let out = dir("packet");
    let m = s.execute("validate.packet", &json!({"area": area, "out_dir": out.to_string_lossy()})).unwrap();
    let o = read_json(&out.join("objects.json"));
    let _ = std::fs::remove_dir_all(&out);
    (m, o)
}

/// The PDF→BIM job on a made-up print set: a floor plan and its power plan.
fn job() -> Session {
    real_pdf_reader();
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["A-101", "E-201"].contains(&s.number.as_str()));
    let folder = dir("job");
    std::fs::create_dir_all(&folder).unwrap();
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(folder.join("Sample Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).unwrap();
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": folder.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let _ = std::fs::remove_dir_all(&folder);
    s
}

/// The job's spec tags as a job with no legend kinds leaves them: every device of kind `device`,
/// with only the plan tag as its type (no legend words), and no kind stored on its built device
/// (Q6130 stores one). The marks' counts.
fn generic_tags(s: &mut Session) -> BTreeMap<String, u64> {
    use cadaddy_engine::doc::props;
    let d = s.doc_mut().unwrap();
    let built: Vec<_> = d.model.iter().filter(|e| made_as(e).as_deref() == Some("device")).map(|e| e.handle).collect();
    for h in built {
        d.modify_entity(h, |e| {
            let mut p = props::read(&e.common.xdata);
            for key in ["kind", "legend_kind", "device_kind", "kind_source", "ifc_class"] {
                p.remove(key);
            }
            e.common.xdata = props::write(&e.common.xdata, &p).into();
        })
        .unwrap();
    }
    let mut sp: Value = serde_json::from_str(d.building_spec.as_deref().unwrap()).unwrap();
    let mut marks = BTreeMap::new();
    for t in sp["tags"].as_array_mut().unwrap().iter_mut().filter(|t| t["id"].as_str().unwrap_or("").starts_with("device:")) {
        t["kind"] = json!("device");
        let f = t["fields"].as_object_mut().unwrap();
        f.retain(|k, _| matches!(k.as_str(), "type" | "sheet" | "symbol"));
        *marks.entry(f["type"]["value"].as_str().unwrap().to_string()).or_default() += 1;
    }
    d.building_spec = Some(sp.to_string());
    marks
}

fn rule(s: &mut Session, domain: &str, key: &str, value: &str) {
    s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "reason": "made-up test rule"})).unwrap();
}

fn text(s: &mut Session, p: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": p, "text": t, "height": 6})).unwrap();
}

/// An electrical legend beside the plans, far from them: each mark and what it is.
fn legend(s: &mut Session, rows: &[(&str, &str)]) {
    let (x, y) = (-20_000.0, -20_000.0);
    text(s, [x, y], "ELECTRICAL SYMBOL LEGEND");
    for (i, (mark, said)) in rows.iter().enumerate() {
        let r = y - 18.0 * (i as f64 + 1.0);
        text(s, [x - 30.0, r], mark);
        text(s, [x + 40.0, r], said);
    }
}

fn verdicts(r: &Value, rule: &str) -> u64 {
    r["checked"][rule].as_object().map_or(0, |m| m.values().filter_map(Value::as_u64).sum())
}

/// Every finding of a prescreen, page by page.
fn all_findings(s: &mut Session) -> (Value, Vec<Value>) {
    let first = s.execute("validate.prescreen", &json!({"all": true, "limit": 50})).unwrap();
    let mut out: Vec<Value> = first["findings"].as_array().unwrap().clone();
    let mut next = first["next"]["cursor"].as_u64();
    while let Some(c) = next {
        let r = s.execute("validate.prescreen", &json!({"all": true, "limit": 50, "cursor": c})).unwrap();
        out.extend(r["findings"].as_array().unwrap().iter().cloned());
        next = r["next"]["cursor"].as_u64();
    }
    (first, out)
}

#[test]
fn generic_job_devices_get_g_host_and_g_height_by_their_legend_and_project_rules() {
    let mut s = job();
    let marks = generic_tags(&mut s);
    let n = |m: &str| marks.get(m).copied().unwrap_or(0);
    assert!(n("REC") >= 10 && n("S") >= 5 && n("A") >= 1 && n("B") >= 1, "the made-up set's marks: {marks:?}");
    let devices: u64 = marks.values().sum();

    // As the job leaves it: no rule naming kinds applies, and the prescreen says so.
    let r = s.execute("validate.prescreen", &json!({})).unwrap();
    assert_eq!(r["checked"]["G-HOST"], json!({"pass": 0, "fail": 0}), "every rule run is listed, with 0: {r}");
    // G-HEIGHT: the devices whose plan prints a height by them (`+48" AFF` by each switch) were
    // built at it (Q6158); the job's usual heights for the rest (no document gives them) are
    // reported as the defaults they are.
    assert_eq!(r["checked"]["G-HEIGHT"]["fail"].as_u64(), Some(0), "{r}");
    assert!(r["checked"]["G-HEIGHT"]["pass"].as_u64().is_some_and(|p| p >= n("S")), "{r}");
    assert!(r["checked"]["G-HEIGHT"]["default"].as_u64().is_some_and(|n| n > 0), "{r}");
    assert!(r["checked"]["G-PRESENT"].is_object(), "{r}");
    assert_eq!(r["unmatched"]["elements"].as_u64(), Some(devices), "{r}");
    assert_eq!(r["unmatched"]["kinds"]["device"].as_u64(), Some(devices), "{r}");
    assert_eq!(r["unmatched"]["marks"]["REC"].as_u64(), Some(n("REC")), "{r}");
    assert!(r["unmatched"]["why"].as_str().unwrap().contains("validate.kind.<mark>"), "{r}");

    // The job's legend names REC and S; the project names A; B stays unnamed. A project rule
    // never overrides the legend (REC stays a receptacle).
    legend(&mut s, &[("REC", "DUPLEX RECEPTACLE, 18\" AFF"), ("S", "SINGLE POLE SWITCH"), ("SD", "SMOKE DETECTOR")]);
    rule(&mut s, "custom", "validate.kind.A", "thermostat");
    rule(&mut s, "custom", "validate.kind.REC", "switch");
    rule(&mut s, "mounting_height", "receptacle", "18");
    rule(&mut s, "mounting_height", "switch", "54");
    let (r, found) = all_findings(&mut s);
    let host = verdicts(&r, "G-HOST");
    assert!(host >= n("REC") + n("S") + n("A") - 2, "G-HOST runs on the named devices ({host} of {devices}): {r}");
    assert!(host < devices, "B is not wall-hosted by any rule: {r}");
    assert!(verdicts(&r, "G-HEIGHT") >= n("REC"), "G-HEIGHT runs on them: {r}");
    assert_eq!(r["unmatched"]["elements"].as_u64(), Some(n("B")), "{r}");
    assert_eq!(r["unmatched"]["marks"], json!({"B": n("B")}), "{r}");
    let of = |rule: &str, mark: &str| -> Vec<&Value> { found.iter().filter(|f| f["rule"] == rule && f["mark"] == mark).collect() };
    let rec = of("G-HOST", "REC");
    assert!(rec.len() as u64 >= n("REC") - 1, "{r}");
    assert!(rec.iter().all(|f| f["kind"] == "receptacle" && f["kind_from"] == "legend"), "{rec:?}");
    assert!(of("G-HOST", "S").iter().all(|f| f["kind"] == "switch" && f["kind_from"] == "legend"));
    assert!(of("G-HOST", "A").iter().all(|f| f["kind"] == "thermostat" && f["kind_from"] == "project_rule"));
    // The switches are built at the +48" their plan prints by them (a project's 54" is no
    // document of theirs over that note); one with no note by it is built at 48" (a default) and
    // the project says 54": G-HEIGHT measures that.
    let sw = of("G-HEIGHT", "S");
    let noted = |f: &&Value| f["verdict"] == "pass" && f["measured"]["expected_from"].as_str().is_some_and(|w| w.contains("+48"));
    let short = |f: &&Value| {
        f["verdict"] == "fail" && f["measured"]["model_height_from"] == "default" && (f["measured"]["off_in"].as_f64().unwrap() + 6.0).abs() < 0.5
    };
    assert!(sw.iter().any(noted) && sw.iter().all(|f| noted(f) || short(f)), "{sw:?}");
    assert!(of("G-HEIGHT", "REC").iter().any(|f| f["verdict"] == "pass"));
}

/// A part the build made, as a copy of a built device's mesh with other build tags.
fn built_like(s: &mut Session, like: &Entity, tags: &[&str]) -> String {
    let d = s.doc_mut().unwrap();
    let mut e = like.clone();
    e.handle = d.new_handle();
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    x.extend(tags.iter().map(|t| RawTag { code: 1000, value: (*t).into() }));
    e.common.xdata = x.into();
    let h = e.handle.hex();
    d.model.push(e);
    h
}

#[test]
fn a_return_grille_and_an_exhaust_fan_are_mechanical_and_receptacles_electrical() {
    let mut s = job();
    generic_tags(&mut s);
    legend(&mut s, &[("REC", "DUPLEX RECEPTACLE"), ("EF", "EXHAUST FAN, CEILING MOUNTED")]);
    // The room of a built receptacle, and a return grille and an exhaust fan built beside it.
    let areas = s.execute("validate.areas", &json!({"kind": "room", "limit": 50})).unwrap();
    let mut pick = None;
    for a in areas["areas"].as_array().unwrap() {
        let (_, o) = packet(&mut s, a["id"].as_str().unwrap());
        if let Some(e) = o["elements"].as_array().unwrap().iter().find(|e| e["mark"] == "REC") {
            pick = Some((a["id"].as_str().unwrap().to_string(), e["handle"].as_str().unwrap().to_string()));
            break;
        }
    }
    let (area, rec) = pick.expect("a room with a built receptacle");
    let like = {
        let d = s.doc().unwrap();
        let h = cadaddy_engine::doc::Handle::parse_hex(&rec).unwrap();
        (**d.model.get(h).unwrap()).clone()
    };
    assert_eq!(made_as(&like).as_deref(), Some("device"));
    let level = cadaddy_engine::cmd::bspec::made_level(&like).unwrap();
    let grille = built_like(&mut s, &like, &["mep", &level, "0", "system:RA", "kind:duct", "part:terminal:return_grille"]);
    let fan = built_like(&mut s, &like, &["device", &level, "0", "tag:EF"]);
    // A device whose kind the job stored (Q6130): that kind, before the legend's for its mark.
    let gfci = built_like(&mut s, &like, &["device", &level, "0", "tag:REC", "kind:gfci_receptacle"]);
    let (m, o) = packet(&mut s, &area);
    let el = |h: &str| o["elements"].as_array().unwrap().iter().find(|e| e["handle"] == h).cloned().unwrap_or_else(|| panic!("{h} not in {o:#}"));
    let g = el(&grille);
    assert_eq!((&g["kind"], &g["trade"], &g["category"]), (&json!("return_grille"), &json!("mechanical"), &json!("IfcAirTerminal")), "{g}");
    assert_eq!((&g["kind_read"], &g["kind_from"]), (&json!("fixture"), &json!("stored")), "{g}");
    let f = el(&fan);
    assert_eq!(
        (&f["kind"], &f["trade"], &f["category"], &f["kind_from"]),
        (&json!("exhaust_fan"), &json!("mechanical"), &json!("IfcFan"), &json!("legend")),
        "{f}"
    );
    let k = el(&gfci);
    assert_eq!(
        (&k["kind"], &k["kind_from"], &k["trade"], &k["category"]),
        (&json!("gfci_receptacle"), &json!("stored"), &json!("electrical"), &json!("IfcOutlet")),
        "{k}"
    );
    let r = el(&rec);
    assert_eq!(
        (&r["kind"], &r["trade"], &r["category"], &r["kind_read"]),
        (&json!("receptacle"), &json!("electrical"), &json!("IfcOutlet"), &json!("device")),
        "{r}"
    );
    // The rules for them come with the packet.
    let out = dir("rules");
    s.execute("validate.packet", &json!({"area": area, "out_dir": out.to_string_lossy()})).unwrap();
    let rules = read_json(&out.join("rules.json"));
    let _ = std::fs::remove_dir_all(&out);
    let ids: Vec<&str> = rules["rules"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
    for id in ["G-HOST", "E-RECEPT-POS", "M-TERMINAL", "M-EF"] {
        assert!(ids.contains(&id), "{id} in {ids:?} ({m})");
    }
    assert!(!ids.contains(&"P-FIXTURE"), "a return grille is no plumbing fixture: {ids:?}");
}

#[test]
fn a_marks_kind_rule_is_checked_when_set() {
    let mut s = Session::new();
    for (k, v) in [("validate.kind.EF", "device"), ("validate.kind.", "receptacle")] {
        let e = s.execute("project.rules.set", &json!({"domain": "custom", "key": k, "value": v, "reason": "made-up"})).unwrap_err().to_string();
        assert!(e.contains("validate.kind"), "{k}: {e}");
    }
    let e = s
        .execute("project.rules.set", &json!({"domain": "tolerance", "key": "validate.kind.EF", "value": "exhaust_fan", "reason": "made-up"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("custom"), "{e}");
    rule(&mut s, "custom", "validate.kind.EF", "exhaust fan");
    // Not a rule override: validate.rules lists nothing refused.
    let r = s.execute("validate.rules", &json!({"limit": 1})).unwrap();
    assert_eq!(r["rejected"], json!([]), "{r}");
}
