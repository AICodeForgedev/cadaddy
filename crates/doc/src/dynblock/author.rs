//! Making a block dynamic: parameters, actions and visibility states added to a block's
//! evaluation graph, written as the objects [`super::dynamic_block`] reads (so a reference's
//! grips and properties work as for a dynamic block read from a file, and saving writes them with
//! the drawing's other kept data).
//!
//! The objects follow the layout the reader expects: an extension dictionary on the block's
//! BLOCK_RECORD with an `ACAD_ENHANCEDBLOCK` entry naming an `ACAD_EVALUATION_GRAPH` whose
//! members (360) are the parameters, their grips and the actions, each with its graph id
//! (`AcDbEvalExpr` 90) and name (`AcDbBlockElement` 300).

use cadaddy_geom::{Vec2, Vec3};

use super::{ParameterKind, ValueSet, dynamic_block};
use crate::foreign::ForeignObject;
use crate::{Drawing, Entity, EntityKind, Handle, RawTag};

fn tag(code: i32, value: impl Into<String>) -> RawTag {
    RawTag { code, value: value.into() }
}

fn real(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

fn point(tags: &mut Vec<RawTag>, x: i32, p: Vec3) {
    tags.push(tag(x, real(p.x)));
    tags.push(tag(x + 10, real(p.y)));
    tags.push(tag(x + 20, real(p.z)));
}

/// A new parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum NewParameter {
    Linear {
        base: Vec3,
        end: Vec3,
        values: ValueSet,
        from_midpoint: bool,
    },
    Point {
        at: Vec3,
    },
    Polar {
        base: Vec3,
        end: Vec3,
        values: ValueSet,
    },
    Xy {
        base: Vec3,
        end: Vec3,
        values: ValueSet,
    },
    Rotation {
        base: Vec3,
        end: Vec3,
        values: ValueSet,
    },
    Alignment {
        base: Vec3,
        end: Vec3,
    },
    Flip {
        base: Vec3,
        end: Vec3,
    },
    /// Named states of visible objects (the objects of the block it controls are all those
    /// named in any state).
    Visibility {
        at: Vec3,
        states: Vec<(String, Vec<Handle>)>,
    },
    Lookup {
        at: Vec3,
    },
    BasePoint {
        at: Vec3,
    },
}

impl NewParameter {
    fn dxf_type(&self) -> &'static str {
        match self {
            NewParameter::Linear { .. } => "BLOCKLINEARPARAMETER",
            NewParameter::Point { .. } => "BLOCKPOINTPARAMETER",
            NewParameter::Polar { .. } => "BLOCKPOLARPARAMETER",
            NewParameter::Xy { .. } => "BLOCKXYPARAMETER",
            NewParameter::Rotation { .. } => "BLOCKROTATIONPARAMETER",
            NewParameter::Alignment { .. } => "BLOCKALIGNMENTPARAMETER",
            NewParameter::Flip { .. } => "BLOCKFLIPPARAMETER",
            NewParameter::Visibility { .. } => "BLOCKVISIBILITYPARAMETER",
            NewParameter::Lookup { .. } => "BLOCKLOOKUPPARAMETER",
            NewParameter::BasePoint { .. } => "BLOCKBASEPOINTPARAMETER",
        }
    }
    /// The grip object type, and how many grips (two-point parameters have two).
    fn grip(&self) -> (&'static str, usize) {
        match self {
            NewParameter::Linear { .. } => ("BLOCKLINEARGRIP", 2),
            NewParameter::Polar { .. } => ("BLOCKPOLARGRIP", 2),
            NewParameter::Xy { .. } => ("BLOCKXYGRIP", 2),
            NewParameter::Rotation { .. } => ("BLOCKROTATIONGRIP", 1),
            NewParameter::Alignment { .. } => ("BLOCKALIGNMENTGRIP", 1),
            NewParameter::Flip { .. } => ("BLOCKFLIPGRIP", 1),
            NewParameter::Point { .. } => ("BLOCKXYGRIP", 1),
            NewParameter::Visibility { .. } => ("BLOCKVISIBILITYGRIP", 1),
            NewParameter::Lookup { .. } => ("BLOCKLOOKUPGRIP", 1),
            NewParameter::BasePoint { .. } => ("BLOCKXYGRIP", 1),
        }
    }
    fn two_points(&self) -> Option<(Vec3, Vec3)> {
        match *self {
            NewParameter::Linear { base, end, .. }
            | NewParameter::Polar { base, end, .. }
            | NewParameter::Xy { base, end, .. }
            | NewParameter::Rotation { base, end, .. }
            | NewParameter::Alignment { base, end }
            | NewParameter::Flip { base, end } => Some((base, end)),
            _ => None,
        }
    }
    fn one_point(&self) -> Vec3 {
        match self {
            NewParameter::Point { at } | NewParameter::Visibility { at, .. } | NewParameter::Lookup { at } | NewParameter::BasePoint { at } => *at,
            _ => self.two_points().map_or(Vec3::default(), |p| p.0),
        }
    }
    fn is_finite(&self) -> bool {
        let ok = |p: Vec3| p.x.is_finite() && p.y.is_finite() && p.z.is_finite();
        match self.two_points() {
            Some((a, b)) => ok(a) && ok(b),
            None => ok(self.one_point()),
        }
    }
}

