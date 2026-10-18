//! Grips of associative arrays, as AutoCAD shows them on a selected array, and the Key point
//! base points of ARRAYRECT, ARRAYPOLAR and ARRAYEDIT.
//!
//! A rectangular array has a base grip (move), spacing grips on its second column and second row,
//! count grips on its last column and last row, and a row-and-column count grip on its far corner.
//! A polar array has its base grip (move, or stretch the radius), a centre grip, an item spacing
//! grip on the second item and an item count grip on the last; both have row (and level) grips
//! when they have more than one. Count grips keep the spacing unless their option (Total…)
//! changes the total instead. Dragging a grip is one undo step: `array.grip` from JSON, or
//! [`Session::start_array_grip`] for the drag the UI starts with a click on the grip.
//!
//! Positions are worked out in the array block's coordinates (its base point at the origin) and
//! placed by the array's transform `w`.

use cadaddy_doc::{ArrayKind, ArrayParams, Drawing, EntityKind, Handle};
use cadaddy_geom::{Mat3, Vec2};
use serde::Serialize;
use serde_json::{Value, json};

use super::edit::{array_of, rebuild};
use super::{PREVIEW_LIMIT, other};
use crate::cmd::{CommandSpec, bad, point_param, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Running, Session, Step};

/// Which grip of an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GripId {
    /// The base point: moves the array (polar: or stretches its radius).
    Base,
    /// Rectangular: the second column (distance between columns).
    ColumnSpacing,
    /// Rectangular: the last column (number of columns).
    ColumnCount,
    /// Both arrays: the second row (distance between rows).
    RowSpacing,
    /// Both arrays: the last row (number of rows).
    RowCount,
    /// Rectangular: the far corner (columns and rows).
    RowColumnCount,
    /// The second level (distance between levels): typed, a plan view can't drag along Z.
    LevelSpacing,
    /// The last level (number of levels): typed.
    LevelCount,
    /// Polar: the centre.
    Center,
    /// Polar: the second item (angle between items).
    ItemSpacing,
    /// Polar: the last item (number of items).
    ItemCount,
}

const ALL: [GripId; 11] = [
    GripId::Base,
    GripId::ColumnSpacing,
    GripId::ColumnCount,
    GripId::RowSpacing,
    GripId::RowCount,
    GripId::RowColumnCount,
    GripId::LevelSpacing,
    GripId::LevelCount,
    GripId::Center,
    GripId::ItemSpacing,
    GripId::ItemCount,
];

impl GripId {
    /// The name JSON uses (`columnCount`…).
    pub fn name(self) -> &'static str {
        match self {
            GripId::Base => "base",
            GripId::ColumnSpacing => "columnSpacing",
            GripId::ColumnCount => "columnCount",
            GripId::RowSpacing => "rowSpacing",
            GripId::RowCount => "rowCount",
            GripId::RowColumnCount => "rowColumnCount",
            GripId::LevelSpacing => "levelSpacing",
            GripId::LevelCount => "levelCount",
            GripId::Center => "center",
            GripId::ItemSpacing => "itemSpacing",
            GripId::ItemCount => "itemCount",
        }
    }

    pub fn from_name(s: &str) -> Option<GripId> {
        ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(s.trim()))
    }

    /// What the grip changes, as its tooltip names it.
    pub fn label(self) -> &'static str {
        match self {
            GripId::Base => "Base Point",
            GripId::ColumnSpacing => "Column Spacing",
            GripId::ColumnCount => "Column Count",
            GripId::RowSpacing => "Row Spacing",
            GripId::RowCount => "Row Count",
            GripId::RowColumnCount => "Row and Column Count",
            GripId::LevelSpacing => "Level Spacing",
            GripId::LevelCount => "Level Count",
            GripId::Center => "Center",
            GripId::ItemSpacing => "Item Spacing",
            GripId::ItemCount => "Item Count",
        }
    }
}

/// A grip's second behaviour, picked from its menu or typed while dragging it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GripOption {
    /// What the grip does first (a count grip keeps the spacing; a base grip moves the array).
    #[default]
    Default,
    /// A count grip changes the total distance (or the fill angle) and keeps the count.
    Total,
    /// A polar array's base grip stretches the radius instead of moving the array.
    StretchRadius,
}

