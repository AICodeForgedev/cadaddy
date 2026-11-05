//! E-EQUIP-ELEC (Q6177) on a made-up job ("Project A"): equipment connections checked against
//! their submittals (made-up PDFs read by the real PDF reader) and the code rules data, and the
//! build's `equipment_electrical` RFIs matched against the verdicts.
//!
//! - PMP-1, a pump: MCA 20 A, MOP 35 A, FLA 16 A, SCCR 5 kA (its approved revision; an earlier one
//!   sent back says other values). Its circuit: 30 A on #10 copper in 3/4" conduit, a 30 A
//!   disconnect; its panel has 3.2 kA available. Everything passes.
//! - RTU-1, a rooftop unit: MCA 46 A, MOP 60 A, SCCR 5 kA; protected at 70 A from a panel with
//!   22 kA available. Protection fails (the build's RFI says so too), SCCR fails (no RFI: missing).
//!   The model follows its first revision; a later approved one exists.
//! - ELEV-1, an elevator: its submittal is a scan with no answer: unsure (the build's RFI about it
//!   has no failure behind it: extra).
//! - FAN-1 and FAN-2: scanned too; an agent answered both, FAN-1's answer cites no page (unsure),
//!   FAN-2's cites page 3 (checked, the page reported).

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// The real PDF reader behind the job's submittals.
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