/// A new action.
#[derive(Clone, Debug, PartialEq)]
pub enum NewActionKind {
    /// Moves the vertices inside `frame` (and the whole objects inside it) by the parameter's change.
    /// `vertices` names, for objects crossing the frame, which of their vertices move (a line's 0
    /// and 1, a polyline's by index); a selected object not listed moves whole.
    Stretch {
        frame: Vec<Vec2>,
        vertices: Vec<(Handle, Vec<i32>)>,
    },
    Move,
    /// Scales by the change of a distance, about the parameter's base point.
    Scale,
    /// Rotates by the change of an angle, about the parameter's base point.
    Rotate,
    /// Mirrors about a flip parameter's reflection line while it is flipped.
    Flip,
    /// Copies along the parameter every `column_offset` (and, for an XY parameter, `row_offset`
    /// along its vertical distance).
    Array { column_offset: f64, row_offset: f64 },
    /// A stretch by a polar parameter's end point; the `rotate` objects turn about its base.
    PolarStretch { frame: Vec<Vec2>, vertices: Vec<(Handle, Vec<i32>)>, rotate: Vec<Handle> },
    /// Each row sets the columns' parameter values together.
    Lookup {
        columns: Vec<(i32, String, bool)>,
        rows: Vec<Vec<String>>,
    },
}

impl NewActionKind {
    fn dxf_type(&self) -> &'static str {
        match self {
            NewActionKind::Stretch { .. } => "BLOCKSTRETCHACTION",
            NewActionKind::Move => "BLOCKMOVEACTION",
            NewActionKind::Scale => "BLOCKSCALEACTION",
            NewActionKind::Rotate => "BLOCKROTATEACTION",
            NewActionKind::Flip => "BLOCKFLIPACTION",
            NewActionKind::Array { .. } => "BLOCKARRAYACTION",
            NewActionKind::PolarStretch { .. } => "BLOCKPOLARSTRETCHACTION",
            NewActionKind::Lookup { .. } => "BLOCKLOOKUPACTION",
        }
    }
}

/// Most elements one authored block takes, and most objects one action or state lists.
const MAX_ELEMENTS: usize = 2_000;
const MAX_LIST: usize = 100_000;

/// The kept-data handles of block `name`'s dynamic block: its record, extension dictionary and
/// evaluation graph, made when missing.
fn ensure_graph(d: &mut Drawing, name: &str) -> Result<(Handle, Handle, Handle), String> {
    let up = name.trim().to_ascii_uppercase();
    if d.block(name).is_none() {
        return Err(format!("Block \"{}\" not found.", name.trim()));
    }
    let key = ("BLOCK_RECORD".to_string(), up.clone());
    let record = match d.foreign.targets.iter().find(|(_, k)| **k == key).map(|(h, _)| *h) {
        Some(h) => h,
        None => {
            let h = d.new_handle();
            std::sync::Arc::make_mut(&mut d.foreign).targets.insert(h, key.clone());
            h
        }
    };
    let xdict = d.foreign.records.get(&key).and_then(|x| x.xdict);
    let graph_of = |d: &Drawing, dict: Handle| -> Option<Handle> {
        let o = d.foreign.objects.iter().find(|o| o.handle == dict)?;
        let mut it = o.tags.iter();
        while let Some(t) = it.next() {
            if t.code == 3 && t.value.eq_ignore_ascii_case("ACAD_ENHANCEDBLOCK") {
                return it.next().and_then(|t| Handle::parse_hex(&t.value));
            }
        }
        None
    };
    if let Some(dict) = xdict
        && let Some(g) = graph_of(d, dict)
    {
        tag_objects(d, name);
        return Ok((record, dict, g));
    }
    let dict = xdict.unwrap_or_else(|| d.new_handle());
    let graph = d.new_handle();
    tag_objects(d, name);
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    match f.objects.iter_mut().find(|o| o.handle == dict) {
        Some(o) => {
            o.tags.push(tag(3, "ACAD_ENHANCEDBLOCK"));
            o.tags.push(tag(360, graph.hex()));
        }
        None => f.objects.push(ForeignObject {
            handle: dict,
            kind: "DICTIONARY".into(),
            tags: vec![
                tag(5, dict.hex()),
                tag(330, record.hex()),
                tag(100, "AcDbDictionary"),
                tag(280, "1"),
                tag(281, "1"),
                tag(3, "ACAD_ENHANCEDBLOCK"),
                tag(360, graph.hex()),
            ],
        }),
    }
    f.objects.push(ForeignObject { handle: graph, kind: "ACAD_EVALUATION_GRAPH".into(), tags: Vec::new() });
    f.records.entry(key).or_default().xdict = Some(dict);
    write_graph(d, graph, dict, &[]);
    Ok((record, dict, graph))
}

/// Xdata application pairing an object of a dynamic block with its copies in references.
const ETAG: &str = "AcDbBlockRepETag";

