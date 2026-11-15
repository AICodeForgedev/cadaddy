//! Equipment boxes sized by their approved submittals (Q6655, M-EQUIP-SUBMITTAL-SIZE), on a
//! made-up job ("Project A") whose submittals are made-up PDFs read by the real PDF reader.
//!
//! - HP-9, a heat pump: no size in its schedule, so built at its kind's usual size on its stand.
//!   Its approved submittal's data sheet gives Height 31.89 Width 37.24 Depth 16.14. A later
//!   revision was sent back with other numbers, and an accessory page gives a controller's size.
//! - MAU-4, a makeup air unit: built at the schedule's size (another maker's model). The approved
//!   submittal is a substitution: another maker, its own size, on a 30" curb.
//! - CU-8: no submittal; it keeps the usual size of its kind.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::{Value, json};

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

/// A PDF of letter pages, each its lines of Helvetica text.
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

fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// The made-up job's submittals, in revision folders as a project export keeps them.
fn submittals(root: &Path) {
    let approved: &[&str] = &["SUBMITTAL COVER - PROJECT A", "X NO EXCEPTIONS TAKEN"];
    write(
        &root.join("Split Systems/Rev 1-0/Submittal Split Systems.pdf"),
        &pdf(&[
            &[approved[0], approved[1], "Unit Tags: HP-9"],
            &["OUTDOOR UNIT DATA", "Height 31.89   Width 37.24   Depth 16.14", "Shipping Height 36   Shipping Width 40   Shipping Depth 19"],
            &["WIRELESS CONTROLLER (OPTIONAL)", "Dimensions 4.72\" H x 3.39\" W x 1.06\" D"],
        ]),
    );
    write(
        &root.join("Split Systems/Rev 1-1/Submittal Split Systems.pdf"),
        &pdf(&[&["SUBMITTAL COVER - PROJECT A", "X REVISE AND RESUBMIT", "Unit Tags: HP-9"], &["Height 60 Width 60 Depth 60"]]),
    );
    write(
        &root.join("Makeup Air Units/Rev 1-0/Submittal Makeup Air.pdf"),
        &pdf(&[
            &[approved[0], approved[1], "SUBSTITUTION: OTHERCO MODEL ZQ-310 IN PLACE OF THE SCHEDULED UNIT", "TAG: MAU-4"],
            &["UNIT DIMENSIONS (L x W x H): 81.75 x 54 x 48 in", "ROOF CURB - 30\" HIGH ROOF CURB BY THE UNIT MAKER"],
        ]),
    );
}

fn equipment(tag: &str, kind: &str, min: [f64; 2], max: [f64; 2], bottom: f64, height: f64, sized: &str, props: Value) -> Value {
    json!({"tag": tag, "kind": kind, "level": "L1", "min": min, "max": max, "bottom_ft": bottom, "height_ft": height,
           "title": format!("{} SCHEDULE", kind.replace('_', " ").to_uppercase()), "properties": props,
           "sources": {"outline": format!("no outline by its tag: {sized} at its tag, which way it faces a guess"),
                       "height": if sized.contains("schedule") { "schedule" } else { "rule: the usual height of its kind" }}})
}