/// A PDF of letter pages, each its lines of Helvetica text (none: a scan, only a frame drawn).
fn pdf(pages: &[&[&str]]) -> Vec<u8> {
    let mut objs: Vec<String> =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), String::new(), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
    let mut kids = Vec::new();
    for lines in pages {
        let mut c = String::from("0 0 0 RG 1 w 36 36 540 720 re S\n");
        for (i, l) in lines.iter().enumerate() {
            let esc = l.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
            c.push_str(&format!("BT /F1 10 Tf 72 {} Td ({esc}) Tj ET\n", 700 - 14 * i as i32));
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

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-validateequip-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// The made-up job's submittals, revision folders as a project export keeps them.
fn submittals(root: &Path) {
    let cover: &[&str] = &["SUBMITTAL COVER - PROJECT A", "X NO EXCEPTIONS TAKEN"];
    // The pump: an earlier revision sent back (other values), the approved one.
    write(
        &root.join("Pumps/Rev 1-0/PMP-1 Pump Submittal.pdf"),
        &pdf(&[
            &["SUBMITTAL COVER - PROJECT A", "REVISE AND RESUBMIT"],
            &["PMP-1 CIRCULATING PUMP", "FULL LOAD AMPS: 30", "MOP: 20", "SCCR: 1 kAmp"],
        ]),
    );
    write(
        &root.join("Pumps/Rev 1-1/PMP-1 Pump Submittal.pdf"),
        &pdf(&[
            cover,
            &["PMP-1 CIRCULATING PUMP", "208 V 3 PH 60 HZ", "FULL LOAD AMPS: 16", "MINIMUM CIRCUIT AMPACITY: 20", "MOP: 35", "SCCR: 5 kAmp"],
        ]),
    );
    // The rooftop unit: its first revision (the model's), and a later approved one.
    let rtu: &[&str] = &["RTU-1 ROOFTOP UNIT", "208 V 3 PH", "46 Amps MCA", "60 Amps MOP", "SCCR: 5 kAmp"];
    write(&root.join("Rooftop Units/Rev 1-0/RTU-1 Submittal First.pdf"), &pdf(&[cover, rtu]));
    write(&root.join("Rooftop Units/Rev 1-1/RTU-1 Submittal.pdf"), &pdf(&[cover, rtu]));
    // The elevator and the fans: scans (no text).
    write(&root.join("Elevator/Rev 1-0/ELEV Shop Drawings.pdf"), &pdf(&[&[], &[]]));
    write(&root.join("Fans/Rev 1-0/FAN Submittal.pdf"), &pdf(&[&[], &[], &[]]));
    // The chiller: its first revision's text gives values; the later one (another maker) is a scan.
    write(&root.join("Chillers/Rev 1-0/CHL-1 Submittal.pdf"), &pdf(&[cover, &["CHL-1 CHILLER", "46 Amps MCA", "50 Amps MOP"]]));
    write(&root.join("Chillers/Rev 2-0/CHL-1 Submittal.pdf"), &pdf(&[&[]]));
}

fn row(tag: &str, desc: &str, panel: &str, ocpd: &str, source: &str) -> Value {
    json!({"tag": tag, "description": desc, "source_panel": panel, "drawing_ocpd": ocpd, "feeder": "", "source": source})
}

fn circuit(n: &str, desc: &str, breaker: &str, wire: &str) -> Value {
    json!({"number": n, "description": desc, "breaker": breaker, "poles": 3, "wire": wire, "source": "X-601"})
}

fn rfi(id: &str, about: &str) -> Value {
    json!({"id": id, "kind": "equipment_electrical", "severity": "high", "about": about, "issue": format!("Submittal against the drawings: {about}"), "assumed": "the submittal", "status": "open"})
}

fn spec() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-100, -100, 1300, 700]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "rooms": [{"name": "Mechanical", "number": "11", "level": "L1"}],
        "mep": {
            "panel_schedules": [
                {"panel": "LP1", "source": "X-601", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "afc value": "3,200A"},
                 "circuits": [circuit("1", "PMP-1", "30", "feeder 10: 1 set(s) of 4 10 cu, ground 10, in 0.75\" conduit"), circuit("3", "FAN-1", "20", "12"), circuit("5", "FAN-2", "20", "12")]},
                {"panel": "DP1", "source": "X-601", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "afc value": "22 kA"},
                 "circuits": [circuit("2", "RTU-1", "70", "feeder 60: 1 set(s) of 4 6 cu, ground 10, in 1\" conduit"), circuit("4", "ELEVATOR", "60", "feeder 61: 1 set(s) of 4 6 cu, ground 10, in 1\" conduit")]}
            ],
            "equipment_electrical": [
                row("PMP-1", "PMP-1", "LP1", "30A/3P", "submittal PMP-1 Pump Submittal.pdf"),
                row("RTU-1", "RTU-1", "DP1", "70A/3P", "submittal RTU-1 Submittal First.pdf"),
                row("ELEV-1", "ELEVATOR", "DP1", "60A/3P", ""),
                row("FAN-1", "FAN-1", "LP1", "20A/3P", ""),
                row("FAN-2", "FAN-2", "LP1", "20A/3P", ""),
                row("CHL-1", "CHL-1", "DP1", "70A/3P", "")
            ],
            "devices": [
                {"tag": "DS-PMP-1", "kind": "disconnect", "level": "L1", "room": "11", "panel": "LP1", "circuit": "1", "properties": {"RATING": "30A", "TYPE": "NON-FUSED"}}
            ]
        },
        "rfis": [
            rfi("RFI-E1", "RTU-1 (protection)"),
            rfi("RFI-E2", "ELEV-1 (protection)"),
            rfi("RFI-E3", "PMP-1 (conductors)"),
            {"id": "RFI-D1", "kind": "equipment_electrical", "severity": "high", "about": "Fire damper power", "issue": "no circuit", "assumed": "none", "status": "open"}
        ]
    })
}

/// The guided job: its submittals folder, and the agents' answers for the fans.
fn set_job(s: &mut Session, subs: &Path) {
    let answer = |src: &str| json!({"mca": 6.0, "max_ocpd_a": 20.0, "fla": 4.8, "sccr_a": 5000.0, "source": src}).to_string();
    let state = json!({"guided_job": {
        "id": "made-up-job", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": "/made-up/prints",
        "stages": [], "audit": [], "question_seq": 0, "submittals": [subs.to_string_lossy()],
        "rules": [
            {"key": "equip:FAN-1", "value": answer("an agent's reading of the submittal"), "from_question": "Q-bim_build-1", "stage": "bim_build"},
            {"key": "equip:FAN-2", "value": answer("FAN Submittal.pdf p.3"), "from_question": "Q-bim_build-2", "stage": "bim_build"}
        ]
    }});
    s.doc_mut().unwrap().agent = Some(state.to_string());
}