impl GripOption {
    pub fn name(self) -> &'static str {
        match self {
            GripOption::Default => "default",
            GripOption::Total => "total",
            GripOption::StretchRadius => "stretchRadius",
        }
    }

    pub fn from_name(s: &str) -> Option<GripOption> {
        [GripOption::Default, GripOption::Total, GripOption::StretchRadius].into_iter().find(|o| o.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// How a grip is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GripLook {
    /// A square (base, centre).
    Square,
    /// A triangle pointing along `dir` (counts and spacings).
    Arrow,
}

/// One grip of an array.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrayGrip {
    pub grip: GripId,
    pub look: GripLook,
    /// Where it is, in drawing coordinates.
    #[serde(serialize_with = "point")]
    pub at: Vec2,
    /// Its elevation (level grips sit above the base point).
    pub z: f64,
    /// The way an arrow points (a unit vector in drawing coordinates; zero for a square).
    #[serde(serialize_with = "point")]
    pub dir: Vec2,
    /// Whether it can be dragged in a plan view (level grips are typed: they sit on the base).
    pub plan: bool,
    /// Its other behaviour, if it has one: the keyword typed while dragging, and the option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<(&'static str, GripOption)>,
}

fn point<S: serde::Serializer>(p: &Vec2, s: S) -> std::result::Result<S::Ok, S::Error> {
    [p.x, p.y].serialize(s)
}

/// The keyword (and label) of grip `g`'s other behaviour on an array of kind `kind`.
fn option_of(kind: &ArrayKind, g: GripId) -> Option<(&'static str, GripOption)> {
    let polar = matches!(kind, ArrayKind::Polar(_));
    match g {
        GripId::ColumnCount => Some(("Total column spacing", GripOption::Total)),
        GripId::RowCount => Some(("Total row spacing", GripOption::Total)),
        GripId::RowColumnCount => Some(("Total row and column spacing", GripOption::Total)),
        GripId::LevelCount => Some(("Total level spacing", GripOption::Total)),
        GripId::ItemCount => Some(("Fill angle", GripOption::Total)),
        GripId::Base if polar => Some(("Stretch radius", GripOption::StretchRadius)),
        _ => None,
    }
}

/// A rectangular array's column and row directions (unit vectors, array coordinates).
fn rect_axes(axis_angle: f64) -> (Vec2, Vec2) {
    let ux = Vec2::from_angle(axis_angle);
    (ux, ux.perp())
}

/// A polar array's step between items, radians (signed like the fill angle).
fn polar_step(p: &cadaddy_doc::PolarArray) -> f64 {
    let total = p.fill.to_radians();
    if is_full(p.fill) { total / f64::from(p.items.max(1)) } else { total / f64::from(p.items.saturating_sub(1).max(1)) }
}

fn is_full(fill: f64) -> bool {
    (fill.abs() - 360.0).abs() < 1e-9
}

/// A polar array's outward direction (from the centre through the base point).
fn polar_out(center: Vec2) -> Vec2 {
    let d = (Vec2::ZERO - center).normalized();
    if d == Vec2::ZERO { Vec2::X } else { d }
}

