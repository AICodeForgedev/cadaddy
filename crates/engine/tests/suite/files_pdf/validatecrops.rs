//! Packet crops against their PDF pages (Q6135): a made-up sheet is plotted to a PDF and read
//! back as a job reads its prints; `validate.packet` compares each crop of it with the page read
//! again at the same rectangle and pixels, and flags a crop that doesn't look like its page.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Line, Space, Wipeout};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::{Value, json};

fn pdf_hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

fn folder(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-vcrops-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(s: &mut Session, p: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": p, "text": t, "height": h})).unwrap();
}

/// A 36" × 24" sheet in drawing inches: its border, a room with a door gap and a symbol, the
/// room's tag, and a block of general notes.
fn sheet() -> Session {
    let mut s = Session::new();
    s.execute("pline", &json!({"vertices": [[0, 0], [36, 0], [36, 24], [0, 24]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0.5, 0.5], [35.5, 0.5], [35.5, 23.5], [0.5, 23.5]], "closed": true})).unwrap();
    for (a, b) in
        [([4.0, 4.0], [16.0, 4.0]), ([16.0, 4.0], [16.0, 14.0]), ([16.0, 14.0], [4.0, 14.0]), ([4.0, 14.0], [4.0, 9.0]), ([4.0, 7.5], [4.0, 4.0])]
    {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }
    s.execute("circle", &json!({"center": [10, 9], "radius": 0.4})).unwrap();
    text(&mut s, [8.0, 11.0], "OFFICE 101", 0.25);
    text(&mut s, [22.0, 20.0], "GENERAL NOTES", 0.3);
    for (k, line) in ["1. MOUNT RECEPTACLES AT 18 IN AFF.", "2. ALL RECEPTACLES TAMPER-RESISTANT.", "3. SEE PANEL SCHEDULES."].iter().enumerate() {
        text(&mut s, [22.0, 19.2 - 0.6 * k as f64], line, 0.2);
    }
    s
}

struct Set {
    dir: PathBuf,
    d: Drawing,
    region: [f64; 4],
}

/// The sheet plotted to `Sample Set.pdf` in a new folder, and read back as the job reads a print.
fn printed(name: &str) -> Set {
    pdf_hooks();
    let dir = folder(name);
    let opts = json!({"layers": false, "pages": [{"window": [[0, 0], [36, 24]], "paper": "ARCH D", "landscape": true, "scale": 1.0}]});
    let bytes = cadaddy_io::pdf::plot(sheet().doc().unwrap(), &Space::Model, &opts).unwrap();
    std::fs::write(dir.join("Sample Set.pdf"), &bytes).unwrap();
    let (d, rep) = cadaddy_io::pdf_read::read_json(&bytes, &json!({"page": 1, "images": false, "fills": "merge"})).unwrap();
    let k = rep["scale"].as_f64().unwrap_or(1.0);
    let (w, h) = (rep["sheet"][0].as_f64().unwrap() * k, rep["sheet"][1].as_f64().unwrap() * k);
    Set { dir, d, region: [0.0, 0.0, w, h] }
}

