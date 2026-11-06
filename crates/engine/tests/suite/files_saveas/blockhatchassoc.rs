//! An associative hatch put into a block (BLOCK, WBLOCK, PASTEBLOCK, associative ARRAY) follows
//! its boundary to the copy when the boundary goes along, and is non-associative when it stays
//! behind; either way DXF and DWG keep it without a "not kept" note.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Hatch};
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn handle(v: &Value) -> String {
    v["handle"].as_str().unwrap().to_string()
}

/// A rectangle hatched by object: (session, hatch handle, boundary handle).
fn hatched() -> (Session, String, String) {
    let mut s = Session::new();
    let r = handle(&s.execute("rectang", &json!({"p1": [0, 0], "p2": [10, 8]})).unwrap());
    let h = handle(&s.execute("hatch", &json!({"pattern": "ANSI37", "handles": [r]})).unwrap());
    (s, h, r)
}

fn block_hatches(s: &Session, name: &str) -> Vec<(bool, usize, bool)> {
    let b = s.doc().unwrap().block(name).unwrap();
    let ids: Vec<_> = b.entities.iter().map(|e| e.handle).collect();
    b.entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Hatch(h) => Some(sourced(h, &ids)),
            _ => None,
        })
        .collect()
}

/// (associative, loops with a source, every source is an object of the same block).
fn sourced(h: &Hatch, ids: &[cadaddy_engine::doc::Handle]) -> (bool, usize, bool) {
    let n = h.loops.iter().filter(|l| !l.sources.is_empty()).count();
    (h.associative, n, h.loops.iter().flat_map(|l| &l.sources).all(|x| ids.contains(x)))
}

fn assert_kept(s: &mut Session, what: &str) {
    for f in ["dxf", "dwg"] {
        let r = s.execute("checkexchange", &json!({"format": f})).unwrap();
        assert_eq!(r["kept"], true, "{what} .{f}: {r}");
    }
}

#[test]
fn boundary_in_the_block_keeps_the_link_in_dxf_and_dwg() {
    hooks();
    for keep in ["convert", "retain", "delete"] {
        let (mut s, h, r) = hatched();
        s.execute("block", &json!({"name": "HB", "base": [0, 0], "handles": [h, r], "keep": keep})).unwrap();
        assert_eq!(block_hatches(&s, "HB"), vec![(true, 1, true)], "{keep}");
        assert_kept(&mut s, keep);
    }
}

#[test]
fn boundary_left_behind_makes_the_hatch_non_associative() {
    hooks();
    for keep in ["convert", "retain", "delete"] {
        let (mut s, h, _) = hatched();
        s.execute("block", &json!({"name": "HB", "base": [0, 0], "handles": [h], "keep": keep})).unwrap();
        assert_eq!(block_hatches(&s, "HB"), vec![(false, 0, true)], "{keep}");
        assert_kept(&mut s, keep);
    }
}

#[test]
fn hatch_by_point_in_a_block_is_kept() {
    hooks();
    let mut s = Session::new();
    let lines: Vec<String> = [([0, 0], [10, 0]), ([10, 0], [10, 8]), ([10, 8], [0, 8]), ([0, 8], [0, 0])]
        .iter()
        .map(|(a, b)| s.execute("line", &json!({"points": [a, b]})).unwrap()["handles"][0].as_str().unwrap().to_string())
        .collect();
    let h = handle(&s.execute("hatch", &json!({"pattern": "ANSI31", "points": [[5, 4]]})).unwrap());
    let mut all = lines.clone();
    all.push(h);
    s.execute("block", &json!({"name": "HP", "base": [0, 0], "handles": all, "keep": "convert"})).unwrap();
    assert_eq!(block_hatches(&s, "HP").len(), 1);
    assert_kept(&mut s, "by point");
}

#[test]
fn paste_as_block_follows_the_copied_boundary() {
    hooks();
    let (mut s, h, r) = hatched();
    s.execute("copybase", &json!({"handles": [h, r], "base": [0, 0]})).unwrap();
    let ins = handle(&s.execute("pasteblock", &json!({"at": [20, 0]})).unwrap());
    let name = match &s.doc().unwrap().entity(cadaddy_engine::doc::Handle::parse_hex(&ins).unwrap()).unwrap().kind {
        EntityKind::Insert(i) => i.block.clone(),
        k => panic!("{k:?}"),
    };
    assert_eq!(block_hatches(&s, &name), vec![(true, 1, true)]);
    assert_kept(&mut s, "pasteblock");
}
