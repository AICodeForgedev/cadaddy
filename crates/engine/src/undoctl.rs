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
/// of a large drawing took half a second (one long-lived thread frees them instead; the web
/// build has no threads and frees them here). One thread, not one per drop: a fresh thread per
/// step spread the frees over many malloc arenas, and on many-core hosts the process grew without
/// bound. When the queue is full, or the thread can't start, the steps are freed inline.
fn drop_later(steps: Vec<Snapshot>) {
    if steps.is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::mpsc::TrySendError;
        match free_queue() {
            Some(tx) => {
                if let Err(TrySendError::Full(v) | TrySendError::Disconnected(v)) = tx.try_send(steps) {
                    drop(v);
                }
            }
            None => drop(steps),
        }
    }
    #[cfg(target_arch = "wasm32")]
    drop(steps);
}

/// Most batches waiting for the free thread; past it a drop happens inline.
#[cfg(not(target_arch = "wasm32"))]
const FREE_QUEUE: usize = 64;

/// The sender of the one free thread, started on first use (`None` if it couldn't start).
#[cfg(not(target_arch = "wasm32"))]
fn free_queue() -> Option<&'static std::sync::mpsc::SyncSender<Vec<Snapshot>>> {
    use std::sync::{OnceLock, mpsc};
    static QUEUE: OnceLock<Option<mpsc::SyncSender<Vec<Snapshot>>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Vec<Snapshot>>(FREE_QUEUE);
            std::thread::Builder::new()
                .name("undo-free".into())
                .spawn(move || {
                    while let Ok(steps) = rx.recv() {
                        drop(steps);
                    }
                })
                .ok()
                .map(|_| tx)
        })
        .as_ref()
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
        // The activity log records every step, whatever UNDO Control keeps.
        self.activity.record(&snap.label, &snap.doc, &self.doc);
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
        // A long UNDO group (an agent's batch of thousands of commands) never loses its first step,
        // the drawing as it was at BEgin: past the cap its oldest later step goes instead.
        if let Some(g) = self.undo_ctl.group {
            while self.undo.len() > MAX_UNDO && self.undo.len() > g + 2 {
                drop_later(vec![self.undo.remove(g + 1)]);
            }
        }
        let mut extra = self.undo.len().saturating_sub(MAX_UNDO);
        // Steps before an open group's first may go; that one stays.
        let keep_from = self.undo_ctl.group.unwrap_or(usize::MAX);
        extra = extra.min(keep_from);
        // The memory budget: drop the oldest steps until the rest fit, keeping the newest.
        let mut total: usize = self.undo.iter().map(|s| s.bytes).fold(0, usize::saturating_add);
        while total > self.undo_ctl.budget && extra + 1 < self.undo.len() && extra < keep_from {
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn free_threads() -> usize {
        let Ok(dir) = std::fs::read_dir("/proc/self/task") else { return 0 };
        dir.flatten().filter(|t| std::fs::read_to_string(t.path().join("comm")).is_ok_and(|n| n.trim() == "undo-free")).count()
    }

    #[test]
    fn many_undo_drops_use_one_free_thread_and_free_everything() {
        let doc = Arc::new(cadaddy_doc::Drawing::default());
        for i in 0..2000 {
            let step = Snapshot { label: format!("step {i}"), doc: Arc::clone(&doc), selection: Vec::new(), bytes: 0, look: None };
            drop_later(vec![step]);
        }
        assert!(free_threads() <= 1, "{} undo-free threads", free_threads());
        // Every dropped step is released: queued ones by the thread, overflow ones inline.
        let mut left = Arc::strong_count(&doc);
        for _ in 0..500 {
            if left == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            left = Arc::strong_count(&doc);
        }
        assert_eq!(left, 1, "dropped steps still held");
    }
}
