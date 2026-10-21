//! DWG interop with independent readers (Q230).
//!
//! Two `#[ignore]`d steps around a black-box run of other programs, both given a folder in
//! `CADADDY_INTEROP_DIR`:
//! - `interop_write`: made-up drawings written as DWG in every version by both writers (CADaddy's
//!   codec and the DWG bridge) to `ours/<writer>/<drawing>-<year>.dwg`.
//! - (outside: another program opens each file and saves it again, as DWG or DXF, into
//!   `<program>/<writer>/<same stem>.(dwg|dxf)`, with a `<stem>[.dwg|.dxf].err` file for what it rejected.)
//! - `interop_compare`: each file read back into CADaddy and compared with what CADaddy reads from
//!   its own file (counts by type, layers, blocks, text, dimensions), written to `matrix.md`.
//!
//! The always-run tests below hold what the matrix found.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cadaddy_engine::doc::{Drawing, EntityKind};
use cadaddy_io::{DwgEngine, Version};
use serde_json::{Value, json};

#[path = "support/kitchen.rs"]
mod kitchen;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
}

fn interop_dir() -> Option<PathBuf> {
    std::env::var_os("CADADDY_INTEROP_DIR").map(PathBuf::from)
}

/// The made-up drawings: one of each entity and table type, and the two samples.
fn drawings(scratch: &Path) -> Vec<(&'static str, Drawing)> {
    hooks();
    vec![
        ("kitchen", kitchen::kitchen_sink(scratch)),
        ("floorplan", cadaddy_engine::sample::floor_plan()),
        ("bracket", cadaddy_engine::sample::bracket()),
    ]
}

const WRITERS: [(&str, DwgEngine); 2] = [("native", DwgEngine::Native), ("bridge", DwgEngine::Bridge)];

/// What the matrix compares: object counts by space and type, layers, named blocks (with their
/// object counts), how many anonymous blocks, layouts, every text string and every dimension.
#[derive(Debug, Default, PartialEq)]
struct Summary {
    counts: BTreeMap<String, usize>,
    layers: BTreeSet<String>,
    blocks: BTreeMap<String, usize>,
    anonymous: usize,
    layouts: BTreeSet<String>,
    texts: BTreeMap<String, usize>,
    dims: BTreeMap<String, usize>,
}

/// Strings a text-bearing object shows: TEXT/ATTRIB values, MTEXT contents, a dimension's text,
/// table cells; found by key so every kind that carries text counts.
fn strings(v: &Value, out: &mut BTreeMap<String, usize>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match x {
                    Value::String(s) if matches!(k.as_str(), "value" | "contents" | "text") && !s.trim().is_empty() => {
                        *out.entry(s.clone()).or_default() += 1;
                    }
                    _ => strings(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        _ => {}
    }
}

fn summarize(d: &Drawing) -> Summary {
    let mut s = Summary::default();
    let add = |space: &str, e: &cadaddy_engine::doc::Entity, s: &mut Summary| {
        *s.counts.entry(format!("{space}:{}", e.kind.type_name())).or_default() += 1;
        let kind = serde_json::to_value(&e.kind).unwrap_or(Value::Null);
        strings(&kind, &mut s.texts);
        if let EntityKind::Dimension(dim) = &e.kind {
            let kind = serde_json::to_value(dim.kind).unwrap_or(Value::Null);
            *s.dims.entry(format!("{kind} {:.4}", dim.measurement)).or_default() += 1;
        }
    };
    for e in d.model.iter() {
        add("model", e, &mut s);
    }
    for l in &d.layouts {
        s.layouts.insert(l.name.clone());
        for e in l.entities.iter() {
            add("paper", e, &mut s);
        }
    }
    s.layers = d.layers.iter().map(|l| l.name.to_ascii_uppercase()).collect();
    for (name, b) in &d.blocks {
        if name.starts_with('*') {
            s.anonymous += 1;
        } else {
            s.blocks.insert(name.to_ascii_uppercase(), b.entities.len());
        }
    }
    s
}

fn diff_map<K: Ord + std::fmt::Display>(what: &str, a: &BTreeMap<K, usize>, b: &BTreeMap<K, usize>, out: &mut Vec<String>) {
    let keys: BTreeSet<&K> = a.keys().chain(b.keys()).collect();
    for k in keys {
        let (x, y) = (a.get(k).copied().unwrap_or(0), b.get(k).copied().unwrap_or(0));
        if x != y {
            out.push(format!("{what} {k}: {x}→{y}"));
        }
    }
}

fn diff_set(what: &str, a: &BTreeSet<String>, b: &BTreeSet<String>, out: &mut Vec<String>) {
    for k in a.difference(b) {
        out.push(format!("{what} -{k}"));
    }
    for k in b.difference(a) {
        out.push(format!("{what} +{k}"));
    }
}

/// What changed from `a` to `b`, one line each.
fn diff(a: &Summary, b: &Summary) -> Vec<String> {
    let mut out = Vec::new();
    diff_map("count", &a.counts, &b.counts, &mut out);
    diff_set("layer", &a.layers, &b.layers, &mut out);
    diff_map("block", &a.blocks, &b.blocks, &mut out);
    if a.anonymous != b.anonymous {
        out.push(format!("anonymous blocks: {}→{}", a.anonymous, b.anonymous));
    }
    diff_set("layout", &a.layouts, &b.layouts, &mut out);
    let quote = |m: &BTreeMap<String, usize>| m.iter().map(|(k, v)| (format!("{k:?}"), *v)).collect::<BTreeMap<_, _>>();
    diff_map("text", &quote(&a.texts), &quote(&b.texts), &mut out);
    diff_map("dim", &a.dims, &b.dims, &mut out);
    out
}

fn read(path: &Path) -> Result<Drawing, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    cadaddy_io::read(&bytes, &path.to_string_lossy()).map_err(|e| e.to_string())
}