/// Give every object of block `name` an element tag (`AcDbBlockRepETag` 1071) if it has none,
/// so the copies a reference makes stay paired with it.
fn tag_objects(d: &mut Drawing, name: &str) {
    let Some(b) = d.block(name) else { return };
    let handles: Vec<Handle> = b.entities.iter().map(|e| e.handle).collect();
    let tag_of = |d: &Drawing, h: Handle| -> Option<i64> {
        let x = &d.foreign.entities.get(&h)?.xdata;
        let at = x.iter().position(|t| t.code == 1001 && t.value.eq_ignore_ascii_case(ETAG))?;
        x.iter().skip(at + 1).take_while(|t| t.code != 1001).find(|t| t.code == 1071).and_then(|t| t.value.trim().parse().ok())
    };
    let mut next = handles.iter().filter_map(|h| tag_of(d, *h)).max().unwrap_or(0);
    let missing: Vec<Handle> = handles.into_iter().filter(|h| tag_of(d, *h).is_none()).collect();
    if missing.is_empty() {
        return;
    }
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    for h in missing {
        next = next.saturating_add(1);
        let x = f.entities.entry(h).or_default();
        x.xdata.extend([tag(1001, ETAG), tag(1070, "1"), tag(1071, next.to_string()), tag(1005, h.hex())]);
    }
}

/// The graph's members (handle and graph id), in order. Each member is a group of `91` (its place
/// in the graph), `93` (flags), `95` (its graph id), `360` (the object) and four `92` edge slots;
/// an older layout of ours had the id in `91` and `-1` in `95`, which is still read.
fn members(d: &Drawing, graph: Handle) -> Vec<(Handle, i32)> {
    let Some(g) = d.foreign.objects.iter().find(|o| o.handle == graph) else { return Vec::new() };
    let mut out = Vec::new();
    let (mut index, mut node) = (0, None);
    for t in &g.tags {
        match t.code {
            91 => {
                index = t.value.trim().parse().unwrap_or(0);
                node = None;
            }
            95 => node = t.value.trim().parse::<i32>().ok().filter(|n| *n >= 0),
            360 => {
                if let Some(h) = Handle::parse_hex(&t.value) {
                    out.push((h, node.unwrap_or(index)));
                }
            }
            _ => {}
        }
    }
    out
}

/// Rewrite the graph object with `members`, in the layout AutoCAD writes: for each member its
/// place in the graph (`91`, from 0), flags (`93`), graph id (`95`), the object (`360`) and the
/// four edge slots (`92`, none).
fn write_graph(d: &mut Drawing, graph: Handle, dict: Handle, members: &[(Handle, i32)]) {
    let mut tags = vec![
        tag(5, graph.hex()),
        tag(330, dict.hex()),
        tag(100, "AcDbEvalGraph"),
        tag(96, members.len().to_string()),
        tag(97, members.len().to_string()),
    ];
    for (i, (h, node)) in members.iter().enumerate() {
        tags.push(tag(91, i.to_string()));
        tags.push(tag(93, "32"));
        tags.push(tag(95, node.to_string()));
        tags.push(tag(360, h.hex()));
        for _ in 0..4 {
            tags.push(tag(92, "-1"));
        }
    }
    if let Some(o) = std::sync::Arc::make_mut(&mut d.foreign).objects.iter_mut().find(|o| o.handle == graph) {
        o.tags = tags;
    }
}

/// An element's common head: handle, owner, graph id and name.
fn head(h: Handle, owner: Handle, node: i32, name: &str) -> Vec<RawTag> {
    vec![
        tag(5, h.hex()),
        tag(330, owner.hex()),
        tag(100, "AcDbEvalExpr"),
        tag(90, node.to_string()),
        tag(98, "33"),
        tag(99, "4"),
        tag(100, "AcDbBlockElement"),
        tag(300, name),
        tag(98, "33"),
        tag(99, "4"),
        tag(1071, "0"),
    ]
}

fn next_node(d: &Drawing, graph: Handle) -> i32 {
    members(d, graph).iter().map(|m| m.1).max().unwrap_or(0) + 1
}

/// A unique element name for the block: `prefix` then a number (`Distance1`, `Stretch2`).
fn unique_name(d: &Drawing, name: &str, prefix: &str) -> String {
    let taken = |n: &str| {
        dynamic_block(d, name).is_some_and(|m| {
            m.parameters.iter().any(|p| p.element.eq_ignore_ascii_case(n) || p.kind.labels().iter().any(|l| l.eq_ignore_ascii_case(n)))
                || m.actions.iter().any(|a| a.name.eq_ignore_ascii_case(n))
        })
    };
    (1..).map(|i| format!("{prefix}{i}")).find(|n| !taken(n)).unwrap_or_else(|| prefix.to_string())
}

