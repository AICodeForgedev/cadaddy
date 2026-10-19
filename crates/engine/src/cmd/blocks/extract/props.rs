//! The object properties data extraction can put in columns, per object type: what the
//! Properties palette shows for it (a property an object doesn't have is left empty).

use cadaddy_doc::{Common, EntityKind, Lineweight};
use cadaddy_geom::Vec2;

use super::Cell;

/// Every property, as its column is headed.
pub const PROPERTIES: &[&str] = &[
    "Layer",
    "Color",
    "Linetype",
    "Lineweight",
    "Length",
    "Area",
    "Radius",
    "Center X",
    "Center Y",
    "Start X",
    "Start Y",
    "End X",
    "End Y",
    "Position X",
    "Position Y",
    "Rotation",
    "Contents",
    "Text height",
    "Closed",
];

/// The property named `name` (any case), as its column is headed.
pub(crate) fn canonical(name: &str) -> Option<&'static str> {
    PROPERTIES.iter().copied().find(|p| p.eq_ignore_ascii_case(name.trim()))
}

/// What a row's object is, for its properties.
pub(crate) struct Subject<'a> {
    pub common: &'a Common,
    /// The layer it shows on (a nested object on layer 0 takes its holder's).
    pub layer: &'a str,
    pub kind: &'a EntityKind,
    /// Its position and rotation (degrees) in the drawing.
    pub at: Vec2,
    pub rotation: f64,
}

fn lineweight(lw: Lineweight) -> String {
    match lw {
        Lineweight::ByLayer => "ByLayer".into(),
        Lineweight::ByBlock => "ByBlock".into(),
        Lineweight::Default => "Default".into(),
        Lineweight::Mm100(n) => format!("{:.2} mm", f64::from(n) / 100.0),
    }
}

fn center(k: &EntityKind) -> Option<Vec2> {
    match k {
        EntityKind::Circle(c) => Some(c.center.xy()),
        EntityKind::Arc(a) => Some(a.center.xy()),
        EntityKind::Ellipse(e) => Some(e.center.xy()),
        _ => None,
    }
}

fn ends(k: &EntityKind) -> Option<(Vec2, Vec2)> {
    match k {
        EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
        EntityKind::Circle(_) => None,
        _ => {
            let c = crate::cmd::curves::Chain::of(k)?;
            if c.closed { None } else { Some((c.start()?, c.end()?)) }
        }
    }
}

fn num(v: Option<f64>) -> Cell {
    match v.filter(|v| v.is_finite()) {
        Some(v) => Cell::Num(v),
        None => Cell::Text(String::new()),
    }
}

/// The value of property `name` (from [`PROPERTIES`]) for `s`.
pub(crate) fn value(name: &str, s: &Subject) -> Cell {
    let k = s.kind;
    match name {
        "Layer" => Cell::Text(s.layer.to_string()),
        "Color" => Cell::Text(s.common.color.name()),
        "Linetype" => Cell::Text(s.common.linetype.clone()),
        "Lineweight" => Cell::Text(lineweight(s.common.lineweight)),
        "Length" => num(match k {
            EntityKind::Line(_)
            | EntityKind::Arc(_)
            | EntityKind::Circle(_)
            | EntityKind::LwPolyline(_)
            | EntityKind::Polyline3d(_)
            | EntityKind::Ellipse(_)
            | EntityKind::Spline(_) => crate::cmd::curves::Chain::of(k).map(|c| c.len()),
            _ => None,
        }),
        "Area" => num(match k {
            EntityKind::LwPolyline(p) if !p.closed => None,
            _ => crate::cmd::inquiry::kind_area(k).ok().map(|a| a.0),
        }),
        "Radius" => num(match k {
            EntityKind::Circle(c) => Some(c.radius),
            EntityKind::Arc(a) => Some(a.radius),
            _ => None,
        }),
        "Center X" => num(center(k).map(|c| c.x)),
        "Center Y" => num(center(k).map(|c| c.y)),
        "Start X" => num(ends(k).map(|e| e.0.x)),
        "Start Y" => num(ends(k).map(|e| e.0.y)),
        "End X" => num(ends(k).map(|e| e.1.x)),
        "End Y" => num(ends(k).map(|e| e.1.y)),
        "Position X" => num(Some(s.at.x)),
        "Position Y" => num(Some(s.at.y)),
        "Rotation" => num(Some(s.rotation)),
        "Contents" => Cell::Text(match k {
            EntityKind::Text(t) => t.value.clone(),
            EntityKind::AttDef(a) => a.text.value.clone(),
            EntityKind::MText(t) => cadaddy_fonts::plain_text(&t.contents).replace('\n', " "),
            _ => String::new(),
        }),
        "Text height" => num(match k {
            EntityKind::Text(t) => Some(t.height),
            EntityKind::AttDef(a) => Some(a.text.height),
            EntityKind::MText(t) => Some(t.height),
            _ => None,
        }),
        "Closed" => Cell::Text(match k {
            EntityKind::LwPolyline(p) => if p.closed { "Yes" } else { "No" }.into(),
            EntityKind::Polyline3d(p) => if p.closed { "Yes" } else { "No" }.into(),
            EntityKind::Spline(p) => if p.closed { "Yes" } else { "No" }.into(),
            _ => String::new(),
        }),
        _ => Cell::Text(String::new()),
    }
}

/// Where an object is, for its row: the insertion point of blocks and text, the centre of
/// circles and arcs, the start of other curves.
pub(crate) fn position(k: &EntityKind) -> Vec2 {
    match k {
        EntityKind::Text(t) => t.justify_point(),
        EntityKind::AttDef(a) => a.text.justify_point(),
        EntityKind::MText(t) => t.insert.xy(),
        EntityKind::Insert(i) => i.insert.xy(),
        EntityKind::Point(p) => p.p.xy(),
        _ => center(k).or_else(|| crate::cmd::curves::Chain::of(k).and_then(|c| c.start())).unwrap_or(Vec2::new(0.0, 0.0)),
    }
}

/// An object's own rotation (degrees), where it has one.
pub(crate) fn rotation(k: &EntityKind) -> f64 {
    match k {
        EntityKind::Text(t) => t.rotation.to_degrees(),
        EntityKind::MText(t) => t.rotation.to_degrees(),
        EntityKind::Insert(i) => i.rotation.to_degrees(),
        _ => 0.0,
    }
}