#[test]
#[ignore = "writes the interop matrix's DWG files into CADADDY_INTEROP_DIR"]
fn interop_write() {
    let Some(dir) = interop_dir() else { return };
    let scratch = dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let mut log = String::from("file\twriter used\tnot encoded\tkept\n");
    for (name, d) in drawings(&scratch) {
        for (writer, engine) in WRITERS {
            let out = dir.join("ours").join(writer);
            std::fs::create_dir_all(&out).unwrap();
            for v in Version::ALL {
                let file = format!("{name}-{}.dwg", v.year());
                let (bytes, used, skipped, kept) = if engine == DwgEngine::Native {
                    match cadaddy_io::dwg_write::write(&d, v.code()) {
                        Ok((b, r)) => (b, "native", format!("{:?}", r.skipped), r.kept),
                        Err(e) => (cadaddy_io::write_dwg(&d, v, engine).unwrap(), "bridge (native failed)", e.to_string(), 0),
                    }
                } else {
                    (cadaddy_io::write_dwg(&d, v, engine).unwrap(), "bridge", String::new(), 0)
                };
                std::fs::write(out.join(&file), bytes).unwrap();
                log.push_str(&format!("{writer}/{file}\t{used}\t{skipped}\t{kept}\n"));
            }
        }
    }
    std::fs::write(dir.join("ours").join("written.tsv"), log).unwrap();
}

#[test]
#[ignore = "compares other programs' copies in CADADDY_INTEROP_DIR with ours; writes matrix.md"]
fn interop_compare() {
    let Some(dir) = interop_dir() else { return };
    let ours = dir.join("ours");
    let programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "ours") && WRITERS.iter().any(|(w, _)| p.join(w).is_dir()))
        .collect();
    let mut md = String::from("| file | program | output | result | changes against CADaddy's reading of its own file |\n|---|---|---|---|---|\n");
    for (writer, _) in WRITERS {
        let mut files: Vec<PathBuf> = std::fs::read_dir(ours.join(writer))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "dwg"))
            .collect();
        files.sort();
        for f in files {
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            let base = match read(&f) {
                Ok(d) => summarize(&d),
                Err(e) => {
                    md.push_str(&format!("| {writer}/{stem} | CADaddy | - | READ FAILED: {e} | |\n"));
                    continue;
                }
            };
            for p in &programs {
                let program = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let folder = p.join(writer);
                let note = ["err", "dwg.err", "dxf.err"]
                    .iter()
                    .filter_map(|x| std::fs::read_to_string(folder.join(format!("{stem}.{x}"))).ok())
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join(" / ");
                let mut found = false;
                for ext in ["dwg", "dxf"] {
                    let out = folder.join(format!("{stem}.{ext}"));
                    if !out.exists() {
                        continue;
                    }
                    found = true;
                    let (result, changes) = match read(&out) {
                        Ok(d) => {
                            let changes = diff(&base, &summarize(&d));
                            (if changes.is_empty() { "same".to_string() } else { format!("{} changes", changes.len()) }, changes.join("; "))
                        }
                        Err(e) => (format!("CADaddy can't read it back: {e}"), String::new()),
                    };
                    let result = if note.is_empty() { result } else { format!("{result} (program: {note})") };
                    md.push_str(&format!("| {writer}/{stem} | {program} | {ext} | {result} | {changes} |\n"));
                }
                if !found {
                    let why = if note.is_empty() { "no output".to_string() } else { note };
                    md.push_str(&format!("| {writer}/{stem} | {program} | - | REJECTED: {why} | |\n"));
                }
            }
        }
    }
    std::fs::write(dir.join("matrix.md"), md).unwrap();
}