/// Add parameter `p` to block `name`, labelled `label` (default `Distance1`, `Position1`,
/// `Visibility1`, …). Returns its graph id and label.
pub fn add_parameter(d: &mut Drawing, name: &str, p: &NewParameter, label: Option<&str>, description: &str) -> Result<(i32, String), String> {
    if !p.is_finite() {
        return Err("The parameter's points must be numbers.".into());
    }
    if let Some((base, end)) = p.two_points()
        && (end - base).len() < 1e-9
    {
        return Err(if matches!(p, NewParameter::Linear { .. }) {
            "A linear parameter needs two different points.".into()
        } else {
            "A parameter with two points needs two different ones.".into()
        });
    }
    if let NewParameter::Linear { values, .. } | NewParameter::Polar { values, .. } | NewParameter::Xy { values, .. } | NewParameter::Rotation { values, .. } = p {
        if values.list.len() > MAX_LIST
            || values.list.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || ![values.minimum, values.maximum, values.increment].iter().all(|v| v.is_finite())
        {
            return Err("The value set must be positive numbers.".into());
        }
    }
    let (record, dict, graph) = ensure_graph(d, name)?;
    let _ = (record, dict);
    let mut list = members(d, graph);
    if list.len() >= MAX_ELEMENTS {
        return Err("The block has too many parameters and actions.".into());
    }
    if matches!(p, NewParameter::Visibility { .. })
        && dynamic_block(d, name).is_some_and(|m| m.parameters.iter().any(|q| matches!(q.kind, ParameterKind::Visibility { .. })))
    {
        return Err("A block has one visibility parameter; add states to it (BVSTATE).".into());
    }
    let prefix = match p {
        NewParameter::Linear { .. } => "Distance",
        NewParameter::Point { .. } => "Position",
        NewParameter::Polar { .. } => "Distance",
        NewParameter::Xy { .. } => "Horizontal",
        NewParameter::Rotation { .. } => "Angle",
        NewParameter::Alignment { .. } => "Alignment",
        NewParameter::Flip { .. } => "Flip state",
        NewParameter::Visibility { .. } => "Visibility",
        NewParameter::Lookup { .. } => "Lookup",
        NewParameter::BasePoint { .. } => "Base point",
    };
    let label = match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => l.chars().take(255).collect(),
        None => unique_name(d, name, prefix),
    };
    // A polar parameter's angle and an XY parameter's vertical distance have names of their own.
    let second = match p {
        NewParameter::Polar { .. } => {
            let n = unique_name(d, name, "Angle");
            if n.eq_ignore_ascii_case(&label) { format!("{n}b") } else { n }
        }
        NewParameter::Xy { .. } => {
            let n = unique_name(d, name, "Vertical");
            if n.eq_ignore_ascii_case(&label) { format!("{n}b") } else { n }
        }
        _ => String::new(),
    };
    let node = next_node(d, graph);
    let (grip_type, grips) = p.grip();
    let grip_nodes: Vec<i32> = (0..grips as i32).map(|i| node + 1 + i).collect();
    let h = d.new_handle();
    let mut tags = head(h, graph, node, &label);
    tags.extend([tag(100, "AcDbBlockParameter"), tag(280, "1"), tag(281, "0")]);
    match p.two_points() {
        Some((base, end)) => {
            tags.push(tag(100, "AcDbBlock2PtParameter"));
            point(&mut tags, 1010, base);
            point(&mut tags, 1011, end);
            tags.push(tag(170, grips.to_string()));
            for g in &grip_nodes {
                tags.push(tag(91, g.to_string()));
            }
            let mid = matches!(p, NewParameter::Linear { from_midpoint: true, .. });
            tags.push(tag(177, if mid { "1" } else { "0" }));
        }
        None => {
            tags.push(tag(100, "AcDbBlock1PtParameter"));
            point(&mut tags, 1010, p.one_point());
            tags.push(tag(93, grip_nodes.first().copied().unwrap_or(0).to_string()));
        }
    }
    match p {
        NewParameter::Linear { values, .. } => {
            tags.extend([tag(100, "AcDbBlockLinearParameter"), tag(305, label.clone()), tag(306, description), tag(307, "")]);
            tags.extend([
                tag(96, values.flags.to_string()),
                tag(141, real(values.minimum)),
                tag(142, real(values.maximum)),
                tag(143, real(values.increment)),
            ]);
            tags.push(tag(175, values.list.len().to_string()));
            tags.extend(values.list.iter().map(|v| tag(144, real(*v))));
        }
        // The subclass data of the parameters, as AutoCAD writes it.
        NewParameter::Polar { values, .. } => {
            tags.extend([tag(100, "AcDbBlockPolarParameter"), tag(305, label.clone()), tag(306, description)]);
            tags.extend([tag(307, second.clone()), tag(308, ""), tag(140, "0")]);
            value_set(&mut tags, values, (309, 96, 141, 175));
            value_set(&mut tags, &ValueSet::default(), (410, 97, 145, 176));
        }
        NewParameter::Xy { values, .. } => {
            // The vertical label first (305), the horizontal one second (306); the vertical
            // distance's value set, then the horizontal one's.
            tags.extend([tag(100, "AcDbBlockXYParameter"), tag(305, second.clone()), tag(306, label.clone())]);
            tags.extend([tag(307, ""), tag(308, description), tag(141, "0"), tag(140, "0")]);
            value_set(&mut tags, &ValueSet::default(), (309, 97, 146, 176));
            value_set(&mut tags, values, (410, 96, 142, 175));
        }
        NewParameter::Rotation { values, base, .. } => {
            tags.extend([tag(100, "AcDbBlockRotationParameter"), tag(305, label.clone()), tag(306, description)]);
            point(&mut tags, 1011, *base);
            tags.push(tag(140, "0"));
            value_set(&mut tags, values, (307, 96, 141, 175));
        }
        NewParameter::Alignment { .. } => tags.extend([tag(100, "AcDbBlockAlignmentParameter"), tag(280, "0")]),
        NewParameter::Flip { base, end } => {
            tags.extend([
                tag(100, "AcDbBlockFlipParameter"),
                tag(305, label.clone()),
                tag(306, description),
                tag(307, "Not flipped"),
                tag(308, "Flipped"),
            ]);
            point(&mut tags, 1012, (*base + *end) * 0.5);
            tags.extend([tag(309, ""), tag(96, "0")]);
        }
        NewParameter::Point { .. } => tags.extend([tag(100, "AcDbBlockPointParameter"), tag(303, label.clone()), tag(304, description)]),
        NewParameter::Lookup { .. } => tags.extend([tag(100, "AcDbBlockLookupParameter"), tag(303, label.clone()), tag(304, description)]),
        NewParameter::Visibility { states, .. } => {
            let mut controlled: Vec<Handle> = Vec::new();
            for (_, v) in states {
                for h in v.iter().take(MAX_LIST) {
                    if !controlled.contains(h) {
                        controlled.push(*h);
                    }
                }
            }
            tags.extend([tag(100, "AcDbBlockVisibilityParameter"), tag(301, label.clone()), tag(302, description)]);
            tags.push(tag(93, controlled.len().to_string()));
            tags.extend(controlled.iter().map(|h| tag(331, h.hex())));
            tags.push(tag(92, states.len().to_string()));
            for (n, v) in states {
                tags.push(tag(303, n.clone()));
                tags.push(tag(94, v.len().to_string()));
                tags.extend(v.iter().map(|h| tag(332, h.hex())));
                tags.push(tag(95, "0"));
            }
        }
        _ => {}
    }
    let mut objects = vec![ForeignObject { handle: h, kind: p.dxf_type().into(), tags }];
    list.push((h, node));
    let at = p.two_points().map_or(p.one_point(), |(_, e)| e);
    for (i, g) in grip_nodes.iter().enumerate() {
        let gh = d.new_handle();
        let mut gt = head(gh, graph, *g, "Grip");
        let loc = match (i, p.two_points()) {
            (0, Some((b, _))) => b,
            _ => at,
        };
        gt.extend([tag(100, "AcDbBlockGrip"), tag(91, node.to_string()), tag(92, i.to_string())]);
        point(&mut gt, 1010, loc);
        gt.push(tag(280, "0"));
        objects.push(ForeignObject { handle: gh, kind: grip_type.into(), tags: gt });
        list.push((gh, *g));
    }
    std::sync::Arc::make_mut(&mut d.foreign).objects.extend(objects);
    write_graph(d, graph, dict, &list);
    refresh_states(d, name)?;
    Ok((node, label))
}