fn model(spec: Value) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [300.0, 186.0]);
    rect(&mut s, [6.0, 6.0], [294.0, 180.0]);
    layer(&mut s, "A-ROOM-TAG");
    s.execute("text", &json!({"at": [150.0, 90.0], "text": "11", "height": 6.0})).unwrap();
    let r = s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn screen(s: &mut Session) -> (Value, Vec<Value>) {
    let p = |c: Option<u64>| {
        let mut p = json!({"rules": ["E-EQUIP-ELEC"], "all": true, "limit": 50});
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
    (r, fs)
}

fn finding<'a>(fs: &'a [Value], tag: &str, check: &str, verdict: &str) -> &'a Value {
    fs.iter().find(|f| f["rule"] == "E-EQUIP-ELEC" && f["tag"] == tag && f["measured"]["check"] == check && f["verdict"] == verdict).unwrap_or_else(
        || {
            let have: Vec<String> =
                fs.iter().filter(|f| f["tag"] == tag).map(|f| format!("{} {}: {}", f["measured"]["check"], f["verdict"], f["issue"])).collect();
            panic!("no {verdict} {check} finding for {tag}; have {have:#?}")
        },
    )
}

struct Job {
    s: Session,
    subs: PathBuf,
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.subs);
    }
}

fn job(name: &str) -> Job {
    hooks();
    let subs = dir(name);
    submittals(&subs);
    let mut s = model(spec());
    set_job(&mut s, &subs);
    Job { s, subs }
}

#[test]
fn the_rule_is_built_in_and_run_by_the_prescreen() {
    let r = Session::new().execute("validate.rules", &json!({"id": "E-EQUIP-ELEC"})).unwrap();
    let rule = &r["rules"][0];
    assert_eq!(rule["id"], "E-EQUIP-ELEC", "{r}");
    assert_eq!(rule["severity"], "high");
    let mut j = job("built-in");
    let (r, _) = screen(&mut j.s);
    let counts = &r["checked"]["E-EQUIP-ELEC"];
    for v in ["pass", "fail", "unsure", "missing", "extra"] {
        assert!(counts[v].is_u64(), "{v} counted: {counts}");
    }
}

#[test]
fn a_motor_load_with_mca_and_mop_and_a_correct_circuit_passes() {
    let mut j = job("pump");
    let (_, fs) = screen(&mut j.s);
    for check in ["protection", "conductors", "conduit_fill", "disconnect", "sccr", "revision"] {
        finding(&fs, "PMP-1", check, "pass");
    }
    // Its values are the approved revision's (the earlier one, sent back, said 20 A MOP), with the page.
    let p = finding(&fs, "PMP-1", "protection", "pass");
    let sub = &p["measured"]["submittal"];
    assert_eq!(sub["values"]["max_ocpd_a"], 35.0, "{sub}");
    assert_eq!(sub["revision"], "1-1", "{sub}");
    assert_eq!(sub["status"], "no exceptions taken", "{sub}");
    assert_eq!(sub["page"], 2, "{sub}");
    // Conductors: #10 copper 35 A against its 20 A MCA; disconnect 30 A against 1.15 x 16 A.
    let c = finding(&fs, "PMP-1", "conductors", "pass");
    assert_eq!(c["measured"]["recomputed"]["need_a"], 20.0, "{c}");
    assert_eq!(c["measured"]["recomputed"]["have_a"], 35.0, "{c}");
    let d = finding(&fs, "PMP-1", "disconnect", "pass");
    assert_eq!(d["measured"]["recomputed"]["standard_a"], 30.0, "{d}");
    // The build's RFI about its conductors has no failure behind it.
    let x = finding(&fs, "PMP-1", "conductors", "extra");
    assert_eq!(x["measured"]["rfi"], "RFI-E3", "{x}");
    assert_eq!(x["measured"]["validate"], "pass", "{x}");
}