/// A session on the read-back sheet: one level, the sheet as its reference plan, and the job's
/// record of where the sheet's page is (`pdfs`: the folder it names).
fn session(set: &Set, pdfs: &Path, edit: impl FnOnce(&mut Drawing, [f64; 4])) -> Session {
    let mut d = set.d.clone();
    edit(&mut d, set.region);
    let job = json!({"guided_job": {
        "id": "J-1", "goal": "bim", "created": "2026-01-01T00:00:00Z", "pdfs_path": pdfs.to_string_lossy(),
        "stages": [], "rules": [], "audit": [], "question_seq": 0,
        "sheets": {"sheets": [{"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "file": "Sample Set.pdf", "page": 1, "type": "plan"}]},
        "sheet_regions": [{"number": "A-101", "level": "L1", "rect": set.region}]
    }});
    d.agent = Some(job.to_string());
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Job", None);
    let r = set.region;
    let spec = json!({
        "units_per_foot": 1,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "base": [0, 0], "region": r}],
        "sheets": [{"number": "A-101", "title": "FLOOR PLAN - LEVEL 1", "discipline": "A", "level": "L1", "source": "model", "rect": r,
            "registration": {"reference": "A-101", "method": "reference", "a": 1.0, "b": 0.0, "tx": 0.0, "ty": 0.0, "matched": 4}}]
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s
}

/// The packet of the sheet's middle (the room and the notes beside it).
fn packet(s: &mut Session, set: &Set, out: &Path, extra: Value) -> Value {
    let r = set.region;
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    let mut p = json!({"area": {"area": [r[0] + 0.05 * w, r[1] + 0.1 * h, r[0] + 0.95 * w, r[1] + 0.9 * h], "level": "L1"}, "margin_ft": 0,
        "out_dir": out.to_string_lossy()});
    if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    s.execute("validate.packet", &p).unwrap();
    serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap()
}

fn plan(m: &Value) -> &Value {
    m["prints"].as_array().unwrap().iter().find(|c| c["sheet"] == "A-101" && c["kind"] == "plan").unwrap_or_else(|| panic!("no plan crop in {m:#}"))
}

fn add(d: &mut Drawing, kind: EntityKind) {
    let h = d.new_handle();
    d.model.push(Entity::new(h, kind));
}

#[test]
fn a_crop_is_compared_with_its_pdf_page_and_flagged_when_it_differs() {
    let set = printed("compare");
    let r = set.region;
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    // As read, plus a mask the reader merged over the notes, later than them: the crop is the page.
    let mut s = session(&set, &set.dir, |d, r| {
        let (x0, y0) = (r[0] + 0.55 * w, r[1] + 0.7 * h);
        let boundary = vec![Vec2::new(x0, y0), Vec2::new(x0 + 0.4 * w, y0), Vec2::new(x0 + 0.4 * w, y0 + 0.2 * h), Vec2::new(x0, y0 + 0.2 * h)];
        add(d, EntityKind::Wipeout(Wipeout { boundary }));
    });
    let out = folder("compare-same");
    let m = packet(&mut s, &set, &out, json!({}));
    let c = plan(&m);
    let sim = c["similarity"].as_f64().unwrap_or_else(|| panic!("no similarity: {m:#}"));
    assert!(sim >= 0.9, "the crop is the page: {sim} {c}");
    assert_eq!(c["page"], "Sample Set.pdf p.1");
    assert!(c.get("similarity_low").is_none(), "{c}");
    assert_eq!(m["similarity"]["compared"], json!(m["prints"].as_array().unwrap().len()), "{:#}", m["similarity"]);
    assert!(m["similarity"]["low"].as_array().unwrap().is_empty());

    // Lines the page doesn't print (a hatch of diagonals across the room): flagged, with the
    // number, in the manifest and the prompt.
    let mut s = session(&set, &set.dir, |d, r| {
        for k in 0..40 {
            let x = r[0] + 0.05 * w + f64::from(k) * 0.02 * w;
            add(d, EntityKind::Line(Line { a: Vec3::new(x, r[1] + 0.1 * h, 0.0), b: Vec3::new(x + 0.2 * w, r[1] + 0.9 * h, 0.0) }));
        }
    });
    let out = folder("compare-other");
    let m = packet(&mut s, &set, &out, json!({}));
    let c = plan(&m);
    let sim = c["similarity"].as_f64().unwrap();
    assert!(sim < 0.6, "lines the page doesn't have: {sim}");
    assert_eq!(c["similarity_low"], json!(true));
    assert_eq!(m["similarity"]["low"][0]["file"], c["file"]);
    let prompt = std::fs::read_to_string(out.join("prompt.md")).unwrap();
    assert!(prompt.contains("may not look like their PDF page") && prompt.contains(c["file"].as_str().unwrap()), "{prompt}");
}

#[test]
fn the_prints_folder_can_be_given_and_a_missing_page_is_said() {
    let set = printed("folder");
    // The job read its prints from a folder this machine doesn't have.
    let gone = set.dir.join("not-here");
    let mut s = session(&set, &gone, |_, _| {});
    let out = folder("folder-missing");
    let m = packet(&mut s, &set, &out, json!({}));
    let why = &m["similarity"]["not_compared"][0];
    assert_eq!(why["sheet"], "A-101");
    assert!(why["why"].as_str().unwrap().contains("Sample Set.pdf") && why["why"].as_str().unwrap().contains("`prints`"), "{why}");
    assert!(plan(&m).get("similarity").is_none());
    // Given the folder, the crop is compared; `compare: false` skips it.
    let out = folder("folder-given");
    let m = packet(&mut s, &set, &out, json!({"prints": set.dir.to_string_lossy()}));
    assert!(plan(&m)["similarity"].as_f64().is_some_and(|x| x >= 0.9), "{m:#}");
    let out = folder("folder-off");
    let m = packet(&mut s, &set, &out, json!({"prints": set.dir.to_string_lossy(), "compare": false}));
    assert_eq!(m["similarity"]["not_compared"][0]["why"], "not asked (compare: false)");
    assert!(
        s.execute("validate.packet", &json!({"area": {"area": [1, 1, 5, 5], "level": "L1"}, "out_dir": out.to_string_lossy(), "compare": 3}))
            .is_err()
    );
    assert!(
        s.execute("validate.packet", &json!({"area": {"area": [1, 1, 5, 5], "level": "L1"}, "out_dir": out.to_string_lossy(), "prints": 7})).is_err()
    );
}
