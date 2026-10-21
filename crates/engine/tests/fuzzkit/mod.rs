//! What the prompt sweeps share (`prompt_fuzz.rs`, `stuck_prompts.rs`): the commands left out,
//! a seeded generator and the drawings the commands run on.
#![allow(dead_code)]

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use serde_json::json;

/// Commands not fuzzed, and why.
pub const SKIP: &[(&str, &str)] = &[
    // The undo commands themselves move the undo stack the check counts on (UNDO/U/REDO are
    // fuzzed as input inside every other command).
    ("undo", "is the undo stack"),
    ("mredo", "is the undo stack"),
    // Writes the user's plotter configurations outside the drawing.
    ("plottermanager", "writes plotter configuration files"),
    ("-plottermanager", "writes plotter configuration files"),
    // Bakes global illumination: minutes of rendering per run.
    ("lightbake", "renders for minutes"),
];

/// A small, fast, seeded generator (SplitMix64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() { None } else { items.get(self.below(items.len())) }
    }
}

pub fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The drawings the commands run on.
pub fn drawings() -> Vec<(&'static str, fn() -> Session)> {
    vec![
        ("empty", Session::new as fn() -> Session),
        ("bracket", || {
            let mut s = Session::new();
            s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
            s
        }),
        ("mixed", || {
            // Lines, a closed polyline, a circle, an arc and text typed as a user would; a block
            // and its insert, a dimension and a hatch through their JSON forms.
            let mut s = Session::new();
            let _ = s.script("LINE 0,0 10,0 10,6 \nPLINE 12,0 20,0 20,6 12,6 C\nCIRCLE 5,12 3\nARC 14,10 17,13 20,10\nTEXT 0,-4 1 0 Fuzz label\n\n");
            let mark = s.execute("circle", &json!({ "center": [25, 0], "radius": 1 })).ok().and_then(|v| v["handle"].as_str().map(str::to_string));
            let _ = s.execute("block", &json!({ "name": "Mark", "base": [25, 0], "handles": [mark] }));
            let _ = s.execute("insert", &json!({ "name": "Mark", "at": [30, 5] }));
            let _ = s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] }));
            let _ = s.execute("hatch", &json!({ "points": [[16, 3]], "pattern": "ANSI31" }));
            s
        }),
    ]
}

/// The objects of the current space.
pub fn handles(s: &Session) -> Vec<Handle> {
    let space = s.space();
    s.doc().ok().and_then(|d| d.space(&space)).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default()
}