/// The grips of an array with parameters `p` placed by `w` (array coordinates to drawing
/// coordinates). Without `with_base` the base grip is left out (while an array is being made,
/// its source objects stay where they are).
pub(crate) fn grips_for(p: &ArrayParams, w: &Mat3, with_base: bool) -> Vec<ArrayGrip> {
    let mut out = Vec::new();
    let mut add = |grip: GripId, look: GripLook, at: Vec2, dir: Vec2| {
        let world_dir = w.apply_vec(dir).normalized();
        out.push(ArrayGrip { grip, look, at: w.apply(at), z: 0.0, dir: world_dir, plan: true, option: option_of(&p.kind, grip) });
    };
    if with_base {
        add(GripId::Base, GripLook::Square, Vec2::ZERO, Vec2::ZERO);
    }
    match &p.kind {
        ArrayKind::Rect(r) => {
            let (ux, uy) = rect_axes(r.axis_angle);
            let (c, rows) = (f64::from(r.cols), f64::from(r.rows));
            // Count grips come first: with two columns (rows, items) the count grip and the spacing
            // grip are on the same item, and a pick there takes the count.
            add(GripId::ColumnCount, GripLook::Arrow, ux * (r.col_spacing * (c - 1.0).max(1.0)), ux * r.col_spacing.signum());
            if r.cols > 1 {
                add(GripId::ColumnSpacing, GripLook::Arrow, ux * r.col_spacing, ux * r.col_spacing.signum());
            }
            add(GripId::RowCount, GripLook::Arrow, uy * (r.row_spacing * (rows - 1.0).max(1.0)), uy * r.row_spacing.signum());
            if r.rows > 1 {
                add(GripId::RowSpacing, GripLook::Arrow, uy * r.row_spacing, uy * r.row_spacing.signum());
            }
            if r.cols > 1 && r.rows > 1 {
                let corner = ux * (r.col_spacing * (c - 1.0)) + uy * (r.row_spacing * (rows - 1.0));
                add(GripId::RowColumnCount, GripLook::Arrow, corner, corner.normalized());
            }
        }
        ArrayKind::Polar(k) => {
            let step = polar_step(k);
            let turn = if k.fill < 0.0 { -1.0 } else { 1.0 };
            add(GripId::Center, GripLook::Square, k.center, Vec2::ZERO);
            if k.items > 1 {
                let last = Vec2::ZERO.rotate_about(k.center, step * f64::from(k.items - 1));
                add(GripId::ItemCount, GripLook::Arrow, last, (last - k.center).perp() * turn);
                let second = Vec2::ZERO.rotate_about(k.center, step);
                add(GripId::ItemSpacing, GripLook::Arrow, second, (second - k.center).perp() * turn);
            }
            if k.rows > 1 {
                let out_dir = polar_out(k.center);
                add(GripId::RowCount, GripLook::Arrow, out_dir * (k.row_spacing * f64::from(k.rows - 1)), out_dir);
                add(GripId::RowSpacing, GripLook::Arrow, out_dir * k.row_spacing, out_dir);
            }
        }
    }
    if p.levels > 1 {
        let base = w.apply(Vec2::ZERO);
        for (grip, z) in [(GripId::LevelSpacing, p.level_spacing), (GripId::LevelCount, p.level_spacing * f64::from(p.levels - 1))] {
            out.push(ArrayGrip { grip, look: GripLook::Arrow, at: base, z, dir: Vec2::ZERO, plan: false, option: option_of(&p.kind, grip) });
        }
    }
    out
}

/// The grips of array `h` in drawing `d` (none when it isn't an associative array).
pub fn array_grips(d: &Drawing, h: Handle) -> Vec<ArrayGrip> {
    match array_of(d, h) {
        Ok((ins, blk, p)) => grips_for(&p, &ins.transform(blk.base.xy()), true),
        Err(_) => Vec::new(),
    }
}

/// An array changed by a grip: its new parameters, and its base point's new place (drawing
/// coordinates) when the grip moved it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edited {
    pub params: ArrayParams,
    pub base: Option<Vec2>,
}

/// `n` items from a distance `d` along a direction whose items are `spacing` apart (at least 1).
fn count_for(d: f64, spacing: f64) -> Option<u32> {
    if spacing.abs() < 1e-12 || !d.is_finite() {
        return None;
    }
    let n = (d / spacing).round() + 1.0;
    Some(if n < 1.0 {
        1
    } else if n > f64::from(u32::MAX) {
        u32::MAX
    } else {
        n as u32
    })
}

fn nonzero(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() > 1e-12).then_some(v)
}

/// The angle (radians) from the base point to `to` about `center`, going the way of the fill
/// (`turn` 1 counterclockwise, -1 clockwise), in (0, 2π].
fn sweep(center: Vec2, to: Vec2, turn: f64) -> Option<f64> {
    let a0 = (Vec2::ZERO - center).angle();
    let a1 = (to - center).angle();
    if (to - center).len() < 1e-12 {
        return None;
    }
    let mut a = (a1 - a0) * turn;
    while a <= 1e-12 {
        a += std::f64::consts::TAU;
    }
    while a > std::f64::consts::TAU {
        a -= std::f64::consts::TAU;
    }
    Some(a)
}

