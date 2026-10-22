//! Shapes from shape files (`.shx`): SHAPE objects and the shape elements of complex linetypes.
//!
//! A SHAPE names its shape; the shape files the drawing loaded (LOAD, `Drawing::shape_files`)
//! are searched in order for it. One unit of a shape's vectors is `size / above` drawing units
//! (`above`: the file's font height, 1 for a plain shape file); the width factor stretches X,
//! the oblique angle slants it, and the shape turns about its insertion point. A shape whose
//! file isn't found draws nothing, as in other CAD programs.

use std::sync::Arc;

use cadaddy_doc::{Drawing, Shape};
use cadaddy_fonts::shx::{self, ShxFont};
use cadaddy_geom::Vec2;

/// The loaded shape file defining `name` and its number there.
pub fn find(d: &Drawing, name: &str) -> Option<(Arc<ShxFont>, u32)> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    d.shape_files.iter().filter_map(|f| shx::find(f)).find_map(|f| f.code_by_name(name).map(|k| (f, k)))
}

/// Shape `code` of `font` as strokes: a font unit is `size / above`, X stretched by `width` and
/// slanted by `oblique` (radians), turned by `rotation` about `at`.
pub fn place(font: &ShxFont, code: u32, at: Vec2, size: f64, rotation: f64, width: f64, oblique: f64) -> Vec<Vec<Vec2>> {
    let Some(g) = font.glyph(code, false) else { return Vec::new() };
    let fin = |v: f64, or: f64| if v.is_finite() { v } else { or };
    let k = fin(size, 1.0) / font.above.max(1e-9);
    let wf = fin(width, 1.0);
    let shear = fin(oblique, 0.0).tan();
    let shear = if shear.is_finite() { shear } else { 0.0 };
    let rot = fin(rotation, 0.0);
    g.strokes
        .iter()
        .map(|l| l.iter().map(|p| at + Vec2::new((p.x + p.y * shear) * k * wf, p.y * k).rotate(rot)).collect())
        .filter(|l: &Vec<Vec2>| l.len() >= 2 && l.iter().all(|p| p.is_finite()))
        .collect()
}

/// A SHAPE object's strokes in drawing coordinates; empty when its shape isn't in a loaded file.
pub fn strokes(d: &Drawing, sh: &Shape) -> Vec<Vec<Vec2>> {
    match find(d, &sh.name) {
        Some((f, k)) => place(&f, k, sh.insert.xy(), sh.size, sh.rotation, sh.width_factor, sh.oblique),
        None => Vec::new(),
    }
}

/// The shape file a linetype's shape element names: its `style` is the file itself (an unnamed
/// shape-file style) or a text style whose font is a shape file.
pub fn linetype_file(d: &Drawing, style: &str) -> Option<Arc<ShxFont>> {
    let style = style.trim();
    if style.is_empty() {
        return None;
    }
    match d.text_style(style) {
        Some(ts) => shx::find(&ts.font),
        None => shx::find(style),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{EntityKind, Shape};
    use cadaddy_geom::Vec3;

    fn shape_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cadaddy-render-shapes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = cadaddy_fonts::shp::compile("*1,5,BOX\n014,010,01C,018,0\n").unwrap();
        std::fs::write(dir.join("madeup.shx"), &c.shx).unwrap();
        dir
    }

    #[test]
    fn a_shape_draws_from_its_loaded_file() {
        let dir = shape_dir();
        let mut d = Drawing::default();
        let sh = Shape {
            insert: Vec3::new(10.0, 5.0, 0.0),
            name: "box".into(),
            size: 2.0,
            rotation: std::f64::consts::FRAC_PI_2,
            width_factor: 1.0,
            oblique: 0.0,
        };
        // Not loaded yet: nothing.
        assert!(strokes(&d, &sh).is_empty());
        d.shape_files.push(dir.join("madeup.shx").to_string_lossy().into_owned());
        let s = strokes(&d, &sh);
        let pts = s.first().unwrap();
        // Up 2 (turned a quarter: left 2), then right 2 (turned: up 2), …
        let want = [Vec2::new(10.0, 5.0), Vec2::new(8.0, 5.0), Vec2::new(8.0, 7.0), Vec2::new(10.0, 7.0), Vec2::new(10.0, 5.0)];
        assert_eq!(pts.len(), want.len());
        for (a, b) in pts.iter().zip(want) {
            assert!(a.dist(b) < 1e-9, "{pts:?}");
        }
        // A width factor stretches X before turning; an unknown name draws nothing.
        let wide = Shape { width_factor: 2.0, rotation: 0.0, ..sh.clone() };
        assert!(strokes(&d, &wide).iter().flatten().any(|p| (p.x - 14.0).abs() < 1e-9));
        assert!(strokes(&d, &Shape { name: "nothing".into(), ..sh.clone() }).is_empty());
        // Drawn by the renderer as lines.
        d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), EntityKind::Shape(sh)).unwrap();
        let list = crate::build(&d, &cadaddy_doc::Space::Model, &crate::Options::default());
        assert_eq!(list.prims.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_shapes_never_panic() {
        let dir = shape_dir();
        let mut d = Drawing::default();
        d.shape_files.push(dir.join("madeup.shx").to_string_lossy().into_owned());
        for v in [f64::NAN, f64::INFINITY, -1e308, 0.0, 1e308] {
            let sh = Shape { insert: Vec3::new(v, v, v), name: "BOX".into(), size: v, rotation: v, width_factor: v, oblique: v };
            for l in strokes(&d, &sh) {
                assert!(l.iter().all(|p| p.is_finite()));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
