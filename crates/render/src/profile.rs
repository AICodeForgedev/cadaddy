//! Where a display-list build spends its time: per kind of object, the time spent drawing it
//! (curve and linetype tessellation, text, hatches, dimensions, block references), and how many
//! objects the build looked at and how many drew something.
//!
//! Profiling is off unless [`Options::profile`] holds a [`BuildProfile`]; each builder then keeps
//! its own counts (no shared writes per object, so parallel builds stay parallel) and adds them to
//! the profile when it is done. Times are exclusive: a block reference's time is its own work,
//! the objects inside it count as their own kind. They are CPU time summed over the threads that
//! built the list, so on a parallel build they add up to more than the wall-clock time.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cadaddy_doc::EntityKind;

use crate::Options;

/// What kind of object a build was drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Lines, arcs, circles, ellipses, polylines and splines: curve and linetype tessellation.
    Curves,
    /// Single-line and multiline text and attribute definitions.
    Text,
    Hatch,
    /// Dimensions, leaders, tolerances and tables (their text included).
    Dimension,
    /// Block references themselves (attributes, arrays, clipping); their contents count as their
    /// own kinds.
    Block,
    /// Everything else: fills, images, points, meshes, solids, wipeouts.
    Other,
}

const PARTS: usize = 6;

impl Part {
    pub const ALL: [Part; PARTS] = [Part::Curves, Part::Text, Part::Hatch, Part::Dimension, Part::Block, Part::Other];

    pub fn name(self) -> &'static str {
        match self {
            Part::Curves => "curves",
            Part::Text => "text",
            Part::Hatch => "hatch",
            Part::Dimension => "dimensions",
            Part::Block => "blocks",
            Part::Other => "other",
        }
    }

    pub fn of(kind: &EntityKind) -> Part {
        match kind {
            EntityKind::Line(_)
            | EntityKind::Circle(_)
            | EntityKind::Arc(_)
            | EntityKind::Ellipse(_)
            | EntityKind::LwPolyline(_)
            | EntityKind::Polyline3d(_)
            | EntityKind::Spline(_)
            | EntityKind::Helix(_)
            | EntityKind::Ray(_)
            | EntityKind::XLine(_)
            | EntityKind::MLine(_) => Part::Curves,
            EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::AttDef(_) => Part::Text,
            EntityKind::Hatch(_) => Part::Hatch,
            EntityKind::Dimension(_) | EntityKind::Leader(_) | EntityKind::MLeader(_) | EntityKind::Tolerance(_) | EntityKind::Table(_) => {
                Part::Dimension
            }
            EntityKind::Insert(_) => Part::Block,
            _ => Part::Other,
        }
    }

    fn index(self) -> usize {
        match self {
            Part::Curves => 0,
            Part::Text => 1,
            Part::Hatch => 2,
            Part::Dimension => 3,
            Part::Block => 4,
            Part::Other => 5,
        }
    }
}

/// The counts of one or more builds (see the module docs). Shared through
/// [`Options::profile`]; read it with [`BuildProfile::report`].
#[derive(Debug, Default)]
pub struct BuildProfile {
    ns: [AtomicU64; PARTS],
    objects: [AtomicU64; PARTS],
    considered: AtomicU64,
    drawn: AtomicU64,
}

/// One kind of object in a [`ProfileReport`].
#[derive(Clone, Debug, PartialEq)]
pub struct PartTime {
    pub part: Part,
    /// CPU milliseconds (summed over threads).
    pub ms: f64,
    /// Objects of this kind drawn, at any depth (block contents included).
    pub objects: u64,
}

/// What a [`BuildProfile`] counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileReport {
    /// Top-level objects the build looked at. The build has no culling (only hatch patterns are
    /// limited to the view), so this is every object of the space.
    pub considered: u64,
    /// Top-level objects that drew something.
    pub drawn: u64,
    pub parts: Vec<PartTime>,
}

impl ProfileReport {
    /// CPU milliseconds of every part.
    pub fn total_ms(&self) -> f64 {
        self.parts.iter().map(|p| p.ms).sum()
    }
    /// CPU milliseconds of `part`.
    pub fn ms(&self, part: Part) -> f64 {
        self.parts.iter().find(|p| p.part == part).map_or(0.0, |p| p.ms)
    }
    /// Objects of `part` drawn.
    pub fn objects(&self, part: Part) -> u64 {
        self.parts.iter().find(|p| p.part == part).map_or(0, |p| p.objects)
    }
}

impl BuildProfile {
    pub fn new() -> Arc<BuildProfile> {
        Arc::new(BuildProfile::default())
    }

    pub fn report(&self) -> ProfileReport {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let parts = Part::ALL
            .iter()
            .map(|&part| {
                let i = part.index();
                PartTime { part, ms: self.ns.get(i).map_or(0, get) as f64 / 1e6, objects: self.objects.get(i).map_or(0, get) }
            })
            .collect();
        ProfileReport { considered: get(&self.considered), drawn: get(&self.drawn), parts }
    }
}

/// A started measurement: when it started and the time its caller's children had used so far.
pub(crate) struct Started {
    at: web_time::Instant,
    outer_child_ns: u64,
}

/// One builder's counts, added to the shared profile when it is dropped.
#[derive(Default)]
pub(crate) struct Local {
    shared: Option<Arc<BuildProfile>>,
    ns: [u64; PARTS],
    objects: [u64; PARTS],
    considered: u64,
    drawn: u64,
    /// Time of the objects drawn inside the one being measured (taken off its own time).
    child_ns: u64,
}