/// The array after dragging grip `g` (with option `opt`) to drawing point `to`. `None` when the
/// point gives no usable array (a zero spacing, a point on the centre…).
pub(crate) fn drag(p: &ArrayParams, w: &Mat3, g: GripId, opt: GripOption, to: Vec2) -> Option<Edited> {
    if !to.is_finite() {
        return None;
    }
    let l = w.inverse()?.apply(to);
    let mut q = p.clone();
    let mut base = None;
    match (&mut q.kind, g) {
        (_, GripId::Base) if opt != GripOption::StretchRadius => base = Some(to),
        (ArrayKind::Rect(r), _) => {
            let (ux, uy) = rect_axes(r.axis_angle);
            let (dx, dy) = (l.dot(ux), l.dot(uy));
            let total = opt == GripOption::Total;
            match g {
                GripId::ColumnSpacing => r.col_spacing = nonzero(dx)?,
                GripId::RowSpacing => r.row_spacing = nonzero(dy)?,
                GripId::ColumnCount if total => r.col_spacing = nonzero(dx / f64::from(r.cols.saturating_sub(1).max(1)))?,
                GripId::RowCount if total => r.row_spacing = nonzero(dy / f64::from(r.rows.saturating_sub(1).max(1)))?,
                GripId::RowColumnCount if total => {
                    r.col_spacing = nonzero(dx / f64::from(r.cols.saturating_sub(1).max(1)))?;
                    r.row_spacing = nonzero(dy / f64::from(r.rows.saturating_sub(1).max(1)))?;
                }
                GripId::ColumnCount => r.cols = count_for(dx, r.col_spacing)?,
                GripId::RowCount => r.rows = count_for(dy, r.row_spacing)?,
                GripId::RowColumnCount => {
                    r.cols = count_for(dx, r.col_spacing)?;
                    r.rows = count_for(dy, r.row_spacing)?;
                }
                _ => return None,
            }
        }
        (ArrayKind::Polar(k), _) => {
            let turn = if k.fill < 0.0 { -1.0 } else { 1.0 };
            let out_dir = polar_out(k.center);
            match g {
                GripId::Base => {
                    // Stretch the radius: the base point slides along the line from the centre.
                    let r = nonzero(l.dist(k.center))?;
                    let b = k.center + out_dir * r;
                    k.center -= b;
                    base = Some(w.apply(b));
                }
                GripId::Center => {
                    if l.len() < 1e-12 {
                        return None;
                    }
                    k.center = l;
                }
                GripId::ItemSpacing => {
                    let between = sweep(k.center, l, turn)?.to_degrees();
                    let n = f64::from(k.items.saturating_sub(1).max(1));
                    k.fill = (between * n).min(360.0) * turn;
                }
                GripId::ItemCount => {
                    let a = sweep(k.center, l, turn)?;
                    if opt == GripOption::Total {
                        let deg = a.to_degrees();
                        k.fill = if (deg - 360.0).abs() < 0.5 { 360.0 } else { deg } * turn;
                    } else if is_full(k.fill) {
                        // The last item of a full circle sits one step short of it.
                        let rest = std::f64::consts::TAU - a;
                        let n = if rest < 1e-9 { f64::from(u32::MAX) } else { (std::f64::consts::TAU / rest).round() };
                        k.items = if n < 1.0 {
                            1
                        } else if n > f64::from(u32::MAX) {
                            u32::MAX
                        } else {
                            n as u32
                        };
                    } else {
                        let step = polar_step(k).abs();
                        let n = count_for(a, step)?;
                        // The fill angle can't pass a full circle.
                        let max = ((std::f64::consts::TAU / step.max(1e-12)).floor() as u32).saturating_add(1);
                        k.items = n.min(max).max(1);
                        k.fill = (step.to_degrees() * f64::from(k.items.saturating_sub(1))).min(360.0) * turn;
                    }
                }
                GripId::RowSpacing => k.row_spacing = nonzero(l.dot(out_dir))?,
                GripId::RowCount if opt == GripOption::Total => k.row_spacing = nonzero(l.dot(out_dir) / f64::from(k.rows.saturating_sub(1).max(1)))?,
                GripId::RowCount => k.rows = count_for(l.dot(out_dir), k.row_spacing)?,
                _ => return None,
            }
        }
    }
    q.is_valid().then_some(Edited { params: q, base })
}

