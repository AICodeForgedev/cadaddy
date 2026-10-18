//! Associative arrays: the parameters an array keeps, and where they place its items.
//!
//! An associative array is a block reference to an anonymous block (`*U…`, the *array block*)
//! that holds one block reference per item. Each item refers to the array's *source block*
//! (another anonymous block holding the source objects) or, for items replaced with ARRAYEDIT,
//! to another block. Other programs see an ordinary block reference; CADaddy keeps the
//! parameters below on the array block ([`Block::array`](crate::Block)) so the items can be
//! regenerated when the array is edited.
//!
//! All positions are in the array block's coordinates: its origin is the array's base point,
//! which is where the array's block reference is inserted.

use cadaddy_geom::{Polyline, Segment, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::{Common, Drawing, Entity, EntityKind, Insert, Lineweight};

/// Most items one array may have (items × rows × levels), whatever MAXARRAY says.
pub const MAX_ARRAY_ITEMS: u64 = 10_000_000;

fn one() -> u32 {
    1
}

/// The parameters of an associative array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrayParams {
    /// The source block the items refer to.
    pub source: String,
    pub kind: ArrayKind,
    /// Levels: copies of the whole array along Z.
    #[serde(default = "one")]
    pub levels: u32,
    #[serde(default)]
    pub level_spacing: f64,
    /// Items that refer to another block than the source (ARRAYEDIT REPlace).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<ReplacedItem>,
    /// Items taken out of the array (ARRAYEDIT, Ctrl+pick and Erase): (i, j, level).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<[u32; 3]>,
    /// The source objects are out for editing (ARRAYEDIT Source, until ARRAYCLOSE). Not saved:
    /// a drawing saved meanwhile keeps the objects as separate objects.
    #[serde(skip)]
    pub editing: Option<SourceEdit>,
}

/// ARRAYEDIT Source in progress: the array's source objects placed in the drawing for editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceEdit {
    /// The array (block reference) being edited.
    pub array: crate::Handle,
    /// The item the objects were placed at: (i, j, level).
    pub item: (u32, u32, u32),
    /// The objects placed for editing.
    pub objects: Vec<crate::Handle>,
    /// Objects with this handle or above were drawn while editing; they join the source too.
    pub first_new: u64,
}

/// Rectangular or polar placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ArrayKind {
    Rect(RectArray),
    Polar(PolarArray),
    Path(PathArray),
}

/// A rectangular array: columns along the axis angle, rows across it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RectArray {
    pub cols: u32,
    pub rows: u32,
    pub col_spacing: f64,
    pub row_spacing: f64,
    /// Direction of the rows (the column axis), radians.
    #[serde(default)]
    pub axis_angle: f64,
    /// Elevation added per row.
    #[serde(default)]
    pub row_elevation: f64,
}

/// A polar array: items around a centre, rows as rings further out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolarArray {
    pub center: Vec2,
    pub items: u32,
    /// Angle to fill, degrees (positive = counterclockwise, ±360 = a full circle).
    pub fill: f64,
    /// Items are rotated as they go around (else they keep their orientation).
    pub rotate: bool,
    #[serde(default = "one")]
    pub rows: u32,
    /// Distance between rings, outwards from the centre.
    #[serde(default)]
    pub row_spacing: f64,
    #[serde(default)]
    pub row_elevation: f64,
}

/// A path array: items along a curve, rows offset to its left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathArray {
    /// The path curve in the drawing; the array follows it when it is edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<crate::Handle>,
    /// The path in the array block's coordinates (arcs as bulges).
    pub path: Polyline,
    /// Items along the path (with `divide` exactly this many; else at most this many).
    pub items: u32,
    /// Distance between items along the path (Measure).
    pub spacing: f64,
    /// Divide: the items spread evenly over the whole path, whatever its length.
    #[serde(default)]
    pub divide: bool,
    /// Items turn with the path.
    pub align: bool,
    /// The path's direction where the source objects are (radians): items are turned by the
    /// path's direction minus this.
    #[serde(default)]
    pub ref_angle: f64,
    #[serde(default = "one")]
    pub rows: u32,
    #[serde(default)]
    pub row_spacing: f64,
    #[serde(default)]
    pub row_elevation: f64,
}

/// Most vertices a path array's path keeps.
pub const MAX_PATH_VERTICES: usize = 100_000;

impl PathArray {
    fn segments(&self) -> Vec<Segment> {
        self.path.segments()
    }

    /// The path's length.
    pub fn length(&self) -> f64 {
        self.segments().iter().map(Segment::len).sum()
    }