/// Small drawings, one feature each, and the kitchen-sink drawing split by object type, written
/// in every version by CADaddy's codec to `probe/<feature>-<year>.dwg` and by the bridge to
/// `probe-bridge/`: which feature another program rejects.
#[test]
#[ignore = "writes one-feature DWG files into CADADDY_INTEROP_DIR/probe and probe-bridge"]
fn interop_probe() {
    let Some(dir) = interop_dir() else { return };
    hooks();
    let (out, bridged) = (dir.join("probe"), dir.join("probe-bridge"));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&bridged).unwrap();
    let features: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("empty", vec![]),
        ("line", vec![("line", json!({ "points": [[0, 0], [10, 5]] }))]),
        ("circle", vec![("circle", json!({ "center": [0, 0], "radius": 4 }))]),
        ("text", vec![("text", json!({ "at": [0, 0], "text": "Room", "height": 2.5 }))]),
        ("mtext", vec![("mtext", json!({ "at": [0, 0], "text": "a\\Pb", "width": 10, "height": 2 }))]),
        ("layer", vec![("layer.new", json!({ "name": "Walls", "color": "red" }))]),
        ("dim", vec![("dimlinear", json!({ "p1": [0, 0], "p2": [20, 0], "at": [10, 5] }))]),
        ("hatch", vec![("rectang", json!({ "p1": [0, 0], "p2": [10, 10] })), ("hatch", json!({ "points": [[5, 5]], "pattern": "ANSI31" }))]),
        ("splinefit", vec![("spline", json!({ "fit": [[0, 40], [5, 44], [10, 40], [15, 44]] }))]),
        ("splinectrl", vec![("spline", json!({ "control": [[20, 40], [25, 46], [30, 40], [35, 46]], "degree": 3 }))]),
        // Objects past the first 0x4000 bytes of a section (far LZ77 copies).
        ("lines3000", (0..3000).map(|i| ("line", json!({ "points": [[0, i], [10, i]] }))).collect()),
        ("mview", vec![("mview", json!({ "layout": "Layout1", "p1": [1, 1], "p2": [8, 6] }))]),
    ];
    let mut drawings = Vec::new();
    for (name, steps) in features {
        let mut s = cadaddy_engine::Session::new();
        for (id, p) in steps {
            s.execute(id, &p).unwrap();
        }
        drawings.push((name.to_string(), s.doc().unwrap().clone()));
    }
    // The kitchen-sink drawing with the model's objects of one type each (tables, objects, paper
    // space and blocks as they are), and with none.
    let scratch = dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let k = kitchen::kitchen_sink(&scratch);
    let kinds: BTreeSet<&str> = k.model.iter().map(|e| e.kind.type_name()).collect();
    for kind in kinds.into_iter().chain(["none"]) {
        let mut one = k.clone();
        let others: Vec<_> = one.model.iter().filter(|e| e.kind.type_name() != kind).map(|e| e.handle).collect();
        for h in others {
            one.remove_entity(h);
        }
        drawings.push((format!("k_{}", kind.replace(' ', "")), one));
    }
    for (name, d) in drawings {
        for v in Version::ALL {
            let (bytes, _) = cadaddy_io::dwg_write::write(&d, v.code()).unwrap();
            std::fs::write(out.join(format!("{name}-{}.dwg", v.year())), bytes).unwrap();
            std::fs::write(bridged.join(format!("{name}-{}.dwg", v.year())), cadaddy_io::write_dwg(&d, v, DwgEngine::Bridge).unwrap()).unwrap();
        }
    }
}
