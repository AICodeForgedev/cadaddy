use cadaddy_doc::Drawing;
use std::collections::BTreeMap;
fn all(d: &Drawing) -> Vec<(u64, std::sync::Arc<cadaddy_doc::Entity>)> {
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).chain(d.blocks.values().map(|b| &b.entities));
    stores.flat_map(|s| s.iter().map(|e| (e.handle.0, e.clone())).collect::<Vec<_>>()).collect()
}
fn ty(e: &cadaddy_doc::Entity) -> String { serde_json::to_value(&e.kind).unwrap()["type"].as_str().unwrap_or("?").to_string() }
#[test]
fn diff() {
    let p = std::env::var("F").unwrap();
    let ver = std::env::var("V").unwrap_or("AC1018".into());
    let engine = if std::env::var("NATIVE").is_ok() { cadaddy_io::DwgEngine::Native } else { cadaddy_io::DwgEngine::Bridge };
    let bytes = std::fs::read(&p).unwrap();
    let d = cadaddy_io::read(&bytes, &p).unwrap();
    let out = cadaddy_io::write_dwg(&d, cadaddy_io::Version::for_acadver(&ver).unwrap(), engine).unwrap();
    let b = cadaddy_io::read(&out, "x.dwg").unwrap();
    let b1: std::collections::HashMap<u64, _> = all(&b).into_iter().collect();
    let mut tally: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (h, e) in all(&d) {
        let Some(f) = b1.get(&h) else { continue };
        let (x, y) = (e.common.extrusion, f.common.extrusion);
        if (x.x - y.x).abs() > 1e-9 || (x.y - y.y).abs() > 1e-9 || (x.z - y.z).abs() > 1e-9 {
            let t = tally.entry(ty(&e)).or_default();
            t.0 += 1;
            if t.1.is_empty() { t.1 = format!("{h:X} {:?} -> {:?}", x, y); }
        }
    }
    println!("{tally:#?}");
    for (h, e) in all(&d) { if let cadaddy_doc::EntityKind::MText(m) = &e.kind { let f = b1.get(&h).unwrap(); if let cadaddy_doc::EntityKind::MText(n) = &f.kind { println!("MTEXT {h:X} {:?} -> {:?}", m.contents, n.contents); } } }
}
#[test]
fn dxf_of() {
    let p = std::env::var("F").unwrap();
    let d = cadaddy_io::read(&std::fs::read(&p).unwrap(), &p).unwrap();
    let out = cadaddy_io::write_dwg(&d, cadaddy_io::Version::for_acadver("AC1018").unwrap(), cadaddy_io::DwgEngine::Bridge).unwrap();
    let (dxf, _) = cadaddy_dwg::dwg_to_dxf_noted(&out, cadaddy_dwg::Limits::DEFAULT).unwrap();
    let s = String::from_utf8_lossy(&dxf);
    let i = s.find("MTEXT").unwrap();
    println!("DXFOUT {:?}", &s[i..(i + 400).min(s.len())]);
}
