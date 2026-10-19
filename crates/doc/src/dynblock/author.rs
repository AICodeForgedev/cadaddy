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
use crate::{Drawing, Handle, RawTag};

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
    },
    Xy {
        base: Vec3,
        end: Vec3,
    },
    Rotation {
        base: Vec3,
        end: Vec3,
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
            | NewParameter::Polar { base, end }
            | NewParameter::Xy { base, end }
            | NewParameter::Rotation { base, end }
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
    Stretch {
        frame: Vec<Vec2>,
    },
    Move,
    Scale,
    Rotate,
    Flip,
    Array,
    PolarStretch,
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
            NewActionKind::Array => "BLOCKARRAYACTION",
            NewActionKind::PolarStretch => "BLOCKPOLARSTRETCHACTION",
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

/// The graph's members (handle and graph id), in order.
fn members(d: &Drawing, graph: Handle) -> Vec<(Handle, i32)> {
    let Some(g) = d.foreign.objects.iter().find(|o| o.handle == graph) else { return Vec::new() };
    let mut out = Vec::new();
    let mut node = 0;
    for t in &g.tags {
        match t.code {
            91 => node = t.value.trim().parse().unwrap_or(0),
            360 => {
                if let Some(h) = Handle::parse_hex(&t.value) {
                    out.push((h, node));
                }
            }
            _ => {}
        }
    }
    out
}

/// Rewrite the graph object with `members`.
fn write_graph(d: &mut Drawing, graph: Handle, dict: Handle, members: &[(Handle, i32)]) {
    let mut tags = vec![
        tag(5, graph.hex()),
        tag(330, dict.hex()),
        tag(100, "AcDbEvalGraph"),
        tag(96, members.len().to_string()),
        tag(97, members.len().to_string()),
    ];
    for (h, node) in members {
        tags.push(tag(91, node.to_string()));
        tags.push(tag(93, "32"));
        for _ in 0..4 {
            tags.push(tag(95, "-1"));
        }
        tags.push(tag(360, h.hex()));
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
            m.parameters.iter().any(|p| p.element.eq_ignore_ascii_case(n) || p.kind.label().is_some_and(|l| l.eq_ignore_ascii_case(n)))
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
    if let NewParameter::Linear { base, end, values, .. } = p {
        if (*end - *base).len() < 1e-9 {
            return Err("A linear parameter needs two different points.".into());
        }
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
        NewParameter::Xy { .. } => "Distance",
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
        NewActionKind::Array => "Array",
        NewActionKind::PolarStretch => "Polar Stretch",
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
        NewActionKind::Stretch { frame } => {
            if frame.len() < 3 || frame.len() > 1000 || frame.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
                return Err("A stretch needs a frame of at least 3 points.".into());
            }
            tags.push(tag(100, "AcDbBlockStretchAction"));
            tags.extend([tag(92, conns[0].0.to_string()), tag(301, conns[0].1), tag(93, conns[1].0.to_string()), tag(302, conns[1].1)]);
            tags.push(tag(72, frame.len().to_string()));
            for f in frame {
                tags.push(tag(1011, real(f.x)));
                tags.push(tag(1021, real(f.y)));
            }
            // Every vertex of the selected objects inside the frame moves (no vertex lists: the
            // whole object for objects without listed vertices).
            tags.push(tag(73, "0"));
            tags.extend([tag(75, "0"), tag(140, "1"), tag(141, "0")]);
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
        other => {
            let marker = match other {
                NewActionKind::Scale => "AcDbBlockScaleAction",
                NewActionKind::Rotate => "AcDbBlockRotateAction",
                NewActionKind::Flip => "AcDbBlockFlipAction",
                NewActionKind::Array => "AcDbBlockArrayAction",
                _ => "AcDbBlockPolarStretchAction",
            };
            tags.push(tag(100, marker));
            tags.extend([tag(92, node.to_string()), tag(301, "UpdatedBase"), tag(93, node.to_string()), tag(302, "UpdatedEnd")]);
        }
    }
    let _ = p;
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