/// The array after a value typed at grip `g`: a count for count grips (a total distance or fill
/// angle with Total), a distance for spacing grips (degrees between items for a polar item
/// spacing), the radius for a stretched polar base grip. `None` for a value the grip can't take.
pub(crate) fn typed(p: &ArrayParams, w: &Mat3, g: GripId, opt: GripOption, v: f64) -> Option<Edited> {
    if !v.is_finite() {
        return None;
    }
    let total = opt == GripOption::Total;
    let count = || (v.fract() == 0.0 && v >= 1.0 && v <= f64::from(u32::MAX)).then_some(v as u32);
    let mut q = p.clone();
    let mut base = None;
    match g {
        GripId::LevelSpacing => q.level_spacing = nonzero(v)?,
        GripId::LevelCount if total => q.level_spacing = nonzero(v / f64::from(q.levels.saturating_sub(1).max(1)))?,
        GripId::LevelCount => q.levels = count()?,
        _ => match &mut q.kind {
            ArrayKind::Rect(r) => match g {
                GripId::ColumnSpacing => r.col_spacing = nonzero(v)?,
                GripId::RowSpacing => r.row_spacing = nonzero(v)?,
                GripId::ColumnCount if total => r.col_spacing = nonzero(v / f64::from(r.cols.saturating_sub(1).max(1)))?,
                GripId::RowCount if total => r.row_spacing = nonzero(v / f64::from(r.rows.saturating_sub(1).max(1)))?,
                GripId::ColumnCount => r.cols = count()?,
                GripId::RowCount => r.rows = count()?,
                GripId::RowColumnCount if !total => {
                    let n = count()?;
                    (r.cols, r.rows) = (n, n);
                }
                _ => return None,
            },
            ArrayKind::Polar(k) => {
                let turn = if k.fill < 0.0 { -1.0 } else { 1.0 };
                match g {
                    GripId::ItemSpacing => {
                        let between = nonzero(v)?.abs();
                        k.fill = (between * f64::from(k.items.saturating_sub(1).max(1))).min(360.0) * turn;
                    }
                    GripId::ItemCount if total => {
                        let f = nonzero(v)?;
                        if f.abs() > 360.0 {
                            return None;
                        }
                        k.fill = f;
                    }
                    GripId::ItemCount => k.items = count()?,
                    GripId::RowSpacing => k.row_spacing = nonzero(v)?,
                    GripId::RowCount if total => k.row_spacing = nonzero(v / f64::from(k.rows.saturating_sub(1).max(1)))?,
                    GripId::RowCount => k.rows = count()?,
                    GripId::Base if opt == GripOption::StretchRadius => {
                        let r = nonzero(v)?.abs();
                        let b = k.center + polar_out(k.center) * r;
                        k.center -= b;
                        base = Some(w.apply(b));
                    }
                    _ => return None,
                }
            }
        },
    }
    q.is_valid().then_some(Edited { params: q, base })
}

/// The prompt while grip `g` is dragged (with option `opt`), as AutoCAD words it.
pub(crate) fn prompt(kind: &ArrayKind, g: GripId, opt: GripOption, from: Vec2) -> Prompt {
    let what = match (g, opt) {
        (GripId::Base, GripOption::StretchRadius) => "** STRETCH RADIUS ** Specify radius",
        (GripId::Base, _) => "** MOVE ** Specify base point",
        (GripId::Center, _) => "** CENTER ** Specify center point of array",
        (GripId::ColumnSpacing, _) => "** COLUMN SPACING ** Specify the distance between columns",
        (GripId::RowSpacing, _) => "** ROW SPACING ** Specify the distance between rows",
        (GripId::LevelSpacing, _) => "** LEVEL SPACING ** Specify the distance between levels",
        (GripId::ItemSpacing, _) => "** ITEM SPACING ** Specify the angle between items",
        (GripId::ColumnCount, GripOption::Total) => "** TOTAL COLUMN SPACING ** Specify the distance between the first and last column",
        (GripId::RowCount, GripOption::Total) => "** TOTAL ROW SPACING ** Specify the distance between the first and last row",
        (GripId::RowColumnCount, GripOption::Total) => "** TOTAL ROW AND COLUMN SPACING ** Specify the far corner",
        (GripId::LevelCount, GripOption::Total) => "** TOTAL LEVEL SPACING ** Specify the distance between the first and last level",
        (GripId::ItemCount, GripOption::Total) => "** FILL ANGLE ** Specify the angle to fill",
        (GripId::ColumnCount, _) => "** COLUMN COUNT ** Specify number of columns",
        (GripId::RowCount, _) => "** ROW COUNT ** Specify number of rows",
        (GripId::RowColumnCount, _) => "** ROW AND COLUMN COUNT ** Specify number of rows and columns",
        (GripId::LevelCount, _) => "** LEVEL COUNT ** Enter the number of levels",
        (GripId::ItemCount, _) => "** ITEM COUNT ** Specify number of items",
    };
    let plan = !matches!(g, GripId::LevelSpacing | GripId::LevelCount);
    let accept = if !plan {
        Accept::NUMBER
    } else if g == GripId::Center || (g == GripId::Base && opt != GripOption::StretchRadius) {
        Accept::POINT
    } else {
        Accept::POINT_OR_NUMBER
    };
    let mut kws: Vec<&'static str> = Vec::new();
    if let Some((k, o)) = option_of(kind, g) {
        // The option toggles: typed again, it goes back to the grip's first behaviour.
        kws.push(if opt == o { first_behaviour(g) } else { k });
    }
    kws.push("eXit");
    let pr = Prompt::new(what, accept).kw(&kws);
    if plan { pr.base(from) } else { pr }
}