/// Add an action to block `name` that follows parameter `node` and acts on `selection` (objects
/// of the block). Returns its graph id and name.
pub fn add_action(
    d: &mut Drawing,
    name: &str,
    node: i32,
    kind: &NewActionKind,
    selection: &[Handle],
    at: Vec3,
    label: Option<&str>,
) -> Result<(i32, String), String> {
    let m = dynamic_block(d, name).ok_or_else(|| format!("Block \"{}\" has no parameters yet; add one first (BPARAMETER).", name.trim()))?;
    let p = m.parameters.iter().find(|p| p.node == node).ok_or_else(|| format!("The block has no parameter {node}."))?;
    let b = d.block(name).ok_or("No such block.")?;
    if selection.len() > MAX_LIST || selection.iter().any(|h| b.entities.get(*h).is_none()) {
        return Err("The action's objects must be objects of the block.".into());
    }
    if selection.is_empty() && !matches!(kind, NewActionKind::Lookup { .. }) {
        return Err("Select the objects the action works on.".into());
    }
    let follows = match (kind, &p.kind) {
        (NewActionKind::Scale, ParameterKind::Linear { .. } | ParameterKind::Polar { .. }) => true,
        (NewActionKind::Rotate, ParameterKind::Rotation { .. } | ParameterKind::Polar { .. } | ParameterKind::Alignment { .. }) => true,
        (NewActionKind::Flip, ParameterKind::Flip { .. }) => true,
        (NewActionKind::Array { .. }, ParameterKind::Linear { .. } | ParameterKind::Polar { .. } | ParameterKind::Xy { .. }) => true,
        (NewActionKind::PolarStretch { .. }, ParameterKind::Polar { .. }) => true,
        (NewActionKind::Scale | NewActionKind::Rotate | NewActionKind::Flip | NewActionKind::Array { .. } | NewActionKind::PolarStretch { .. }, _) => false,
        _ => true,
    };
    if !follows {
        return Err(match kind {
            NewActionKind::Scale => "A scale action follows a distance (a linear or polar parameter).",
            NewActionKind::Rotate => "A rotate action follows an angle (a rotation, polar or alignment parameter).",
            NewActionKind::Flip => "A flip action follows a flip parameter.",
            NewActionKind::Array { .. } => "An array action follows a distance (a linear, polar or XY parameter).",
            _ => "A polar stretch action follows a polar parameter.",
        }
        .into());
    }
    let (_, dict, graph) = ensure_graph(d, name)?;
    let mut list = members(d, graph);
    if list.len() >= MAX_ELEMENTS {
        return Err("The block has too many parameters and actions.".into());
    }
    let prefix = match kind {
        NewActionKind::Stretch { .. } => "Stretch",
        NewActionKind::Move => "Move",
        NewActionKind::Scale => "Scale",
        NewActionKind::Rotate => "Rotate",
        NewActionKind::Flip => "Flip",
        NewActionKind::Array { .. } => "Array",
        NewActionKind::PolarStretch { .. } => "Polar Stretch",
        NewActionKind::Lookup { .. } => "Lookup",
    };
    let label = match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => l.chars().take(255).collect(),
        None => unique_name(d, name, prefix),
    };
    let me = next_node(d, graph);
    let h = d.new_handle();
    let mut tags = head(h, graph, me, &label);
    tags.extend([tag(100, "AcDbBlockAction"), tag(70, "0"), tag(71, selection.len().to_string())]);
    tags.extend(selection.iter().map(|s| tag(330, s.hex())));
    point(&mut tags, 1010, at);
    // A distance moves its end point (both from the midpoint): follow the end's X and Y.
    let conns = [(node, "UpdatedEndX"), (node, "UpdatedEndY")];
    match kind {
        NewActionKind::Stretch { frame, vertices } => {
            tags.push(tag(100, "AcDbBlockStretchAction"));
            stretch_body(&mut tags, conns, frame, vertices, selection)?;
            tags.extend([tag(75, "0"), tag(140, "1"), tag(141, "0")]);
        }
        NewActionKind::PolarStretch { frame, vertices, rotate } => {
            if rotate.len() > MAX_LIST || rotate.iter().any(|h| d.block(name).and_then(|b| b.entities.get(*h)).is_none()) {
                return Err("The objects to rotate must be objects of the block.".into());
            }
            if frame.len() < 3 || frame.len() > 1000 || frame.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
                return Err("A stretch needs a frame of at least 3 points.".into());
            }
            tags.push(tag(100, "AcDbBlockPolarStretchAction"));
            let names = ["UpdatedEndX", "UpdatedEndY", "UpdatedBaseX", "UpdatedBaseY", "UpdatedAngle", "UpdatedDistance"];
            tags.extend((0..6).map(|i| tag(92 + i, node.to_string())));
            tags.extend(names.iter().enumerate().map(|(i, n)| tag(301 + i as i32, *n)));
            tags.push(tag(73, frame.len().to_string()));
            for f in frame {
                tags.push(tag(1011, real(f.x)));
                tags.push(tag(1021, real(f.y)));
            }
            // The objects that stretch (the selection but those that only turn), each with the
            // vertices of it that move when it crosses the frame.
            let stretched: Vec<&Handle> = selection.iter().filter(|h| !rotate.contains(h)).collect();
            tags.push(tag(72, stretched.len().to_string()));
            tags.extend(stretched.iter().map(|h| tag(331, h.hex())));
            let listed: Vec<&(Handle, Vec<i32>)> = vertices.iter().filter(|(h, _)| stretched.contains(&h)).take(MAX_LIST).collect();
            tags.push(tag(74, listed.len().to_string()));
            for (h, idx) in listed {
                tags.push(tag(332, h.hex()));
                tags.push(tag(75, idx.len().min(MAX_LIST).to_string()));
                tags.extend(idx.iter().take(MAX_LIST).map(|i| tag(76, i.to_string())));
            }
            tags.extend([tag(78, "0"), tag(141, "1"), tag(140, "0"), tag(77, "0")]);
        }
        NewActionKind::Array { column_offset, row_offset } => {
            if !(column_offset.is_finite() && *column_offset > 0.0 && row_offset.is_finite() && *row_offset >= 0.0) {
                return Err("An array needs a column offset above 0 (and a row offset of 0 or more).".into());
            }
            if matches!(p.kind, ParameterKind::Xy { .. }) && *row_offset <= 0.0 {
                return Err("An array along an XY parameter needs a row offset above 0.".into());
            }
            tags.push(tag(100, "AcDbBlockArrayAction"));
            let (c0, c1) = if matches!(p.kind, ParameterKind::Xy { .. }) { ("UpdatedXDistance", "UpdatedYDistance") } else { ("UpdatedDistance", "UpdatedDistance") };
            tags.extend([tag(92, node.to_string()), tag(93, node.to_string()), tag(94, "0"), tag(95, "0")]);
            tags.extend([tag(301, c0), tag(302, c1), tag(303, ""), tag(304, "")]);
            tags.extend([tag(140, real(*row_offset)), tag(141, real(*column_offset))]);
        }
        NewActionKind::Move => {
            tags.push(tag(100, "AcDbBlockMoveAction"));
            tags.extend([tag(92, conns[0].0.to_string()), tag(301, conns[0].1), tag(93, conns[1].0.to_string()), tag(302, conns[1].1)]);
            tags.extend([tag(140, "1"), tag(141, "0")]);
        }
        NewActionKind::Lookup { columns, rows } => {
            if columns.is_empty() || rows.iter().any(|r| r.len() != columns.len()) || rows.len() > 10_000 || columns.len() > 100 {
                return Err("Every lookup row needs a value for each column.".into());
            }
            tags.push(tag(100, "AcDbBlockLookupAction"));
            tags.extend([tag(92, rows.len().to_string()), tag(93, columns.len().to_string()), tag(301, "")]);
            for r in rows {
                tags.extend(r.iter().map(|c| tag(302, c.clone())));
            }
            for (pnode, property, lookup) in columns {
                tags.extend([
                    tag(303, ""),
                    tag(94, pnode.to_string()),
                    tag(282, if *lookup { "1" } else { "0" }),
                    tag(305, "Custom"),
                    tag(304, property.clone()),
                ]);
            }
        }
        NewActionKind::Flip => {
            tags.push(tag(100, "AcDbBlockFlipAction"));
            tags.extend([tag(92, node.to_string()), tag(93, "0"), tag(94, "0"), tag(95, "0")]);
            tags.extend([tag(301, "UpdatedFlipState"), tag(302, ""), tag(303, ""), tag(304, "")]);
        }
        scaling @ (NewActionKind::Scale | NewActionKind::Rotate) => {
            // Both turn or scale about a base point (the parameter's own, dependent on it).
            let base = p.kind.base_point();
            tags.push(tag(100, "AcDbBlockActionWithBasePt"));
            tags.extend([tag(92, node.to_string()), tag(93, node.to_string()), tag(301, "UpdatedBaseX"), tag(302, "UpdatedBaseY")]);
            point(&mut tags, 1011, Vec3::default());
            tags.push(tag(280, "1"));
            point(&mut tags, 1012, base);
            if matches!(scaling, NewActionKind::Rotate) {
                // (AutoCAD's marker for the rotate action's own data is AcDbBlockRotationAction.)
                tags.extend([tag(100, "AcDbBlockRotationAction"), tag(94, node.to_string()), tag(303, "UpdatedAngle")]);
            } else {
                tags.extend([tag(100, "AcDbBlockScaleAction"), tag(94, node.to_string()), tag(95, "0"), tag(96, "0")]);
                tags.extend([tag(303, "UpdatedDistance"), tag(304, ""), tag(305, "")]);
            }
        }
    }
    std::sync::Arc::make_mut(&mut d.foreign).objects.push(ForeignObject { handle: h, kind: kind.dxf_type().into(), tags });
    list.push((h, me));
    write_graph(d, graph, dict, &list);
    refresh_states(d, name)?;
    Ok((me, label))
}