impl Local {
    /// Counts for a builder of `opts` (nothing when it has no profile).
    pub(crate) fn new(opts: &Options) -> Local {
        Local { shared: opts.profile.clone(), ..Local::default() }
    }

    /// No counts: for a builder working inside an object another builder is timing (its time is
    /// that object's).
    pub(crate) fn off() -> Local {
        Local::default()
    }

    /// Start timing an object (`None` when profiling is off).
    pub(crate) fn start(&mut self) -> Option<Started> {
        self.shared.as_ref()?;
        Some(Started { at: web_time::Instant::now(), outer_child_ns: std::mem::take(&mut self.child_ns) })
    }

    /// An object timed from `started` is done: it was of kind `part`, at the top level when
    /// `top`, and drew something when `drew`.
    pub(crate) fn stop(&mut self, started: Option<Started>, part: Part, top: bool, drew: bool) {
        let Some(s) = started else { return };
        let total = u64::try_from(s.at.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let own = total.saturating_sub(self.child_ns);
        let i = part.index();
        if let (Some(ns), Some(n)) = (self.ns.get_mut(i), self.objects.get_mut(i)) {
            *ns = ns.saturating_add(own);
            *n = n.saturating_add(u64::from(drew));
        }
        if top {
            self.considered += 1;
            self.drawn += u64::from(drew);
        }
        self.child_ns = s.outer_child_ns.saturating_add(total);
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        let Some(shared) = &self.shared else { return };
        for (i, (ns, n)) in self.ns.iter().zip(&self.objects).enumerate() {
            if let (Some(a), Some(b)) = (shared.ns.get(i), shared.objects.get(i)) {
                a.fetch_add(*ns, Ordering::Relaxed);
                b.fetch_add(*n, Ordering::Relaxed);
            }
        }
        shared.considered.fetch_add(self.considered, Ordering::Relaxed);
        shared.drawn.fetch_add(self.drawn, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Block, Common, Drawing, Entity, Handle, Insert, Line, Space, Text};
    use cadaddy_geom::Vec3;

    fn line(x: f64) -> EntityKind {
        EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x + 1.0, 1.0, 0.0) })
    }

    /// `lines` lines, `texts` texts and `inserts` references to a block of two lines.
    fn drawing(lines: usize, texts: usize, inserts: usize) -> Drawing {
        let mut d = Drawing::new_imperial();
        let mut b = Block::new("B");
        b.entities.push(Entity::new(Handle(0x50), line(0.0)));
        b.entities.push(Entity::new(Handle(0x51), line(2.0)));
        d.blocks.insert("B".into(), Arc::new(b));
        let mut add = |k: EntityKind| {
            let Ok(_) = d.add(&Space::Model, Common::default(), k) else { panic!("add") };
        };
        for i in 0..lines {
            add(line(i as f64 * 3.0));
        }
        for i in 0..texts {
            add(EntityKind::Text(Text {
                insert: Vec3::new(0.0, i as f64 * 2.0, 0.0),
                align_pt: None,
                height: 1.0,
                value: format!("T{i}"),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }));
        }
        for i in 0..inserts {
            add(EntityKind::Insert(Insert {
                block: "B".into(),
                insert: Vec3::new(i as f64 * 5.0, -10.0, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: 0.0,
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            }));
        }
        d
    }

    #[test]
    fn a_profiled_build_counts_objects_by_kind_and_block_contents_as_their_own() {
        let d = drawing(30, 5, 4);
        let profile = BuildProfile::new();
        let opts = Options { profile: Some(profile.clone()), ..Options::default() };
        let profiled = crate::build(&d, &Space::Model, &opts);
        let r = profile.report();
        assert_eq!((r.considered, r.drawn), (39, 39));
        // The block's two lines, four times, count as curves; the references as blocks.
        assert_eq!(r.objects(Part::Curves), 30 + 8);
        assert_eq!(r.objects(Part::Text), 5);
        assert_eq!(r.objects(Part::Block), 4);
        assert!(r.total_ms() >= 0.0 && r.parts.len() == Part::ALL.len());
        // The list is the one an unprofiled build gives.
        let plain = crate::build(&d, &Space::Model, &Options::default());
        assert_eq!(profiled.prims, plain.prims);
        assert_eq!(profiled.verts, plain.verts);
    }

    #[test]
    fn a_parallel_build_adds_up_every_part() {
        // Past two parallel chunks, the build runs on all cores; the parts still add up.
        let d = drawing(3000, 200, 100);
        let profile = BuildProfile::new();
        let _ = crate::build(&d, &Space::Model, &Options { profile: Some(profile.clone()), ..Options::default() });
        let r = profile.report();
        assert_eq!(r.considered, 3300);
        assert_eq!(r.objects(Part::Curves), 3000 + 200);
        assert_eq!(r.objects(Part::Text), 200);
        // A second build adds to the same profile.
        let _ = crate::build(&d, &Space::Model, &Options { profile: Some(profile.clone()), ..Options::default() });
        assert_eq!(profile.report().considered, 6600);
    }

    #[test]
    fn an_unprofiled_builder_counts_nothing() {
        let mut l = Local::off();
        let s = l.start();
        assert!(s.is_none());
        l.stop(s, Part::Curves, true, true);
        assert_eq!(l.considered, 0);
    }
}