/// The keyword that switches grip `g` back from its option.
fn first_behaviour(g: GripId) -> &'static str {
    match g {
        GripId::Base => "Move",
        GripId::ItemCount => "Item count",
        GripId::RowColumnCount => "Row and column count",
        GripId::ColumnCount => "Column count",
        GripId::RowCount => "Row count",
        GripId::LevelCount => "Level count",
        _ => "eXit",
    }
}

/// The option keyword `k` typed while dragging grip `g` (toggling as [`prompt`] offers it).
pub(crate) fn option_keyword(kind: &ArrayKind, g: GripId, cur: GripOption, k: &str) -> Option<GripOption> {
    let (kw, o) = option_of(kind, g)?;
    if k.eq_ignore_ascii_case(kw) {
        Some(o)
    } else if k.eq_ignore_ascii_case(first_behaviour(g)) {
        Some(GripOption::Default)
    } else {
        let _ = cur;
        None
    }
}

/// The objects of array `p` placed by `w`, for a preview (at most [`PREVIEW_LIMIT`]).
fn drawn(d: &Drawing, p: &ArrayParams, w: &Mat3) -> Vec<EntityKind> {
    let mut out = Vec::new();
    let Some(items) = p.items() else { return out };
    for it in items {
        let Some(b) = d.block(p.block_for(it.i, it.j, it.level)) else { continue };
        let m = w.then_before(Mat3::translate(it.insert.xy())).then_before(Mat3::rotate(it.rotation)).then_before(Mat3::translate(-b.base.xy()));
        for e in b.entities.iter().filter(|e| e.common.visible) {
            if out.len() >= PREVIEW_LIMIT {
                return out;
            }
            let mut k = e.kind.clone();
            k.transform(&m);
            out.push(k);
        }
    }
    out
}

/// The transform of an array whose base point moved to `base` (drawing coordinates).
fn moved(w: &Mat3, base: Option<Vec2>) -> Mat3 {
    match base {
        Some(b) => Mat3::translate(b - w.apply(Vec2::ZERO)).then_before(*w),
        None => *w,
    }
}

/// Apply a grip edit to array `h` (one rebuild).
fn apply(s: &mut Session, h: Handle, e: Edited) -> Result<()> {
    if crate::cmd::curves::is_locked(s, h) {
        return Err(other("The array is on a locked layer."));
    }
    super::edit::refuse_while_editing(s)?;
    let insert = match e.base {
        Some(b) => {
            let (ins, blk, _) = array_of(s.doc()?, h)?;
            // The block reference's insertion point that puts the base point at `b`.
            let w = ins.transform(blk.base.xy());
            Some(ins.insert.xy() + (b - w.apply(Vec2::ZERO)))
        }
        None => None,
    };
    rebuild(s, h, e.params, insert)
}

// ---------------- key points ----------------

/// Most key points gathered from source objects.
const MAX_KEY_POINTS: usize = 100_000;

/// The key points of objects `kinds` (their grips: ends, midpoints, centres, quadrants,
/// insertion points, vertices…).
pub(crate) fn key_points<'a>(kinds: impl Iterator<Item = &'a EntityKind>) -> Vec<Vec2> {
    kinds.flat_map(|k| k.grips()).filter(|p| p.is_finite()).take(MAX_KEY_POINTS).collect()
}

/// The key point of `pts` nearest `p`.
pub(crate) fn nearest_key(pts: &[Vec2], p: Vec2) -> Option<Vec2> {
    pts.iter().copied().min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)))
}

/// The key points of objects `hs` in the drawing.
pub(crate) fn key_points_of(d: &Drawing, hs: &[Handle]) -> Vec<Vec2> {
    key_points(hs.iter().filter_map(|h| d.entity(*h)).map(|e| &e.kind))
}