/// Remove parameter or action `node` (and a parameter's grips and the actions following it).
pub fn remove(d: &mut Drawing, name: &str, node: i32) -> Result<(), String> {
    let m = dynamic_block(d, name).ok_or("The block has no parameters or actions.")?;
    let (_, dict, graph) = ensure_graph(d, name)?;
    let mut gone: Vec<Handle> = Vec::new();
    if let Some(p) = m.parameters.iter().find(|p| p.node == node) {
        gone.push(p.handle);
        let grips: Vec<Handle> = m.grip_objects.iter().filter(|(g, _)| p.grips.contains(g)).map(|(_, h)| *h).collect();
        gone.extend(grips);
        gone.extend(
            m.actions
                .iter()
                .filter(|a| {
                    a.connections.iter().any(|(n, _)| *n == node)
                        || matches!(&a.kind, super::ActionKind::Lookup { columns, .. } if columns.iter().any(|c| c.node == node))
                })
                .map(|a| a.handle),
        );
    } else if let Some(a) = m.actions.iter().find(|a| a.node == node) {
        gone.push(a.handle);
    } else {
        return Err(format!("The block has no parameter or action {node}."));
    }
    let list: Vec<(Handle, i32)> = members(d, graph).into_iter().filter(|(h, _)| !gone.contains(h)).collect();
    std::sync::Arc::make_mut(&mut d.foreign).objects.retain(|o| !gone.contains(&o.handle));
    write_graph(d, graph, dict, &list);
    refresh_states(d, name)
}

