//! OBJECTISOLATIONMODE 1: objects hidden by ISOLATEOBJECTS / HIDEOBJECTS are saved with the
//! drawing (the `CADADDY_HIDDENOBJECTS` XRECORD) and hidden again when it is read. With
//! OBJECTISOLATIONMODE 0 nothing is written and everything opens shown.

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_geom::Vec3;
use cadaddy_io::{read, read_dxf, write, write_dxf};

/// A drawing with three lines; the first two hidden.
fn sample() -> (Drawing, Vec<Handle>) {
    let mut d = Drawing::new_imperial();
    let mut hs = Vec::new();
    for i in 0..3 {
        let h = d.new_handle();
        let y = f64::from(i);
        d.model.push(Entity {
            handle: h,
            common: Common::default(),
            kind: EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(10.0, y, 0.0) }),
        });
        hs.push(h);
    }
    d.hidden_objects.extend(hs.iter().take(2).copied());
    (d, hs)
}

#[test]
fn saved_hidden_objects_stay_hidden_in_dxf() {
    let (mut d, hs) = sample();
    d.hidden_saved = true;
    let text = write_dxf(&d);
    assert!(text.contains("CADADDY_HIDDENOBJECTS"));
    let back = read_dxf(text.as_bytes()).unwrap();
    assert!(back.hidden_saved);
    assert_eq!(back.hidden_objects.iter().copied().collect::<Vec<_>>(), hs[..2].to_vec());
    assert!(back.entity(hs[2]).is_some_and(|e| !back.is_hidden_object(e.handle)));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn saved_hidden_objects_stay_hidden_in_dwg() {
    let (mut d, hs) = sample();
    d.hidden_saved = true;
    let back = read(&write(&d, "x.dwg").unwrap(), "x.dwg").unwrap();
    assert!(back.hidden_saved);
    assert_eq!(back.hidden_objects.iter().copied().collect::<Vec<_>>(), hs[..2].to_vec());
}

#[test]
fn session_hiding_is_not_written() {
    let (d, _) = sample();
    assert!(!d.hidden_saved);
    let text = write_dxf(&d);
    assert!(!text.contains("CADADDY_HIDDENOBJECTS"));
    let back = read_dxf(text.as_bytes()).unwrap();
    assert!(back.hidden_objects.is_empty() && !back.hidden_saved);
}

#[test]
fn hostile_hidden_object_records_are_ignored() {
    let (mut d, hs) = sample();
    d.hidden_saved = true;
    let text = write_dxf(&d);
    // Handles of objects that don't exist are dropped; a payload we don't understand is ignored.
    let bogus = text.replacen(&format!("\"{}\"", hs[0].hex()), "\"FFFFFF\"", 1);
    let back = read_dxf(bogus.as_bytes()).unwrap();
    assert_eq!(back.hidden_objects.iter().copied().collect::<Vec<_>>(), vec![hs[1]]);
    let broken = text.replacen("\"version\":1", "\"version\":7", 1);
    let back = read_dxf(broken.as_bytes()).unwrap();
    assert!(back.hidden_objects.is_empty() && !back.hidden_saved);
}