#[test]
fn protection_above_mop_fails_citing_both_values_and_matches_the_builds_rfi() {
    let mut j = job("rtu");
    let (_, fs) = screen(&mut j.s);
    let f = finding(&fs, "RTU-1", "protection", "fail");
    let issue = f["issue"].as_str().unwrap();
    assert!(issue.contains("70A/3P") && issue.contains("at most 60 A") && issue.contains("RTU-1 Submittal.pdf p.2"), "{issue}");
    assert_eq!(f["measured"]["rfi"], "RFI-E1", "the build's RFI confirms it: {f}");
    assert!(!fs.iter().any(|x| x["tag"] == "RTU-1" && x["measured"]["check"] == "protection" && x["verdict"] == "missing"));
}

#[test]
fn sccr_below_the_sources_fault_current_fails_and_with_no_rfi_is_missing() {
    let mut j = job("sccr");
    let (_, fs) = screen(&mut j.s);
    let f = finding(&fs, "RTU-1", "sccr", "fail");
    assert!(f["issue"].as_str().unwrap().contains("22000 A"), "{f}");
    assert_eq!(f["measured"]["recomputed"]["afc_a"], 22_000.0, "{f}");
    let m = finding(&fs, "RTU-1", "sccr", "missing");
    assert!(m["issue"].as_str().unwrap().contains("no RFI"), "{m}");
}

#[test]
fn the_model_following_an_older_revision_is_a_finding() {
    let mut j = job("revision");
    let (_, fs) = screen(&mut j.s);
    let f = finding(&fs, "RTU-1", "revision", "fail");
    assert!(f["issue"].as_str().unwrap().contains("rev 1-0") && f["issue"].as_str().unwrap().contains("1-1"), "{f}");
    // A model whose record follows a submittal sent back.
    let mut sp = spec();
    sp["mep"]["equipment_electrical"][0]["status"] = json!("revise and resubmit");
    let mut s = model(sp);
    set_job(&mut s, &j.subs);
    let (_, fs) = screen(&mut s);
    finding(&fs, "PMP-1", "revision", "fail");
}

#[test]
fn a_scanned_submittal_with_no_answer_is_unsure_never_a_pass() {
    let mut j = job("scan");
    let (r, fs) = screen(&mut j.s);
    for check in ["protection", "conductors", "disconnect", "sccr"] {
        let f = finding(&fs, "ELEV-1", check, "unsure");
        assert!(f["issue"].as_str().unwrap().contains("scanned"), "{f}");
    }
    assert!(
        !fs.iter().any(|f| f["tag"] == "ELEV-1" && f["verdict"] == "pass" && f["measured"]["check"] != "conduit_fill"),
        "only the fill, which needs no submittal, may pass"
    );
    // The build's RFI about it: Validate can't confirm it.
    let x = finding(&fs, "ELEV-1", "protection", "extra");
    assert_eq!(x["measured"]["validate"], "unsure", "{x}");
    // An answer with no page cited is no source.
    let f = finding(&fs, "FAN-1", "protection", "unsure");
    assert!(f["issue"].as_str().unwrap().contains("no page cited"), "{f}");
    // An answer citing its page is checked, the page reported.
    let f = finding(&fs, "FAN-2", "protection", "pass");
    assert_eq!(f["measured"]["submittal"]["from"], "agent", "{f}");
    assert_eq!(f["measured"]["submittal"]["page"], 3, "{f}");
    // A later revision that is a scan supersedes an earlier one's text: unsure, not a fail.
    let f = finding(&fs, "CHL-1", "protection", "unsure");
    assert!(f["issue"].as_str().unwrap().contains("revision 1-0's values are superseded"), "{f}");
    // The damper-power RFI of the same kind is about no connection: counted, not matched.
    let why = r["unmeasured"]["E-EQUIP-ELEC"].to_string();
    assert!(why.contains("about no equipment connection"), "{why}");
}