    /// The items along the path and the distance between them, for a path of length `len`.
    pub fn count_and_step(&self, len: f64) -> (u32, f64) {
        if self.divide {
            let gaps = if self.path.closed { self.items } else { self.items.saturating_sub(1) };
            (self.items, if gaps == 0 { 0.0 } else { len / f64::from(gaps) })
        } else if self.spacing > 0.0 {
            let fit = (len / self.spacing + 1e-9).floor();
            let fit = if fit.is_finite() && fit >= 0.0 { fit.min(f64::from(u32::MAX - 1)) as u32 + 1 } else { 1 };
            (self.items.min(fit).max(1), self.spacing)
        } else {
            (1, 0.0)
        }
    }

    /// Point and unit tangent at distance `d` along the path (clamped to its ends).
    pub fn at_length(&self, d: f64) -> Option<(Vec2, Vec2)> {
        let segs = self.segments();
        at_length(&segs, d)
    }
}

fn at_length(segs: &[Segment], d: f64) -> Option<(Vec2, Vec2)> {
    let mut acc = 0.0;
    for s in segs {
        let l = s.len();
        if d <= acc + l + 1e-12 && l > 0.0 {
            let t = ((d - acc) / l).clamp(0.0, 1.0);
            return Some((s.at(t), s.tangent(t)));
        }
        acc += l;
    }
    segs.last().map(|s| (s.end(), s.tangent(1.0)))
}

/// An item that refers to another block. `i` is the column (rectangular) or the item around the
/// circle (polar), `j` the row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacedItem {
    pub i: u32,
    pub j: u32,
    #[serde(default)]
    pub level: u32,
    pub block: String,
}

/// Where one item goes: its block reference's insertion point (the image of the array's base
/// point) and rotation (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrayItem {
    pub i: u32,
    pub j: u32,
    pub level: u32,
    pub insert: Vec3,
    pub rotation: f64,
}

impl ArrayParams {
    /// Items along the first direction (columns or items around) and rows.
    pub fn grid(&self) -> (u32, u32) {
        match &self.kind {
            ArrayKind::Rect(r) => (r.cols, r.rows),
            ArrayKind::Polar(p) => (p.items, p.rows),
            ArrayKind::Path(p) => (p.count_and_step(p.length()).0, p.rows),
        }
    }

    /// The number of items, saturating.
    pub fn item_count(&self) -> u64 {
        let (a, b) = self.grid();
        u64::from(a).saturating_mul(u64::from(b)).saturating_mul(u64::from(self.levels))
    }

    /// Whether the parameters are usable: at least one item in every direction, at most
    /// [`MAX_ARRAY_ITEMS`] in all, and finite numbers. Parameters read from files are checked
    /// with this before they are kept.
    pub fn is_valid(&self) -> bool {
        let (a, b) = self.grid();
        let nums = match &self.kind {
            ArrayKind::Rect(r) => [r.col_spacing, r.row_spacing, r.axis_angle, r.row_elevation, 0.0, 0.0],
            ArrayKind::Polar(p) => [p.center.x, p.center.y, p.fill, p.row_spacing, p.row_elevation, 0.0],
            ArrayKind::Path(p) => {
                let v = &p.path.vertices;
                if v.len() < 2 || v.len() > MAX_PATH_VERTICES || !v.iter().all(|v| v.p.x.is_finite() && v.p.y.is_finite() && v.bulge.is_finite()) {
                    return false;
                }
                let len = p.length();
                if !(len.is_finite() && len > 0.0) || p.spacing < 0.0 || (!p.divide && p.spacing == 0.0) {
                    return false;
                }
                [p.spacing, p.ref_angle, p.row_spacing, p.row_elevation, len, 0.0]
            }
        };
        a >= 1
            && b >= 1
            && self.levels >= 1
            && self.item_count() <= MAX_ARRAY_ITEMS
            && !self.source.trim().is_empty()
            && nums.iter().chain(std::iter::once(&self.level_spacing)).all(|v| v.is_finite())
            && self.replaced.len() as u64 <= MAX_ARRAY_ITEMS
            && self.removed.len() as u64 <= MAX_ARRAY_ITEMS
    }

    /// Whether item (i, j, level) was taken out of the array.
    pub fn is_removed(&self, i: u32, j: u32, level: u32) -> bool {
        self.removed.contains(&[i, j, level])
    }