/// The visibility states of block `name`: replace them with `states` ((name, visible objects)).
pub fn set_states(d: &mut Drawing, name: &str, states: &[(String, Vec<Handle>)]) -> Result<(), String> {
    let m = dynamic_block(d, name).ok_or("The block has no visibility parameter (BPARAMETER Visibility).")?;
    let p = m
        .parameters
        .iter()
        .find(|p| matches!(p.kind, ParameterKind::Visibility { .. }))
        .ok_or("The block has no visibility parameter (BPARAMETER Visibility).")?;
    let b = d.block(name).ok_or("No such block.")?;
    if states.is_empty() || states.len() > 1000 {
        return Err("A visibility parameter has 1 to 1000 states.".into());
    }
    for (n, v) in states {
        if n.trim().is_empty() || v.iter().any(|h| b.entities.get(*h).is_none()) {
            return Err("Each state needs a name and objects of the block.".into());
        }
    }
    let ph = p.handle;
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    let Some(o) = f.objects.iter_mut().find(|o| o.handle == ph) else { return Err("The visibility parameter is missing.".into()) };
    let at = o.tags.iter().position(|t| t.code == 100 && t.value == "AcDbBlockVisibilityParameter").ok_or("The visibility parameter is damaged.")?;
    let (label, description) = {
        let rest = &o.tags[at + 1..];
        (
            rest.iter().find(|t| t.code == 301).map(|t| t.value.clone()).unwrap_or_default(),
            rest.iter().find(|t| t.code == 302).map(|t| t.value.clone()).unwrap_or_default(),
        )
    };
    o.tags.truncate(at + 1);
    let mut controlled: Vec<Handle> = Vec::new();
    for (_, v) in states {
        for h in v {
            if !controlled.contains(h) {
                controlled.push(*h);
            }
        }
    }
    o.tags.extend([tag(301, label), tag(302, description), tag(93, controlled.len().to_string())]);
    o.tags.extend(controlled.iter().map(|h| tag(331, h.hex())));
    o.tags.push(tag(92, states.len().to_string()));
    for (n, v) in states {
        o.tags.push(tag(303, n.trim()));
        o.tags.push(tag(94, v.len().to_string()));
        o.tags.extend(v.iter().map(|h| tag(332, h.hex())));
        o.tags.push(tag(95, "0"));
    }
    refresh_states(d, name)
}

