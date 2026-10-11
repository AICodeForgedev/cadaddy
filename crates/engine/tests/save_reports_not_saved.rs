//! Saving a drawing with entities of types CADCraft doesn't model (read from another program's
//! file) as DWG says which ones the file doesn't keep, instead of dropping them silently. DXF
//! writes them back as read: nothing to report.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use cadcraft_engine::doc::{Block, Common, Drawing, EntityKind, RawTag, Space, Unknown};
use serde_json::json;

fn unknown(kind: &str) -> EntityKind {
    EntityKind::Unknown(Unknown { dxf_type: kind.into(), tags: vec![RawTag { code: 8, value: "0".into() }] })
}

#[test]
fn save_lists_the_entities_the_file_does_not_keep() {
    set_io(IoHooks { read: |_, _| Err("no reader in this test".into()), write: |_, _| Ok(b"0\nEOF\n".to_vec()), plot: None });
    let mut d = Drawing::new_imperial();
    for k in ["MLINE", "3DSOLID", "MLINE"] {
        d.add(&Space::Model, Common::default(), unknown(k)).unwrap();
    }
    let mut b = Block::new("Part");
    let h = d.new_handle();
    b.entities.push(cadcraft_engine::doc::Entity { handle: h, common: Common::default(), kind: unknown("REGION") });
    d.blocks.insert("Part".into(), std::sync::Arc::new(b));
    let mut s = Session::empty();
    s.open_drawing(d, "foreign.dxf", None);

    let path = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.dwg", std::process::id()));
    let r = s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(r["notSaved"], json!({ "3DSOLID": 1, "MLINE": 2, "REGION": 1 }));
    let msg = "Not saved in the DWG file (its writer can't write these object types yet; DXF keeps them): 1 3DSOLID, 2 MLINE, 1 REGION";
    assert!(s.log.iter().any(|l| l == msg), "{:?}", s.log);

    // DXF keeps them.
    let dxf = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.dxf", std::process::id()));
    let n = s.log.len();
    let r = s.execute("saveas", &json!({ "path": dxf.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&dxf);
    assert!(r.get("notSaved").is_none() && s.log.len() == n);

    // Exporting an image isn't a save of the drawing: no report.
    let png = std::env::temp_dir().join(format!("cadcraft-not-saved-{}.png", std::process::id()));
    let r = s.execute("saveas", &json!({ "path": png.to_string_lossy() })).unwrap();
    let _ = std::fs::remove_file(&png);
    assert!(r.get("notSaved").is_none());

    // A drawing the file keeps completely: nothing to report.
    s.open_drawing(Drawing::new_imperial(), "plain.dxf", None);
    let n = s.log.len();
    let r = s.execute("document.bytes", &json!({})).unwrap();
    assert!(r.get("notSaved").is_none() && s.log.len() == n);
}

#[test]
fn templates_report_what_dwg_does_not_keep() {
    use cadcraft_engine::cmd::file::drops_unknown;
    assert!(drops_unknown("a.dwg") && drops_unknown("B.DWT"));
    assert!(!drops_unknown("a.dxf"));
}
