//! Round trips of the synthetic corpus (`tests/corpus`) through DXF, DWG and .cdy: what reading the
//! file back changes, exactly, per object kind and field. The big sizes and their timings are
//! `cargo xtask corpus`; this is a small drawing of every kind, every CI run.

mod corpus;

#[test]
fn the_generator_is_seeded_and_has_every_kind() {
    let a = corpus::generate(2_000, 7);
    let b = corpus::generate(2_000, 7);
    assert_eq!(a.drawing, b.drawing, "the same seed makes the same drawing");
    assert_ne!(a.drawing, corpus::generate(2_000, 8).drawing, "another seed, another drawing");
    assert!(a.objects >= 2_000, "{} objects", a.objects);
    let kinds = corpus::kinds(&a.drawing);
    for k in ["LwPolyline", "Line", "Insert", "Hatch", "Dimension", "MText", "Table"] {
        assert!(kinds.contains_key(k), "no {k}: {kinds:?}");
    }
    assert!(a.drawing.blocks.values().any(|b| b.xref_path.is_some()), "an external reference");
    assert!(
        a.drawing.blocks.contains_key("ROOMTAG") && a.drawing.layouts.len() >= 3,
        "{:?}",
        a.drawing.layouts.iter().map(|l| &l.name).collect::<Vec<_>>()
    );
}

/// The differences each format's round trip has today, by kind (the text before the example). All
/// are a value made explicit on writing; a new kind fails, so a loss shows up the day it appears.
/// When one is fixed, take it out.
const KNOWN: &[(&str, &str)] = &[
    // The dimension text's position, worked out when written (0 in memory: not placed by hand).
    ("dxf", "Dimension Entity.kind.Dimension.text_mid"),
    ("dwg", "Dimension Entity.kind.Dimension.text_mid"),
    // The hatch pattern's lines, written out from the pattern's name.
    ("dxf", "Hatch Entity.kind.Hatch"),
    ("dwg", "Hatch Entity.kind.Hatch"),
    // Header variables written with their default values.
    ("dxf", "header variable(s) added"),
    ("dwg", "header variable(s) added"),
    // A second layout's viewport is numbered again (ids are per layout in a DWG).
    ("dwg", "Viewport Entity.kind.Viewport"),
    // CADaddy's own DWG codec (Q203): what the bridge's DWG has.
    ("dwg-native", "Dimension Entity.kind.Dimension.text_mid"),
    ("dwg-native", "Hatch Entity.kind.Hatch"),
    ("dwg-native", "header variable(s) added"),
    ("dwg-native", "Viewport Entity.kind.Viewport"),
    ("dwg-native", "layout A-102 Enlarged Plan Layout.entities.chunks.Chunk.items.Entity.kind.Viewport"),
    ("dwg", "layout A-102 Enlarged Plan Layout.entities.chunks.Chunk.items.Entity.kind.Viewport"),
];

/// A difference line's kind: without its count and example.
fn kind(line: &str) -> &str {
    let line = line.split_once(" × ").map_or(line, |(_, rest)| rest);
    let line = line.split_once(": ").map_or(line, |(k, _)| k);
    line.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
}

#[test]
fn round_trips_report_their_differences() {
    let made = corpus::generate(2_000, 1);
    let mut report = format!("corpus: {} objects, made in {:.2} s\n", made.objects, made.seconds);
    let mut new = Vec::new();
    for format in ["dxf", "dwg", "dwg-native", "cdy"] {
        let trip = corpus::round_trip(&made.drawing, format).unwrap();
        report.push_str(&format!(
            "{format}: save {:.3} s, {} KB, open {:.3} s, regen {:.3} s, {} difference(s)\n",
            trip.save,
            trip.bytes / 1024,
            trip.open,
            trip.regen,
            trip.differences.len()
        ));
        for d in &trip.differences {
            report.push_str(&format!("  {d}\n"));
            if !KNOWN.contains(&(format, kind(d))) {
                new.push(format!("{format}: {d}"));
            }
        }
    }
    eprintln!("{report}");
    assert!(new.is_empty(), "new round-trip differences:\n  {}\n\n{report}", new.join("\n  "));
}