fn spec() -> Value {
    let support = |tag: &str, kind: &str, min: [f64; 2], max: [f64; 2], h: f64| {
        json!({"tag": tag, "kind": kind, "level": "L1", "min": min, "max": max, "bottom_ft": 0, "height_ft": h,
               "properties": {"STATUS": "inferred", "UNIT": tag.split(' ').next().unwrap(), "BASIS": "trade practice"}})
    };
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0], "region": [-100, -100, 1300, 700]}],
        "mep": {"equipment": [
            equipment("HP-9", "heat_pump", [100.0, 100.0], [136.0, 136.0], 1.5, 3.0, "the usual size of its kind (rules data)",
                      json!({"STATUS": "inferred", "SUPPORT": "equipment stand 1.50 ft"})),
            support("HP-9 STAND", "equipment_stand", [100.0, 100.0], [136.0, 136.0], 1.5),
            equipment("MAU-4", "rtu", [300.0, 100.0], [410.7, 196.0], 14.0 / 12.0, 4.5, "the schedule's size",
                      json!({"STATUS": "derived", "MANUFACTURER": "MAKERCO", "MODEL": "MK-4410", "SUPPORT": "roof curb 1.17 ft"})),
            support("MAU-4 CURB", "roof_curb", [300.0, 100.0], [410.7, 196.0], 14.0 / 12.0),
            equipment("CU-8", "condensing_unit", [600.0, 100.0], [636.0, 136.0], 0.0, 3.0, "the usual size of its kind (rules data)", json!({"STATUS": "inferred"}))
        ]}
    })
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
    let subs = std::env::temp_dir().join(format!("cadaddy-equipsize-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&subs);
    submittals(&subs);
    let mut s = Session::new();
    let r = s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    let state = json!({"guided_job": {
        "id": "made-up-job", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": "/made-up/prints",
        "stages": [], "audit": [], "question_seq": 0, "submittals": [subs.to_string_lossy()], "rules": []
    }});
    s.doc_mut().unwrap().agent = Some(state.to_string()).into();
    Job { s, subs }
}

/// The built element of `tag`: (handle, x, y, z extents in drawing units), its build tags.
fn element(s: &Session, tag: &str) -> (cadaddy_doc::Handle, [f64; 3], Vec<String>) {
    let d = s.doc().unwrap();
    let e = d
        .model
        .iter()
        .find(|e| e.common.xdata.iter().any(|t| t.value == format!("tag:{tag}")) && e.common.xdata.iter().any(|t| t.value == "kind:equipment"))
        .unwrap_or_else(|| panic!("no element for {tag}"));
    let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { panic!("not a mesh") };
    let ext = |f: fn(&cadaddy_geom::Vec3) -> f64| {
        let (lo, hi) = m.vertices.iter().map(f).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        hi - lo
    };
    (e.handle, [ext(|v| v.x), ext(|v| v.y), ext(|v| v.z)], e.common.xdata.iter().map(|t| t.value.clone()).collect())
}

fn unit<'a>(sp: &'a Value, tag: &str) -> &'a Value {
    sp["mep"]["equipment"].as_array().unwrap().iter().find(|e| e["tag"] == tag).unwrap_or_else(|| panic!("no {tag}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn each_unit_takes_its_approved_submittals_size_in_place() {
    let mut j = job("sizes");
    let (hp_handle, hp_before, _) = element(&j.s, "HP-9");
    assert!(close(hp_before[2], 36.0), "built at the usual size first: {hp_before:?}");
    let r = j.s.execute("bim.equipment_sizes", &json!({})).unwrap();
    assert_eq!((r["resized_count"].as_u64(), r["curbs"].as_u64(), r["sent_back"].as_array().map(Vec::len)), (Some(2), Some(1), Some(1)), "{r:#}");
    assert_eq!(r["sized_before"], json!({"rule": 2, "schedule": 1}), "{r:#}");
    assert_eq!(r["sized_after"], json!({"rule": 1, "submittal": 2}), "{r:#}");
    let sp = j.s.execute("bspec.get", &json!({})).unwrap();
    let sp = if sp.get("spec").is_some() { sp["spec"].clone() } else { sp };
    // HP-9: the approved revision's data sheet (not the one sent back, not the controller's size).
    let hp = unit(&sp, "HP-9");
    assert_eq!(hp["properties"]["SIZE_BASIS"], "submittal Submittal Split Systems.pdf p.2 (no exceptions taken)", "{hp:#}");
    assert_eq!(hp["properties"]["SIZE"], "37.24 W x 16.14 D x 31.89 H in");
    assert_eq!(hp["properties"]["STATUS"], "derived");
    // Its element resized in place: the same handle, 37.24 x 16.14 x 31.89 in, the basis on it.
    let (h, ext, tags) = element(&j.s, "HP-9");
    assert_eq!(h, hp_handle, "resized in place, not rebuilt");
    assert!(close(ext[0].max(ext[1]), 37.24) && close(ext[0].min(ext[1]), 16.14) && close(ext[2], 31.89), "{ext:?}");
    assert!(tags.iter().any(|t| t == "prop:SIZE_BASIS=submittal Submittal Split Systems.pdf p.2 (no exceptions taken)"), "{tags:#?}");
    // MAU-4: the substitution's size on its 30" curb; the schedule's size kept as the design value.
    let mau = unit(&sp, "MAU-4");
    assert_eq!(mau["properties"]["SIZE"], "81.75 W x 54 D x 48 H in", "{mau:#}");
    assert!(close(mau["bottom_ft"].as_f64().unwrap(), 2.5), "{mau:#}");
    let c = &mau["corrections"][0];
    assert_eq!(c["key"], "size");
    assert!(c["design"].as_str().unwrap().starts_with("110.7 W x 96 D x 54 H in (the schedule's size)"), "{c:#}");
    assert!(c["reason"].as_str().unwrap().contains("Submittal Makeup Air.pdf p.2"), "{c:#}");
    assert!(c["rule"].as_str().unwrap().starts_with("M-EQUIP-SUBMITTAL-SIZE"), "{c:#}");
    let curb = unit(&sp, "MAU-4 CURB");
    assert!(close(curb["height_ft"].as_f64().unwrap(), 2.5) && curb["properties"]["STATUS"] == "derived", "{curb:#}");
    let (_, ext, _) = element(&j.s, "MAU-4 CURB");
    assert!(close(ext[2], 30.0), "the curb element at its submittal's height: {ext:?}");
    // CU-8: no submittal; still the usual size of its kind, said so.
    let cu = unit(&sp, "CU-8");
    assert!(cu["sources"]["outline"].as_str().unwrap().contains("usual size of its kind"), "{cu:#}");
    // The provenance listing shows the corrected size beside the design one.
    let p = j.s.execute("bim.provenance", &json!({"corrections": true})).unwrap();
    assert!(
        p["corrections"]["by_rule"].as_object().is_some_and(|m| m.keys().any(|k| k.starts_with("M-EQUIP-SUBMITTAL-SIZE"))),
        "{:#}",
        p["corrections"]
    );
    // Again: nothing more to change.
    let again = j.s.execute("bim.equipment_sizes", &json!({})).unwrap();
    assert_eq!((again["resized_count"].as_u64(), again["confirmed"].as_u64()), (Some(0), Some(2)), "{again:#}");
}

#[test]
fn a_dry_run_changes_nothing_and_bad_params_are_refused() {
    let mut j = job("dry");
    let r = j.s.execute("bim.equipment_sizes", &json!({"dry_run": true})).unwrap();
    assert_eq!(r["resized_count"].as_u64(), Some(2), "{r:#}");
    let (_, ext, _) = element(&j.s, "HP-9");
    assert!(close(ext[2], 36.0), "{ext:?}");
    assert!(j.s.execute("bim.equipment_sizes", &json!({"paths": "a.pdf"})).is_err());
    assert!(j.s.execute("bim.equipment_sizes", &json!({"dry_run": "yes"})).is_err());
    assert!(j.s.execute("bim.equipment_sizes", &json!({"paths": ["/made-up/none.pdf"]})).is_err());
    // No model: said, not a crash.
    let mut empty = Session::new();
    assert!(empty.execute("bim.equipment_sizes", &json!({"paths": [j.subs.to_string_lossy()]})).is_err());
}