    /// Where every item goes, row by row and level by level; `None` when the parameters are not
    /// [valid](Self::is_valid).
    pub fn items(&self) -> Option<Vec<ArrayItem>> {
        if !self.is_valid() {
            return None;
        }
        let n = usize::try_from(self.item_count()).ok()?;
        let mut out = Vec::with_capacity(n);
        for level in 0..self.levels {
            let lz = self.level_spacing * f64::from(level);
            match &self.kind {
                ArrayKind::Rect(r) => {
                    let (ux, uy) = (Vec2::from_angle(r.axis_angle), Vec2::from_angle(r.axis_angle).perp());
                    for j in 0..r.rows {
                        for i in 0..r.cols {
                            let p = ux * (r.col_spacing * f64::from(i)) + uy * (r.row_spacing * f64::from(j));
                            out.push(ArrayItem { i, j, level, insert: p.to3(lz + r.row_elevation * f64::from(j)), rotation: 0.0 });
                        }
                    }
                }
                ArrayKind::Polar(p) => {
                    let total = p.fill.to_radians();
                    let full = (total.abs() - std::f64::consts::TAU).abs() < 1e-9;
                    let step = if full { total / f64::from(p.items.max(1)) } else { total / f64::from(p.items.saturating_sub(1).max(1)) };
                    // Rows move the items outwards: along the line from the centre to the base point.
                    let out_dir = {
                        let d = (Vec2::ZERO - p.center).normalized();
                        if d == Vec2::ZERO { Vec2::X } else { d }
                    };
                    for j in 0..p.rows {
                        let o = out_dir * (p.row_spacing * f64::from(j));
                        for i in 0..p.items {
                            let a = step * f64::from(i);
                            let at = o.rotate_about(p.center, a);
                            let rotation = if p.rotate { a } else { 0.0 };
                            out.push(ArrayItem { i, j, level, insert: at.to3(lz + p.row_elevation * f64::from(j)), rotation });
                        }
                    }
                }
                ArrayKind::Path(p) => {
                    let segs = p.segments();
                    let len: f64 = segs.iter().map(Segment::len).sum();
                    let (n, step) = p.count_and_step(len);
                    for j in 0..p.rows {
                        for i in 0..n {
                            let Some((at, tangent)) = at_length(&segs, step * f64::from(i)) else { continue };
                            // Rows go to the left of the path.
                            let at = at + tangent.perp() * (p.row_spacing * f64::from(j));
                            let rotation = if p.align { tangent.angle() - p.ref_angle } else { 0.0 };
                            out.push(ArrayItem { i, j, level, insert: at.to3(lz + p.row_elevation * f64::from(j)), rotation });
                        }
                    }
                }
            }
        }
        if !self.removed.is_empty() {
            out.retain(|it| !self.is_removed(it.i, it.j, it.level));
        }
        Some(out)
    }

    /// The block an item refers to: a replacement, else the source.
    pub fn block_for(&self, i: u32, j: u32, level: u32) -> &str {
        self.replaced.iter().rev().find(|r| (r.i, r.j, r.level) == (i, j, level)).map_or(self.source.as_str(), |r| r.block.as_str())
    }

    /// The block references that make up the array block: one per item, on layer 0 with ByBlock
    /// properties so they take the array's. Handles are allocated from `d`. `None` when the
    /// parameters are not [valid](Self::is_valid).
    pub fn item_entities(&self, d: &mut Drawing) -> Option<Vec<Entity>> {
        let items = self.items()?;
        let common = Common {
            layer: "0".into(),
            color: cadaddy_color::Color::ByBlock,
            linetype: "ByBlock".into(),
            lineweight: Lineweight::ByBlock,
            ..Common::default()
        };
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            let ins = Insert {
                block: self.block_for(it.i, it.j, it.level).to_string(),
                insert: it.insert,
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: it.rotation,
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            };
            out.push(Entity { handle: d.new_handle(), common: common.clone(), kind: EntityKind::Insert(ins) });
        }
        Some(out)
    }

    /// Take the block names from the array block's items `ents` (in the order [`items`]
    /// lists them): the source block is whatever the items that are not replaced refer to, so
    /// the parameters follow blocks renamed on paste, on INSERT of a file or by other programs
    /// (which may rename anonymous blocks). `false` when the items don't match the parameters
    /// (another program changed the block), so the array should be treated as a plain block.
    ///
    /// [`items`]: Self::items
    pub fn sync_names<'a>(&mut self, ents: impl Iterator<Item = &'a Entity>) -> bool {
        let Some(items) = self.items() else { return false };
        let ents: Vec<&Entity> = ents.take(items.len().saturating_add(1)).collect();
        if ents.len() != items.len() {
            return false;
        }
        let mut source: Option<&str> = None;
        for (it, e) in items.iter().zip(&ents) {
            let EntityKind::Insert(ins) = &e.kind else { return false };
            match self.replaced.iter_mut().rev().find(|r| (r.i, r.j, r.level) == (it.i, it.j, it.level)) {
                Some(r) => r.block.clone_from(&ins.block),
                None => match source {
                    None => source = Some(&ins.block),
                    Some(s) if s.eq_ignore_ascii_case(&ins.block) => {}
                    Some(_) => return false,
                },
            }
        }
        if let Some(s) = source {
            self.source = s.to_string();
        }
        true
    }
}

