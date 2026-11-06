//! The RFI list (Q296): the print readers' items in the building spec (with their sheets) and
//! the spec's own gaps, `rfi.list` filters, `rfi.zoom` to an item, `rfi.set` tracks one in the
//! spec, `rfi.export` writes CSV, XLSX and PDF with pictures of the built model.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn sq(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    json!([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn board(m: &str) -> Value {
    json!({"material": m, "thickness_in": 0.625, "category": "board"})
}

/// A made-up 40' × 24' building (inches): a toilet room under a two-layer rated ceiling and an
/// office with a tile ceiling, a partition between them whose tag is missing (its type guessed:
/// rated, but only 6" above the ceiling, and no head of wall detail), and two items the print
/// readers found (a tag not in the legend, a sleeve with only a size).
fn building() -> Session {
    hooks();
    let mut s = Session::new();
    if s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    }
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    s.execute("line", &json!({"points": [[237.5, t], [237.5, 288.0 - t]]})).unwrap();
    s.execute("line", &json!({"points": [[242.5, t], [242.5, 288.0 - t]]})).unwrap();
    let head = json!({"track": "deflection", "to": "deck", "deflection_in": 0.75, "bottom_track": true, "detail": "5"});
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": {"tag": "X1", "height": "full", "external": true, "head": head,
                "layers": [board("5/8\" Type X gypsum board")]}},
            {"name": "P1", "thickness_in": 5.0, "from": "pairs", "assembly": {"tag": "P1", "height": "above_ceiling", "fire_rating": "1 HR",
                "layers": [board("5/8\" Type X gypsum board"), board("5/8\" Type X gypsum board")]}}
        ],
        "walls": [
            {"level": "1", "a": [0, 3.625], "b": [480, 3.625], "wall_type": "X1"},
            {"level": "1", "a": [0, 284.375], "b": [480, 284.375], "wall_type": "X1"},
            {"level": "1", "a": [240, 7.25], "b": [240, 280.75], "wall_type": "P1", "guess": "no tag: the commonest interior type that thick"}
        ],
        "ceiling_types": [
            {"name": "GYP-2", "kind": "gypsum", "fire_rating": "1 HR", "layers": [board("5/8\" moisture-resistant gypsum board"), board("5/8\" Type X gypsum board")]},
            {"name": "ACT-1", "kind": "grid", "grid_in": [24, 48], "layers": [{"material": "5/8\" acoustic ceiling tile", "thickness_in": 0.625, "category": "tile"}]}
        ],
        "rcp": {"ceilings": [
            {"level": "1", "room": "TOILET", "outline": sq(7.25, 7.25, 240.0, 280.75), "height_ft": 8, "ceiling_type": "GYP-2"},
            {"level": "1", "room": "OFFICE", "outline": sq(240.0, 7.25, 472.75, 280.75), "height_ft": 9, "ceiling_type": "ACT-1", "guess": "not scheduled: a tile grid on the RCP"}
        ]},
        "rfis": [
            {"kind": "unknown_tag", "severity": "high", "level": "1", "at": [120, 3.625], "extent": [[0, 0], [240, 7.25]],
             "sheets": ["A-101", "A-501"], "about": "wall of room 101 (outside)", "issue": "The plan tags this wall Q9, which the wall type legend doesn't have.",
             "assumed": "Type X1 (no tag: on the building's outline, the exterior type)."},
            {"kind": "sleeve_no_route", "severity": "high", "level": "1", "at": [360, 144], "sheets": ["A-101"], "about": "sleeve: 3\" SLEEVE",
             "issue": "\"3\" SLEEVE\" gives no route or destination, no method.", "assumed": "Not routed.", "proposed": "Say where it runs to, and how."}
        ]
    });
    let r = s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn kinds(r: &Value) -> Vec<String> {
    r["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn list_has_the_readers_items_and_the_specs_own_gaps() {
    let mut s = building();
    let r = s.execute("rfi.list", &json!({})).unwrap();
    let k = kinds(&r);
    for want in [
        "unknown_tag",
        "sleeve_no_route",
        "untagged_wall",
        "rated_to_ceiling",
        "double_ceiling_short_wall",
        "head_unspecified",
        "ceiling_unspecified",
    ] {
        assert!(k.iter().any(|x| x == want), "{want} in {k:?}");
    }
    // Most severe first; the readers' items keep their sheets.
    let sev: Vec<&str> = r["items"].as_array().unwrap().iter().map(|i| i["severity"].as_str().unwrap()).collect();
    let rank = |s: &str| ["high", "medium", "low"].iter().position(|x| *x == s).unwrap();
    assert!(sev.windows(2).all(|w| rank(w[0]) <= rank(w[1])), "{sev:?}");
    let tag = r["items"].as_array().unwrap().iter().find(|i| i["kind"] == "unknown_tag").unwrap();
    assert_eq!(tag["sheets"], json!(["A-101", "A-501"]));
    assert!(tag["id"].as_str().unwrap().starts_with("RFI-"));
    assert_eq!(r["total"].as_u64().unwrap() as usize, k.len());
    assert!(r["bySeverity"]["high"].as_u64().unwrap() >= 3, "{}", r["bySeverity"]);
    assert!(r["csv"].as_str().unwrap().starts_with("RFI,Severity,Kind"));
    // Filters.
    let high = s.execute("rfi.list", &json!({"severity": "high"})).unwrap();
    assert!(high["items"].as_array().unwrap().iter().all(|i| i["severity"] == "high"));
    let one = s.execute("rfi.list", &json!({"kind": "rated_to_ceiling"})).unwrap();
    assert_eq!(one["total"], 1);
    assert!(one["items"][0]["issue"].as_str().unwrap().contains("1 HR"), "{one}");
    assert!(s.execute("rfi.list", &json!({"kind": "nonsense"})).is_err());
    assert!(s.execute("rfi.list", &json!({"severity": "urgent"})).is_err());
}

#[test]
fn zoom_and_track_an_item() {
    let mut s = building();
    let r = s.execute("rfi.list", &json!({"kind": "double_ceiling_short_wall"})).unwrap();
    let id = r["items"][0]["id"].as_str().unwrap().to_string();
    let z = s.execute("rfi.zoom", &json!({"id": id})).unwrap();
    let w: Vec<f64> = z["window"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    // The wall's extent with 6' round it.
    assert!(w[0] <= 240.0 - 72.0 + 1e-6 && w[2] >= 240.0 + 72.0 - 1e-6, "{w:?}");
    // Track it: kept in the spec, so it stays after the list is made again.
    let set = s.execute("rfi.set", &json!({"id": id, "status": "sent", "answer": "Wall runs to the deck."})).unwrap();
    assert_eq!(set["item"]["status"], "sent");
    let sent = s.execute("rfi.list", &json!({"status": "sent"})).unwrap();
    assert_eq!(sent["total"], 1);
    assert_eq!(sent["items"][0]["answer"], "Wall runs to the deck.");
    assert_eq!(sent["items"][0]["id"], id.as_str());
    let all = s.execute("rfi.list", &json!({})).unwrap();
    assert_eq!(all["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "double_ceiling_short_wall").count(), 1);
    // Items about the whole set have no place to zoom to; unknown ids and values are refused.
    assert!(s.execute("rfi.zoom", &json!({"id": "RFI-999"})).is_err());
    assert!(s.execute("rfi.set", &json!({"id": id, "status": "lost"})).is_err());
    assert!(s.execute("rfi.zoom", &json!({})).is_err());
}

#[test]
fn export_csv_xlsx_and_pdf_with_pictures_of_the_model() {
    let mut s = building();
    let dir = std::env::temp_dir().join(format!("cadaddy-rfi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().to_string();
    let total = s.execute("rfi.list", &json!({})).unwrap()["total"].as_u64().unwrap();

    let c = s.execute("rfi.export", &json!({"path": path("rfis.csv")})).unwrap();
    assert_eq!(c["format"], "csv");
    assert_eq!(c["pictures"], 0);
    let csv = std::fs::read_to_string(path("rfis.csv")).unwrap();
    assert_eq!(csv.lines().count() as u64, total + 1);

    let x = s.execute("rfi.export", &json!({"path": path("rfis.xlsx")})).unwrap();
    assert_eq!(x["items"], total);
    assert!(x["pictures"].as_u64().unwrap() > 0, "{x}");
    let xb = std::fs::read(path("rfis.xlsx")).unwrap();
    assert!(xb.starts_with(b"PK"));

    let p = s.execute("rfi.export", &json!({"path": path("rfis.pdf")})).unwrap();
    assert!(p["pictures"].as_u64().unwrap() > 0, "{p}");
    let pb = std::fs::read(path("rfis.pdf")).unwrap();
    assert!(pb.starts_with(b"%PDF"));
    // A picture of the model at an item on the level: a PNG of the built model.
    let shots: Vec<_> = std::fs::read_dir(dir.join("rfis-pictures")).unwrap().filter_map(|e| e.ok()).collect();
    assert!(!shots.is_empty());
    let png = std::fs::read(shots[0].path()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // Without pictures; refused formats.
    let np = s.execute("rfi.export", &json!({"path": path("plain.pdf"), "pictures": false})).unwrap();
    assert_eq!(np["pictures"], 0);
    assert!(s.execute("rfi.export", &json!({"path": path("rfis.doc")})).is_err());
    assert!(s.execute("rfi.export", &json!({})).is_err());
    if std::env::var("KEEP_RFI_EXPORT").is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    } else {
        eprintln!("kept {}", dir.display());
    }
}

#[test]
fn hostile_items_never_panic() {
    let mut s = building();
    let bad = json!({"rfis": [
        {"kind": "sleeve_no_fit", "level": "1", "at": [1e308, -1e308], "extent": [[1e308, 0], [-1e308, 0]], "issue": "x"},
        {"level": "", "at": [0, 0], "issue": "no kind"},
        {"kind": "unknown_tag", "level": "nowhere", "at": [0, 0], "issue": "y", "id": "RFI-001"}
    ]});
    let _ = s.execute("bspec.set", &json!({"spec": bad}));
    let r = s.execute("rfi.list", &json!({})).unwrap();
    for i in r["items"].as_array().unwrap() {
        let id = i["id"].as_str().unwrap();
        let _ = s.execute("rfi.zoom", &json!({"id": id}));
        let _ = s.execute("rfi.set", &json!({"id": id, "answer": "x".repeat(10_000)}));
    }
    let dir = std::env::temp_dir().join(format!("cadaddy-rfi-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _ = s.execute("rfi.export", &json!({"path": dir.join("h.pdf").to_string_lossy()}));
    let _ = std::fs::remove_dir_all(&dir);
}