/// Every visibility state lists every parameter, grip and action as active (they all work in
/// every state).
fn refresh_states(d: &mut Drawing, name: &str) -> Result<(), String> {
    let Some(m) = dynamic_block(d, name) else { return Ok(()) };
    let Some(p) = m.parameters.iter().find(|p| matches!(p.kind, ParameterKind::Visibility { .. })) else { return Ok(()) };
    let mut active: Vec<Handle> =
        m.parameters.iter().map(|p| p.handle).chain(m.actions.iter().map(|a| a.handle)).chain(m.grip_objects.iter().map(|g| g.1)).collect();
    active.dedup();
    let ph = p.handle;
    let f = std::sync::Arc::make_mut(&mut d.foreign);
    let Some(o) = f.objects.iter_mut().find(|o| o.handle == ph) else { return Ok(()) };
    let mut out: Vec<RawTag> = Vec::with_capacity(o.tags.len());
    let mut skip = false;
    for t in o.tags.drain(..) {
        if t.code == 95 {
            out.push(tag(95, active.len().to_string()));
            out.extend(active.iter().map(|h| tag(333, h.hex())));
            skip = true;
            continue;
        }
        if skip && t.code == 333 {
            continue;
        }
        skip = false;
        out.push(t);
    }
    o.tags = out;
    Ok(())
}

/// A value set's groups: its description, flags, minimum, maximum and increment (from `first`),
/// and the count and values of its list (`first + 3` each).
fn value_set(tags: &mut Vec<RawTag>, v: &ValueSet, (description, flags, first, count): (i32, i32, i32, i32)) {
    tags.extend([tag(description, ""), tag(flags, v.flags.to_string())]);
    tags.extend([tag(first, real(v.minimum)), tag(first + 1, real(v.maximum)), tag(first + 2, real(v.increment))]);
    tags.push(tag(count, v.list.len().to_string()));
    tags.extend(v.list.iter().map(|x| tag(first + 3, real(*x))));
}

/// Whether `p` lies in a stretch frame: an axis-aligned rectangle (or two corners) takes its
/// edges, any other polygon the usual inside test.
fn in_stretch_frame(frame: &[Vec2], p: Vec2) -> bool {
    let rect = match frame {
        [a, b] => Some((*a, *b)),
        [a, b, c, d] if (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y) || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x) => Some((*a, *c)),
        _ => None,
    };
    match rect {
        Some((a, b)) => p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y),
        None => super::apply::in_frame(frame, p),
    }
}

/// What a stretch frame moves of `items` (handle, object): lines and polylines with a vertex in
/// the frame (with the vertices that move when not all do), and other objects whose reference
/// points are all in it, which move whole. Objects outside the frame, and solids and the like
/// that are half in it (they would move whole), are left out: they stay where they are.
pub fn stretch_selection<'a>(items: impl IntoIterator<Item = (Handle, &'a Entity)>, frame: &[Vec2]) -> (Vec<Handle>, Vec<(Handle, Vec<i32>)>) {
    let mut sel = Vec::new();
    let mut verts = Vec::new();
    for (h, e) in items {
        let pts: Vec<Vec2> = match &e.kind {
            EntityKind::Line(l) => vec![l.a.xy(), l.b.xy()],
            EntityKind::LwPolyline(p) => p.vertices.iter().map(|v| v.p).collect(),
            EntityKind::Circle(c) => vec![c.center.xy()],
            EntityKind::Arc(c) => vec![c.center.xy()],
            EntityKind::Ellipse(c) => vec![c.center.xy()],
            EntityKind::Text(t) => vec![t.align_pt.unwrap_or(t.insert).xy()],
            EntityKind::Solid(s) => s.corners.iter().map(|c| c.xy()).collect(),
            // Bodies, attribute definitions: left as they are.
            _ => continue,
        };
        let idx: Vec<i32> = pts.iter().enumerate().filter(|(_, p)| in_stretch_frame(frame, **p)).filter_map(|(i, _)| i32::try_from(i).ok()).collect();
        if idx.is_empty() {
            continue;
        }
        let partial = idx.len() < pts.len();
        if partial && !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) {
            continue;
        }
        sel.push(h);
        if partial {
            verts.push((h, idx));
        }
    }
    (sel, verts)
}

/// The groups a stretch and a polar stretch action share: the connections, the frame and the
/// vertices of the objects crossing it.
fn stretch_body(
    tags: &mut Vec<RawTag>,
    conns: [(i32, &str); 2],
    frame: &[Vec2],
    vertices: &[(Handle, Vec<i32>)],
    selection: &[Handle],
) -> Result<(), String> {
    if frame.len() < 3 || frame.len() > 1000 || frame.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return Err("A stretch needs a frame of at least 3 points.".into());
    }
    tags.extend([tag(92, conns[0].0.to_string()), tag(301, conns[0].1), tag(93, conns[1].0.to_string()), tag(302, conns[1].1)]);
    tags.push(tag(72, frame.len().to_string()));
    for f in frame {
        tags.push(tag(1011, real(f.x)));
        tags.push(tag(1021, real(f.y)));
    }
    // The listed objects move the vertices named; the other selected objects move whole.
    let listed: Vec<&(Handle, Vec<i32>)> = vertices.iter().filter(|(h, _)| selection.contains(h)).take(MAX_LIST).collect();
    tags.push(tag(73, listed.len().to_string()));
    for (h, idx) in listed {
        tags.push(tag(331, h.hex()));
        tags.push(tag(74, idx.len().min(MAX_LIST).to_string()));
        tags.extend(idx.iter().take(MAX_LIST).map(|i| tag(94, i.to_string())));
    }
    Ok(())
}