#[test]
fn an_rfi_with_no_matching_failure_is_a_finding() {
    let mut j = job("extra");
    let (r, fs) = screen(&mut j.s);
    let mut extras: Vec<&str> = fs.iter().filter(|f| f["verdict"] == "extra").filter_map(|f| f["measured"]["rfi"].as_str()).collect();
    extras.sort_unstable();
    assert_eq!(extras, ["RFI-E2", "RFI-E3"], "{extras:?}");
    assert_eq!(r["checked"]["E-EQUIP-ELEC"]["extra"], 2, "{}", r["checked"]);
    // RTU-1's SCCR and revision failures have no RFI.
    let mut missing: Vec<&str> = fs.iter().filter(|f| f["verdict"] == "missing").filter_map(|f| f["measured"]["check"].as_str()).collect();
    missing.sort_unstable();
    assert_eq!(missing, ["revision", "sccr"], "{}", r["checked"]);
}

#[test]
fn with_no_job_the_models_record_is_checked_and_an_unsourced_one_is_unsure() {
    let mut sp = spec();
    sp["mep"]["equipment_electrical"] = json!([
        {"tag": "PMP-1", "source_panel": "LP1", "drawing_ocpd": "30A/3P", "mca": 20.0, "max_ocpd_a": 25.0, "fla": 16.0, "sccr_a": 5000.0, "source": "submittal PMP-1 Pump Submittal.pdf"},
        {"tag": "RTU-1", "source_panel": "DP1", "drawing_ocpd": "70A/3P", "mca": 46.0, "max_ocpd_a": 80.0, "source": ""}
    ]);
    let mut s = model(sp);
    let (_, fs) = screen(&mut s);
    let f = finding(&fs, "PMP-1", "protection", "fail");
    assert_eq!(f["measured"]["submittal"]["from"], "record", "{f}");
    let u = finding(&fs, "RTU-1", "protection", "unsure");
    assert!(u["issue"].as_str().unwrap().contains("no source"), "{u}");
}

#[test]
fn hostile_records_do_not_panic() {
    let mut sp = spec();
    sp["mep"]["equipment_electrical"] = json!([
        {"tag": "", "drawing_ocpd": "1e308A/9P"},
        {"tag": "X (y)", "mca": 1e308, "max_ocpd_a": -5.0, "sccr_a": 0.0, "source": "p.99999999999999"},
        {"tag": "ZZZ-9", "source_panel": "NOPE", "drawing_ocpd": "NaN/3P", "mca": 1e-300, "fla": 1e300, "source": "page "}
    ]);
    sp["mep"]["panel_schedules"][0]["circuits"] = json!([
        {"number": "1", "description": "ZZZ-9", "breaker": "99999999999999999999", "poles": 3, "wire": "feeder x: 4294967295 set(s) of 4294967295 1e9 cu, ground ?, in 99\" conduit"},
        {"number": "2", "description": "ZZZ-9", "breaker": "", "poles": 0, "wire": "feeder"}
    ]);
    sp["mep"]["panel_schedules"][0]["header"]["afc value"] = json!("1e400 kA");
    sp["rfis"] = json!([rfi("R1", "ZZZ-9 (sccr)"), rfi("R2", "("), rfi("R3", "()"), rfi("R4", " (protection)")]);
    let mut s = model(sp);
    let r = s.execute("validate.prescreen", &json!({"rules": ["E-EQUIP-ELEC"], "all": true}));
    assert!(r.is_ok(), "{r:?}");
}