/// The key points of array `h`'s source objects as its first item shows them (drawing
/// coordinates).
pub(crate) fn source_key_points(d: &Drawing, h: Handle) -> Result<Vec<Vec2>> {
    let (ins, blk, p) = array_of(d, h)?;
    let src = d.block(&p.source).ok_or_else(|| other("The array's source block is missing."))?;
    let m = ins.transform(blk.base.xy()).then_before(Mat3::translate(-src.base.xy()));
    let placed: Vec<EntityKind> = src
        .entities
        .iter()
        .map(|e| {
            let mut k = e.kind.clone();
            k.transform(&m);
            k
        })
        .collect();
    Ok(key_points(placed.iter()))
}

/// The prompt for a key point.
pub(crate) fn key_prompt() -> Prompt {
    Prompt::new("Specify source objects key point", Accept::POINT)
}

/// The key point of `pts` picked at `i` (the nearest to the point), or an error.
pub(crate) fn key_input(pts: &[Vec2], i: &Input) -> Result<Option<Vec2>> {
    match i {
        Input::Point(p) => nearest_key(pts, *p).map(Some).ok_or_else(|| other("The source objects have no key points.")),
        Input::Deferred(d) => nearest_key(pts, d.at).map(Some).ok_or_else(|| other("The source objects have no key points.")),
        _ => Ok(None),
    }
}

// ---------------- commands ----------------

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("array.grips", "Array Grips", run_grips)
            .params("{handle} → [{grip, look, at, z, dir, plan, option?: [keyword, option]}] (none when the object is not an associative array)")
            .noundo(),
        CommandSpec::new("array.grip", "Edit Array with a Grip", run_grip).params(
            "{handle, grip: \"base\"|\"columnSpacing\"|\"columnCount\"|\"rowSpacing\"|\"rowCount\"|\"rowColumnCount\"|\"levelSpacing\"|\"levelCount\"|\"center\"|\"itemSpacing\"|\"itemCount\", to?: [x, y] | value?: number (a count, a distance or an angle in degrees), option?: \"default\"|\"total\"|\"stretchRadius\"} → the array's grips after the edit",
        ),
    ]
}

fn handle_of(cmd: &str, p: &Value) -> Result<Handle> {
    crate::cmd::curves::handle_param(p, "handle").ok_or_else(|| bad(cmd, "`handle` is required"))
}

fn run_grips(s: &mut Session, p: &Value) -> Result<Value> {
    let h = handle_of("array.grips", p)?;
    serde_json::to_value(array_grips(s.doc()?, h)).map_err(|e| bad("array.grips", e.to_string()))
}

fn run_grip(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "array.grip";
    let h = handle_of(cmd, p)?;
    let g = str_param(p, "grip").and_then(GripId::from_name).ok_or_else(|| bad(cmd, "`grip` must name one of the array's grips"))?;
    let opt = match str_param(p, "option") {
        Some(o) => GripOption::from_name(o).ok_or_else(|| bad(cmd, "`option` must be default, total or stretchRadius"))?,
        None => GripOption::Default,
    };
    let (ins, blk, params) = array_of(s.doc()?, h)?;
    let w = ins.transform(blk.base.xy());
    if !grips_for(&params, &w, true).iter().any(|x| x.grip == g) {
        return Err(bad(cmd, format!("the array has no {} grip", g.name())));
    }
    if opt != GripOption::Default && option_of(&params.kind, g).is_none_or(|(_, o)| o != opt) {
        return Err(bad(cmd, format!("the {} grip has no {} option", g.name(), opt.name())));
    }
    let edited = if let Some(to) = point_param(p, "to") {
        drag(&params, &w, g, opt, to)
    } else if let Some(v) = p.get("value").and_then(Value::as_f64) {
        typed(&params, &w, g, opt, v)
    } else {
        return Err(bad(cmd, "`to` [x, y] or `value` is required"));
    };
    let edited = edited.ok_or_else(|| bad(cmd, "that gives no valid array"))?;
    apply(s, h, edited)?;
    run_grips(s, &json!({ "handle": h.hex() }))
}

// ---------------- dragging a grip ----------------

/// An array grip being dragged.
struct ArrayGripEdit {
    handle: Handle,
    grip: GripId,
    option: GripOption,
    /// Where the grip is (the rubber band's start).
    from: Vec2,
}