impl Drawing {
    /// The parameters of `e` when it is an associative array (a block reference to an array
    /// block whose items match its parameters), with the block names its items use.
    pub fn array_params(&self, e: &Entity) -> Option<ArrayParams> {
        let EntityKind::Insert(i) = &e.kind else { return None };
        let b = self.block(&i.block)?;
        let mut p = b.array.clone()?;
        p.sync_names(b.entities.iter().map(|e| e.as_ref())).then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(cols: u32, rows: u32) -> ArrayParams {
        ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Rect(RectArray { cols, rows, col_spacing: 2.0, row_spacing: 3.0, axis_angle: 0.0, row_elevation: 0.0 }),
            levels: 2,
            level_spacing: 5.0,
            replaced: Vec::new(),
            removed: Vec::new(),
            editing: None,
        }
    }

    fn path(divide: bool) -> ArrayParams {
        ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Path(PathArray {
                curve: None,
                path: Polyline::from_points(&[Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)], false),
                items: 5,
                spacing: 4.0,
                divide,
                align: true,
                ref_angle: 0.0,
                rows: 1,
                row_spacing: 0.0,
                row_elevation: 0.0,
            }),
            levels: 1,
            level_spacing: 0.0,
            replaced: Vec::new(),
            removed: Vec::new(),
            editing: None,
        }
    }

    #[test]
    fn path_items_measure_and_divide() {
        // Measure: every 4 units, 5 items fit in 20.
        let items = path(false).items().unwrap_or_default();
        assert_eq!(items.len(), 5);
        assert!(items[3].insert.xy().near(Vec2::new(10.0, 2.0), 1e-9));
        assert!((items[3].rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "turned with the path");
        // Divide: 5 items over 20 units (4 gaps): ends included.
        let items = path(true).items().unwrap_or_default();
        assert!(items[4].insert.xy().near(Vec2::new(10.0, 10.0), 1e-9));
        // A shorter path holds fewer measured items.
        let mut p = path(false);
        if let ArrayKind::Path(k) = &mut p.kind {
            k.spacing = 7.0;
        }
        assert_eq!(p.items().unwrap_or_default().len(), 3);
    }

    #[test]
    fn removed_items_are_left_out() {
        let mut p = rect(3, 2);
        p.removed.push([1, 0, 0]);
        let items = p.items().unwrap_or_default();
        assert_eq!(items.len(), 11);
        assert!(!items.iter().any(|it| (it.i, it.j, it.level) == (1, 0, 0)));
        assert_eq!(p.item_count(), 12, "the grid is unchanged");
    }

    #[test]
    fn rect_items_go_column_row_level() {
        let items = rect(3, 2).items().unwrap_or_default();
        assert_eq!(items.len(), 12);
        let last = items.last().copied().map(|i| (i.i, i.j, i.level, i.insert));
        assert_eq!(last, Some((2, 1, 1, Vec3::new(4.0, 3.0, 5.0))));
    }

    #[test]
    fn polar_rows_go_outwards() {
        let p = ArrayParams {
            source: "*U1".into(),
            kind: ArrayKind::Polar(PolarArray {
                center: Vec2::new(-10.0, 0.0),
                items: 4,
                fill: 360.0,
                rotate: true,
                rows: 2,
                row_spacing: 1.0,
                row_elevation: 0.0,
            }),
            levels: 1,
            level_spacing: 0.0,
            replaced: Vec::new(),
            removed: Vec::new(),
            editing: None,
        };
        let items = p.items().unwrap_or_default();
        assert_eq!(items.len(), 8);
        // Item 1 of row 0 is a quarter turn round: base (0,0) → (-10,10).
        assert!(items[1].insert.xy().near(Vec2::new(-10.0, 10.0), 1e-9));
        // Row 1 starts one unit further out.
        assert!(items[4].insert.xy().near(Vec2::new(1.0, 0.0), 1e-9));
    }

    #[test]
    fn hostile_params_are_invalid() {
        let mut p = rect(u32::MAX, u32::MAX);
        assert!(!p.is_valid() && p.items().is_none());
        p = rect(0, 1);
        assert!(!p.is_valid());
        p = rect(2, 2);
        p.level_spacing = f64::NAN;
        assert!(!p.is_valid());
        let mut p = path(false);
        if let ArrayKind::Path(k) = &mut p.kind {
            k.path.vertices.truncate(1);
        }
        assert!(!p.is_valid());
        let mut p = path(false);
        if let ArrayKind::Path(k) = &mut p.kind {
            k.spacing = 0.0;
        }
        assert!(!p.is_valid());
    }
}
