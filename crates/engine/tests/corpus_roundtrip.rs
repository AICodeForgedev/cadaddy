//! Round trips of the synthetic corpus (`tests/corpus`) through DXF, DWG and .cdy: what reading the
//! file back changes, exactly, per object kind and field. The big sizes and their timings are
//! `cargo xtask corpus`; this is a small drawing of every kind, every CI run.

mod corpus;

/// The generator keeps its files in a folder named by process and seed: one drawing of a seed at a time.
static SEED_1: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    // (Each dimension also gets the anonymous block its drawn form is written as: `.block`.)
    // The dimension text's position, worked out when written (0 in memory: not placed by hand).
    ("dxf", "Dimension Entity.kind.Dimension.text_mid"),
    ("dxf", "Dimension Entity.kind.Dimension.block"),
    ("dwg", "Dimension Entity.kind.Dimension.text_mid"),
    ("dwg", "Dimension Entity.kind.Dimension.block"),
    // The hatch pattern's lines, written out from the pattern's name.
    ("dxf", "Hatch Entity.kind.Hatch.pattern_lines"),
    ("dwg", "Hatch Entity.kind.Hatch.pattern_lines"),
    // Header variables written with their default values.
    ("dxf", "header variable(s) added"),
    ("dwg", "header variable(s) added"),
    // A second layout's viewport is numbered again (ids are per layout in a DWG).
    ("dwg", "Viewport Entity.kind.Viewport.id"),
    // CADaddy's own DWG codec (Q203): what the bridge's DWG has.
    ("dwg-native", "Dimension Entity.kind.Dimension.text_mid"),
    ("dwg-native", "Dimension Entity.kind.Dimension.block"),
    ("dwg-native", "Hatch Entity.kind.Hatch.pattern_lines"),
    ("dwg-native", "header variable(s) added"),
    ("dwg-native", "Viewport Entity.kind.Viewport.id"),
];

/// A difference line's kind: without its count and example.
fn kind(line: &str) -> &str {
    let line = line.split_once(" × ").map_or(line, |(_, rest)| rest);
    let line = line.split_once(": ").map_or(line, |(k, _)| k);
    line.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
}

#[test]
fn round_trips_report_their_differences() {
    let _one = SEED_1.lock().unwrap_or_else(|e| e.into_inner());
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

/// Changes made to a drawing read back from a DXF are each reported, as a difference no `KNOWN`
/// entry covers: the report names fields, not objects.
#[test]
fn a_changed_field_is_reported_as_new() {
    let _one = SEED_1.lock().unwrap_or_else(|e| e.into_inner());
    use cadaddy_engine::doc::{EntityKind, Space};
    let made = corpus::generate(2_000, 1);
    let mut back = corpus::read_back(&made.drawing, "dxf").unwrap();
    let spaces: Vec<Space> = std::iter::once(Space::Model).chain(back.layouts.iter().map(|l| Space::Paper(l.name.clone()))).collect();
    let (mut hatch, mut dim, mut view) = (false, false, false);
    for space in &spaces {
        let handles: Vec<_> = back.space(space).map(|s| s.iter().map(|e| e.handle).collect()).unwrap_or_default();
        for h in handles {
            let Some(store) = back.space_mut(space) else { continue };
            store.modify(h, |e| match &mut e.kind {
                EntityKind::Hatch(x) if !hatch => {
                    x.scale *= 2.0;
                    hatch = true;
                }
                EntityKind::Dimension(x) if !dim => {
                    x.style = "CHANGED".into();
                    dim = true;
                }
                EntityKind::Viewport(x) if !view => {
                    x.view_height *= 2.0;
                    view = true;
                }
                _ => {}
            });
        }
    }
    assert!(hatch && dim && view, "a hatch, a dimension and a viewport to change: {hatch} {dim} {view}");
    let mut new: Vec<String> =
        corpus::differences(&made.drawing, &back).iter().filter(|d| !KNOWN.contains(&("dxf", kind(d)))).map(|d| kind(d).to_string()).collect();
    new.sort();
    assert_eq!(
        new,
        ["Dimension Entity.kind.Dimension.style", "Hatch Entity.kind.Hatch.scale", "Viewport Entity.kind.Viewport.view_height",],
        "the three injected changes, each by its field"
    );
}
