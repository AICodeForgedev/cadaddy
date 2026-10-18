//! GROUPDISPLAYMODE: how a selected group shows. 0 shows every member's grips; 1 shows one grip
//! at the group's centre instead; 2 (the default) shows that grip and the group's bounding box.
//! The single grip moves the whole group ([`Session::start_group_grip`], one undo step).
//!
//! A group shows this way when group selection is on (PICKSTYLE bit 1), it is selectable, and
//! every one of its members in the space being edited is selected.

use std::collections::HashSet;

use cadaddy_doc::{Handle, entity_bounds};
use cadaddy_geom::{Bounds2, Mat3, Vec2};

use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Running, Session, Step};

/// Most groups shown with a single grip at once.
const MAX_SHOWN: usize = 500;

/// A selected group as GROUPDISPLAYMODE 1 or 2 shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownGroup {
    /// Its index in the drawing's groups.
    pub index: usize,
    pub name: String,
    /// Its members in the space being edited.
    pub members: Vec<Handle>,
    /// Their bounding box (drawn with GROUPDISPLAYMODE 2) and its centre (the group's grip).
    pub bounds: Bounds2,
    pub center: Vec2,
}

/// The selected groups shown with one grip (none with GROUPDISPLAYMODE 0 or group selection off).
pub fn shown_groups(s: &Session) -> Vec<ShownGroup> {
    if s.settings.groupdisplaymode == 0 || s.settings.pickstyle & 1 == 0 {
        return Vec::new();
    }
    let Ok(d) = s.doc() else { return Vec::new() };
    if d.groups.is_empty() {
        return Vec::new();
    }
    let sel: HashSet<Handle> = s.selection().into_iter().collect();
    if sel.is_empty() {
        return Vec::new();
    }
    let space = s.space();
    let Some(store) = d.space(&space) else { return Vec::new() };
    let mut out = Vec::new();
    for (index, g) in d.groups.iter().enumerate().filter(|(_, g)| g.selectable) {
        let members: Vec<Handle> = g.members.iter().copied().filter(|h| store.get(*h).is_some_and(|e| d.is_visible(e))).collect();
        if members.is_empty() || !members.iter().all(|h| sel.contains(h)) {
            continue;
        }
        let bounds = members.iter().filter_map(|h| d.entity(*h)).fold(Bounds2::EMPTY, |a, e| a.union(&entity_bounds(d, e, 0)));
        let center = bounds.center();
        if bounds.is_empty() || !center.is_finite() {
            continue;
        }
        out.push(ShownGroup { index, name: g.name.clone(), members, bounds, center });
        if out.len() >= MAX_SHOWN {
            break;
        }
    }
    out
}

/// The objects whose own grips aren't shown: the members of the groups shown with one grip.
pub fn grips_hidden(s: &Session) -> HashSet<Handle> {
    shown_groups(s).into_iter().flat_map(|g| g.members).collect()
}

/// Dragging a group's grip: the whole group moves.
struct GroupGrip {
    members: Vec<Handle>,
    from: Vec2,
}

impl Interactive for GroupGrip {
    fn name(&self) -> &'static str {
        "GRIP"
    }

    fn begin(&mut self, _s: &mut Session) -> Result<Step> {
        Ok(Step::Continue)
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("** MOVE ** Specify move point", Accept::POINT).kw(&["eXit"]).base(self.from)
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let to = match i {
            Input::Point(p) => p,
            Input::Deferred(d) => d.at,
            Input::Keyword(k) if k == "eXit" => return Ok(Step::Done),
            Input::Enter | Input::Cancel => return Ok(Step::Done),
            _ => return Err(EngineError::Other("Requires a point.".into())),
        };
        if !to.is_finite() {
            return Err(EngineError::Other("Requires a valid point.".into()));
        }
        crate::cmd::modify::transform_entities(s, &self.members, &Mat3::translate(to - self.from), false)?;
        Ok(Step::Done)
    }

    fn drag(&self, _s: &Session, c: Vec2) -> Option<crate::prompt::Drag> {
        Some(crate::prompt::Drag {
            handles: self.members.clone(),
            matrix: Mat3::translate(c - self.from),
            extra: vec![crate::cmd::helpers::line(self.from, c)],
            clipboard: false,
        })
    }
}

impl Session {
    /// Start dragging the grip of the group at `index` (GROUPDISPLAYMODE 1 or 2; the UI's click
    /// on the group's grip). The group must be shown with one grip.
    pub fn start_group_grip(&mut self, index: usize) -> Result<()> {
        if self.running.is_some() {
            self.cancel();
        }
        let g = shown_groups(self).into_iter().find(|g| g.index == index).ok_or_else(|| EngineError::Other("the group isn't selected".into()))?;
        let machine = crate::guard::Guarded::wrap(Box::new(GroupGrip { members: g.members, from: g.center }));
        let st = self.state()?;
        let (before, selection_before) = (st.doc.clone(), st.selection.clone());
        self.running = Some(Running { id: "grip".into(), machine, before, selection_before });
        self.pending_window = None;
        self.point_mods.clear();
        self.echo("** MOVE **");
        Ok(())
    }
}
