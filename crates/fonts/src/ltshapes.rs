//! CADaddy's own shape file for complex linetypes (`cadaddy.shx`): the shapes the linetypes it
//! ships draw (a box, a circle, a track tie, a zigzag, batting loops, a flow arrow, a cross). It
//! is built in, compiled from the definitions below with [`crate::shx::compile_shapes`]; a drawing
//! that names `cadaddy.shx` draws them wherever it is opened. Our own designs in the public shape
//! definition codes: nothing here comes from another program's shape files.
//!
//! Each shape is centred on its anchor (where its linetype element puts it), 10 font units to one
//! unit of size: at a linetype element's scale `S`, a box is `S` wide.

use std::sync::{Arc, OnceLock};

use crate::shx::{ShxFont, compile_shapes};

/// The file name drawings use for the built-in shapes.
pub const FILE: &str = "cadaddy.shx";

/// Font units per unit of shape size.
const ABOVE: u8 = 10;

/// A signed byte, as shape codes take displacements.
const fn b(v: i8) -> u8 {
    v as u8
}

/// The shapes: number, name, definition (codes: 1 pen down, 2 pen up, 8 one displacement, 9
/// displacements to (0,0), 10 octant arc, 12 bulge arc, `0xLD` a vector of length L in direction
/// D of 16, 0 the end).
pub const SHAPES: &[(u16, &str, &[u8])] = &[
    // A square, 1 wide.
    (1, "BOX", &[2, 8, b(-5), b(-5), 1, 0xA0, 0xA4, 0xA8, 0xAC, 2, 8, 5, 5, 0]),
    // A circle, 1 across: from its east point, eight octants counterclockwise about the anchor.
    (2, "CIRC", &[2, 8, 5, 0, 1, 10, 5, 0x00, 2, 8, b(-5), 0, 0]),
    // A tie across the line, 1 long (railway tracks).
    (3, "TRACK", &[2, 8, 0, b(-5), 1, 0xA4, 2, 8, 0, b(-5), 0]),
    // One zigzag, 0.8 long and 0.8 high.
    (4, "ZIG", &[2, 8, b(-4), 0, 1, 9, 2, 4, 4, b(-8), 2, 4, 0, 0, 2, b(8), b(-4), 0, 0]),
    // An S across the line, 1 high: two half loops bulging opposite ways (insulation batting).
    (5, "BAT", &[2, 8, 0, b(-5), 1, 12, 0, 5, 127, 12, 0, 5, b(-127), 2, 8, 0, b(-5), 0]),
    // A chevron pointing along the line (flow direction).
    (6, "ARROW", &[2, 8, b(-3), b(-3), 1, 8, 3, 3, 8, b(-3), 3, 2, 8, 3, b(-3), 0]),
    // An X, 0.6 across.
    (7, "CROSS", &[2, 8, b(-3), b(-3), 1, 8, 6, 6, 2, 8, b(-6), 0, 1, 8, 6, b(-6), 2, 8, b(-3), 3, 0]),
];

/// The built-in shape file, compiled once.
pub fn builtin() -> Arc<ShxFont> {
    static FONT: OnceLock<Arc<ShxFont>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = compile_shapes(ABOVE, 0, SHAPES);
        Arc::new(ShxFont::parse(&bytes).unwrap_or_else(ShxFont::empty_shapes))
    })
    .clone()
}

/// Whether a style's font (or a linetype shape's file) names the built-in shapes, with or without
/// a folder or the `.shx` extension.
pub fn is_builtin(name: &str) -> bool {
    let n = name.trim().rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
    n == FILE || n == "cadaddy"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_compiles_draws_and_is_named() {
        let f = builtin();
        for (n, name, _) in SHAPES {
            assert_eq!(f.shape_number(name), Some(u32::from(*n)), "{name}");
            assert_eq!(f.shape_name(u32::from(*n)), Some(*name));
            let g = f.glyph(u32::from(*n), false).unwrap();
            assert!(!g.strokes.is_empty(), "{name} draws");
            // Centred on the anchor, within a unit of size (10 font units), and the pen comes back.
            for p in g.strokes.iter().flatten() {
                assert!(p.x.abs() <= 5.01 && p.y.abs() <= 5.01, "{name}: {p:?}");
            }
            assert!(g.advance.len() < 1e-9, "{name} ends where it began: {:?}", g.advance);
        }
        // The box is a closed square 10 units across.
        let bx = f.glyph(1, false).unwrap();
        let pts: Vec<_> = bx.strokes.iter().flatten().collect();
        let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.x), hi.max(p.x)));
        assert_eq!((lo, hi), (-5.0, 5.0));
        assert!(is_builtin("cadaddy.shx") && is_builtin("C:\\fonts\\CADaddy.SHX") && is_builtin("cadaddy") && !is_builtin("ltypeshp.shx"));
        assert!(crate::shx::find("cadaddy.shx").is_some_and(|f| f.shape_number("track") == Some(3)));
    }
}