impl ArrayGripEdit {
    /// The array's parameters and transform as they are now.
    fn current(&self, s: &Session) -> Option<(ArrayParams, Mat3)> {
        let (ins, blk, p) = array_of(s.doc().ok()?, self.handle).ok()?;
        Some((p, ins.transform(blk.base.xy())))
    }
}

impl Interactive for ArrayGripEdit {
    fn name(&self) -> &'static str {
        "GRIP"
    }

    fn begin(&mut self, _s: &mut Session) -> Result<Step> {
        Ok(Step::Continue)
    }

    fn prompt(&self, s: &Session) -> Prompt {
        match self.current(s) {
            Some((p, _)) => prompt(&p.kind, self.grip, self.option, self.from),
            None => Prompt::new("** GRIP **", Accept::POINT).kw(&["eXit"]),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let (p, w) = self.current(s).ok_or_else(|| other("The object is not an associative array."))?;
        let edited = match &i {
            Input::Point(to) => drag(&p, &w, self.grip, self.option, *to),
            Input::Deferred(d) => drag(&p, &w, self.grip, self.option, d.at),
            Input::Text(t) => {
                let v = t.trim().parse::<f64>().map_err(|_| other("Requires a number or a point."))?;
                typed(&p, &w, self.grip, self.option, v)
            }
            Input::Keyword(k) if k == "eXit" => return Ok(Step::Done),
            Input::Keyword(k) => {
                self.option = option_keyword(&p.kind, self.grip, self.option, k).ok_or_else(|| other("Invalid option keyword."))?;
                return Ok(Step::Continue);
            }
            Input::Enter | Input::Cancel => return Ok(Step::Done),
            _ => return Err(other("Requires a point.")),
        };
        let edited = edited.ok_or_else(|| other("That gives no valid array; specify another point."))?;
        apply(s, self.handle, edited)?;
        Ok(Step::Done)
    }

    fn preview(&self, s: &Session, c: Vec2) -> Vec<EntityKind> {
        let Some((p, w)) = self.current(s) else { return Vec::new() };
        let Ok(d) = s.doc() else { return Vec::new() };
        let mut out = match drag(&p, &w, self.grip, self.option, c) {
            Some(e) if e.params.item_count() <= PREVIEW_LIMIT as u64 * 4 => drawn(d, &e.params, &moved(&w, e.base)),
            _ => Vec::new(),
        };
        out.push(super::line(self.from, c));
        out
    }
}

impl Session {
    /// Start dragging grip `grip` of associative array `handle` (the UI's click on the grip).
    /// Level grips take a typed value.
    pub fn start_array_grip(&mut self, handle: Handle, grip: GripId) -> Result<()> {
        if self.running.is_some() {
            self.cancel();
        }
        let g = array_grips(self.doc()?, handle).into_iter().find(|g| g.grip == grip).ok_or_else(|| EngineError::Other("no such grip".into()))?;
        let edit = ArrayGripEdit { handle, grip, option: GripOption::Default, from: g.at };
        let label = edit.prompt(self).message.split("**").nth(1).map(str::trim).unwrap_or("GRIP").to_string();
        let machine = crate::guard::Guarded::wrap(Box::new(edit));
        let st = self.state()?;
        let (before, selection_before) = (st.doc.clone(), st.selection.clone());
        self.running = Some(Running { id: "grip".into(), machine, before, selection_before });
        self.pending_window = None;
        self.point_mods.clear();
        self.echo(format!("** {label} **"));
        Ok(())
    }
}

/// The grip of an array being made (parameters `p`, base point at `base`) within `tol` of `at`.
pub(crate) fn creating_grip_at(p: &ArrayParams, base: Vec2, at: Vec2, tol: f64) -> Option<GripId> {
    grips_for(p, &Mat3::translate(base), false)
        .into_iter()
        .filter(|g| g.plan && g.at.dist(at) <= tol)
        .min_by(|a, b| a.at.dist(at).total_cmp(&b.at.dist(at)))
        .map(|g| g.grip)
}

/// The tolerance for picking a grip of an array being made: the grip size and a little more.
pub(crate) fn grip_tolerance(s: &Session) -> f64 {
    s.pixel_size() * (s.settings.gripsize + 3.0) * 1.5
}

/// Where grip `g` of an array being made is (for the rubber band).
pub(crate) fn creating_grip_point(p: &ArrayParams, base: Vec2, g: GripId) -> Vec2 {
    grips_for(p, &Mat3::translate(base), false).into_iter().find(|x| x.grip == g).map_or(base, |x| x.at)
}
