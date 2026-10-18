//! UNDO control: marks, BEgin/End groups and the Control/Auto settings of one drawing, kept on top
//! of its undo stack ([`DocState::undo`]). Every command records its step through
//! [`DocState::push_undo`], so marks, groups, Control and the stack limit apply to all of them
//! alike; a command never needs to know about them.

use crate::{DocState, Snapshot};

/// The most undo steps a drawing keeps; the oldest are dropped first.
pub const MAX_UNDO: usize = 2000;
/// The memory a drawing's undo steps may hold (what they don't share with the drawing as it is
/// now); past it the oldest steps are dropped, the most recent one always kept. A MOVE of every
/// object in a 190k-object drawing holds about 140 MB, so this keeps several such steps.
pub const UNDO_BUDGET: usize = 768 * 1024 * 1024;

/// Free undo or redo steps off the command path: dropping a step that held a whole changed copy
/// of a large drawing took half a second (they're freed on a short-lived thread instead; the web
/// build has no threads and frees them here).
fn drop_later(steps: Vec<Snapshot>) {
    if steps.is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::thread::Builder::new().name("undo-free".into()).spawn(move || drop(steps));
    }
    #[cfg(target_arch = "wasm32")]
    drop(steps);
}
/// The most marks a drawing keeps; the oldest are dropped first.
const MAX_MARKS: usize = 1000;

/// UNDO Control: how much undo information is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Control {
    /// Full undo (the default).
    #[default]
    All,
    /// Undo is off: nothing is recorded.
    None,
    /// Only the last step can be undone.
    One,
}

/// The UNDO settings and markers of one drawing.
#[derive(Clone, Debug)]
pub struct UndoCtl {
    pub control: Control,
    /// UNDO Auto. Every command already records one undo step for all it does, so this is kept and
    /// reported (UNDOCTL) only.
    pub auto: bool,
    /// UNDO Control Combine: consecutive zooms and pans as one step. View changes record no undo
    /// steps, so this is kept and reported only.
    pub combine_view: bool,
    /// UNDO Control Layer: layer property changes as one step. Kept and reported only.
    pub combine_layer: bool,
    /// Undo-stack depths at each UNDO Mark, oldest first.
    pub marks: Vec<usize>,
    /// Undo-stack depth when the open BEgin group started.
    pub group: Option<usize>,
    /// The memory the undo steps may hold ([`UNDO_BUDGET`] unless changed).
    pub budget: usize,
}

impl Default for UndoCtl {
    fn default() -> Self {
        UndoCtl { control: Control::All, auto: true, combine_view: true, combine_layer: true, marks: Vec::new(), group: None, budget: UNDO_BUDGET }
    }
}

impl UndoCtl {
    /// The UNDOCTL bit code: 1 undo on, 2 only one step, 4 Auto, 8 a group is open, 16 combine
    /// zooms and pans, 32 combine layer changes.
    pub fn bits(&self) -> i64 {
        let mut b = 0;
        if self.control != Control::None {
            b |= 1;
        }
        if self.control == Control::One {
            b |= 2;
        }
        if self.auto {
            b |= 4;
        }
        if self.group.is_some() {
            b |= 8;
        }
        if self.combine_view {
            b |= 16;
        }
        if self.combine_layer {
            b |= 32;
        }
        b
    }
}

impl DocState {
    /// Record an undo step (`snap` holds the drawing as it was before the change) and clear redo.
    /// UNDO Control None records nothing and One keeps just this step; the stack holds at most
    /// [`MAX_UNDO`] steps.
    pub fn push_undo(&mut self, mut snap: Snapshot) {
        drop_later(std::mem::take(&mut self.redo));
        match self.undo_ctl.control {
            Control::None => {
                drop_later(std::mem::take(&mut self.undo));
                return;
            }
            Control::One => {
                drop_later(std::mem::take(&mut self.undo));
                self.undo.push(snap);
                return;
            }
            Control::All => {}
        }
        // What keeping this step costs: the objects of the old drawing the new one doesn't share.
        snap.bytes = snap.doc.unique_bytes(&self.doc);
        // Marks above the current depth were undone; a new step makes them unreachable for good.
        let depth = self.undo.len();
        self.undo_ctl.marks.retain(|m| *m <= depth);
        if let Some(g) = self.undo_ctl.group
            && g > depth
        {
            self.undo_ctl.group = Some(depth);
        }
        self.undo.push(snap);
        let mut extra = self.undo.len().saturating_sub(MAX_UNDO);
        // The memory budget: drop the oldest steps until the rest fit, keeping the newest.
        let mut total: usize = self.undo.iter().map(|s| s.bytes).fold(0, usize::saturating_add);
        while total > self.undo_ctl.budget && extra + 1 < self.undo.len() {
            total = total.saturating_sub(self.undo.get(extra).map_or(0, |s| s.bytes));
            extra += 1;
        }
        if extra > 0 {
            drop_later(self.undo.drain(..extra).collect());
            for m in &mut self.undo_ctl.marks {
                *m = m.saturating_sub(extra);
            }
            if let Some(g) = &mut self.undo_ctl.group {
                *g = g.saturating_sub(extra);
            }
        }
    }

    /// UNDO Mark at the current depth. Returns the number of marks.
    pub fn undo_mark(&mut self) -> usize {
        let depth = self.undo.len();
        let marks = &mut self.undo_ctl.marks;
        marks.retain(|m| *m <= depth);
        marks.push(depth);
        if marks.len() > MAX_MARKS {
            marks.remove(0);
        }
        marks.len()
    }

    /// The marks still reachable from the current depth.
    pub fn undo_marks(&self) -> usize {
        let depth = self.undo.len();
        self.undo_ctl.marks.iter().filter(|m| **m <= depth).count()
    }

    /// UNDO BEgin: the steps from here to UNDO End become one. BEgin while a group is open ends that
    /// group first (returns true then).
    pub fn begin_undo_group(&mut self) -> bool {
        let ended = self.end_undo_group();
        self.undo_ctl.group = Some(self.undo.len());
        ended
    }

    /// UNDO End (also on save): collapse the steps since BEgin into one undo step. Returns whether
    /// a group was open; End with no open group does nothing.
    pub fn end_undo_group(&mut self) -> bool {
        let Some(start) = self.undo_ctl.group.take() else { return false };
        // The first step of the group holds the drawing as it was at BEgin.
        if self.undo.len() > start {
            self.undo.truncate(start + 1);
            if let Some(first) = self.undo.get_mut(start) {
                first.label = "Group".into();
            }
        }
        // States inside the group no longer exist.
        self.undo_ctl.marks.retain(|m| *m <= start);
        true
    }

    /// UNDO Control: All, None (drops all undo information) or One (keeps only the last step).
    /// Any open group and all marks are dropped when leaving All.
    pub fn set_undo_control(&mut self, c: Control) {
        self.undo_ctl.control = c;
        if c == Control::All {
            return;
        }
        self.undo_ctl.group = None;
        self.undo_ctl.marks.clear();
        self.redo.clear();
        let keep = if c == Control::One { 1 } else { 0 };
        let drop = self.undo.len().saturating_sub(keep);
        self.undo.drain(..drop);
    }
}
