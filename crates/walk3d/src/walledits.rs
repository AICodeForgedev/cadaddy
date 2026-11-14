//! Wall edits (Q6613): a built wall moved, resized or deleted, or a wall added where the plan
//! has no line pair, kept in the spec's [`BuildingSpec::wall_edits`] and honoured by every build.
//!
//! The plan's line pairs give every wall's geometry; the spec's wall list ([`BuildingSpec::walls`])
//! only types the wall built on each centreline. A wall the reader built in the wrong place stays
//! there when its entry alone changes, so a change to an entry's place is kept as an edit beside
//! the list ([`BuildingSpec::record_wall_list_changes`]): an entry moved moves the wall built on
//! it, one removed removes it, one added builds a wall of its type. `wall.edit` keeps the same
//! edits for a built wall picked in the model. A re-read of the plans rewrites the list; the
//! edits stay, in the order made, and the build applies each to the walls its plan gives (and
//! those the edits before it made).

use serde::{Deserialize, Serialize};

use crate::spec::{BuildingSpec, WallMark};

/// Most edits a spec keeps.
pub const MAX_EDITS: usize = 10_000;
/// Most characters of a reason kept.
const MAX_TEXT: usize = 200;

/// One wall edit. Coordinates are the level's plan (sheet units), as the wall list's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallEdit {
    /// `WE-1`, in the order the edits were made.
    pub id: String,
    /// `move` (a resize too: new ends), `delete` or `add`.
    pub op: String,
    pub level: String,
    /// The built wall it changes (`move`, `delete`): its centreline as built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<[[f64; 2]; 2]>,
    /// Where the wall stands after it (`move`, `add`): its centreline, built to these ends.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<[[f64; 2]; 2]>,
    /// The wall's type ([`crate::WallType::name`]); empty = the wall's own (a moved wall's), else
    /// the type of the wall list's entry on it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub wall_type: String,
    /// Thickness (inches) where no type gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness_in: Option<f64>,
    /// Height above the floor (feet); none = the type's, else the wall's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
    /// How it was made: `wall list` (an entry of the spec's wall list changed) or `wall.edit`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub via: String,
    /// Why, and who: `user` or `agent`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub by: String,
}

impl WallEdit {
    /// The basis a wall it builds carries (its provenance: not drawn on the plan there).
    pub fn basis(&self) -> String {
        let what = match self.op.as_str() {
            "add" => "added",
            "delete" => "deleted",
            _ => "moved",
        };
        let by = if self.by.is_empty() { "user" } else { self.by.as_str() };
        let via = if self.via.is_empty() { "wall.edit" } else { self.via.as_str() };
        let mut s = format!("{what} by wall edit {} ({by}, {via})", self.id);
        if !self.reason.trim().is_empty() {
            s.push_str(": ");
            s.extend(self.reason.trim().chars().take(MAX_TEXT));
        }
        s
    }

    /// Whether its lines are numbers.
    pub fn finite(&self) -> bool {
        self.from.iter().chain(&self.to).all(|l| l.iter().flatten().all(|v| v.is_finite()))
    }
}

/// The id the next edit gets: one past the highest `WE-n`.
pub fn next_id(edits: &[WallEdit]) -> String {
    let n = edits.iter().filter_map(|e| e.id.strip_prefix("WE-").and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0);
    format!("WE-{}", n.saturating_add(1))
}

/// Whether two centrelines are the same (either way round), within `tol`.
pub fn same_line(a: [[f64; 2]; 2], b: [[f64; 2]; 2], tol: f64) -> bool {
    let near = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).abs() <= tol && (p[1] - q[1]).abs() <= tol;
    (near(a[0], b[0]) && near(a[1], b[1])) || (near(a[0], b[1]) && near(a[1], b[0]))
}

fn line(m: &WallMark) -> [[f64; 2]; 2] {
    [m.a, m.b]
}

/// What a change to the wall list did to the edits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListChange {
    /// Edits made or changed (their ids).
    pub made: Vec<String>,
    /// Edits taken back (an entry put back where it was, an added one removed).
    pub dropped: Vec<String>,
    /// Changes not kept: the spec holds [`MAX_EDITS`] edits.
    pub refused: usize,
}

impl ListChange {
    pub fn is_empty(&self) -> bool {
        self.made.is_empty() && self.dropped.is_empty() && self.refused == 0
    }
}

impl BuildingSpec {
    /// Keep the change from the wall list `old` to [`Self::walls`] as wall edits: an entry whose
    /// centreline changed (the same place in the list, the same level) moves the wall built on
    /// it; one removed deletes it; one added builds a wall of its type. A type or height changed
    /// on the same centreline is no edit (the list types the wall). An entry put back where an
    /// edit took its wall from takes that edit back.
    pub fn record_wall_list_changes(&mut self, old: &[WallMark]) -> ListChange {
        let upf = if self.units_per_foot.is_finite() && self.units_per_foot > 0.0 { self.units_per_foot } else { 12.0 };
        let tol = 1e-6 * upf;
        let new: Vec<WallMark> = self.walls.clone();
        let same_level = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        let finite = |m: &WallMark| m.a.iter().chain(&m.b).all(|v| v.is_finite());
        let mut old_used = vec![false; old.len()];
        let mut new_used = vec![false; new.len()];
        // Unchanged: the same centreline on the same level, anywhere in the list.
        for (i, n) in new.iter().enumerate().take(crate::spec::MAX_ITEMS) {
            let hit = old
                .iter()
                .enumerate()
                .take(crate::spec::MAX_ITEMS)
                .find(|(j, o)| !old_used.get(*j).copied().unwrap_or(true) && same_level(&o.level, &n.level) && same_line(line(o), line(n), tol));
            if let Some((j, _)) = hit {
                if let Some(x) = old_used.get_mut(j) {
                    *x = true;
                }
                if let Some(x) = new_used.get_mut(i) {
                    *x = true;
                }
            }
        }
        let mut out = ListChange::default();
        // Moved: the entry at the same place in the list, on the same level, with new ends.
        for (i, (o, n)) in old.iter().zip(&new).enumerate().take(crate::spec::MAX_ITEMS) {
            if old_used.get(i).copied().unwrap_or(true) || new_used.get(i).copied().unwrap_or(true) || !same_level(&o.level, &n.level) {
                continue;
            }
            if !(finite(o) && finite(n)) {
                continue;
            }
            if let Some(x) = old_used.get_mut(i) {
                *x = true;
            }
            if let Some(x) = new_used.get_mut(i) {
                *x = true;
            }
            self.list_moved(o, n, tol, &mut out);
        }
        for (o, _) in old.iter().zip(&old_used).filter(|(o, used)| !**used && finite(o)).take(crate::spec::MAX_ITEMS) {
            self.list_removed(o, tol, &mut out);
        }
        for (n, _) in new.iter().zip(&new_used).filter(|(n, used)| !**used && finite(n)).take(crate::spec::MAX_ITEMS) {
            self.list_added(n, tol, &mut out);
        }
        out
    }

    /// The edit that put a wall at `l` on `level` (an add or a move), if any: the latest.
    fn edit_to(&self, level: &str, l: [[f64; 2]; 2], tol: f64) -> Option<usize> {
        self.wall_edits.iter().rposition(|e| e.level.trim().eq_ignore_ascii_case(level.trim()) && e.to.is_some_and(|t| same_line(t, l, tol)))
    }

    fn push_edit(&mut self, mut e: WallEdit, out: &mut ListChange) {
        if self.wall_edits.len() >= MAX_EDITS {
            out.refused += 1;
            return;
        }
        e.id = next_id(&self.wall_edits);
        e.via = "wall list".into();
        e.by = "user".into();
        out.made.push(e.id.clone());
        self.wall_edits.push(e);
    }

    fn list_moved(&mut self, o: &WallMark, n: &WallMark, tol: f64, out: &mut ListChange) {
        if let Some(i) = self.edit_to(&o.level, line(o), tol) {
            let back = self.wall_edits.get(i).is_some_and(|e| e.op != "add" && e.from.is_some_and(|f| same_line(f, line(n), tol)));
            if back {
                // Put back where the plan built it: the move is taken back.
                let e = self.wall_edits.remove(i);
                out.dropped.push(e.id);
            } else if let Some(e) = self.wall_edits.get_mut(i) {
                e.to = Some(line(n));
                e.wall_type.clone_from(&n.wall_type);
                e.height_ft = n.height_ft;
                out.made.push(e.id.clone());
            }
            return;
        }
        let e = WallEdit {
            op: "move".into(),
            level: n.level.clone(),
            from: Some(line(o)),
            to: Some(line(n)),
            wall_type: n.wall_type.clone(),
            height_ft: n.height_ft,
            ..WallEdit::default()
        };
        self.push_edit(e, out);
    }

    fn list_removed(&mut self, o: &WallMark, tol: f64, out: &mut ListChange) {
        if let Some(i) = self.edit_to(&o.level, line(o), tol) {
            let added = self.wall_edits.get(i).is_some_and(|e| e.op == "add");
            if added {
                // The wall an edit added: the edit is taken back.
                let e = self.wall_edits.remove(i);
                out.dropped.push(e.id);
            } else if let Some(e) = self.wall_edits.get_mut(i) {
                // A moved wall removed: the plan's wall stays deleted.
                e.op = "delete".into();
                e.to = None;
                out.made.push(e.id.clone());
            }
            return;
        }
        let e = WallEdit { op: "delete".into(), level: o.level.clone(), from: Some(line(o)), wall_type: o.wall_type.clone(), ..WallEdit::default() };
        self.push_edit(e, out);
    }

    fn list_added(&mut self, n: &WallMark, tol: f64, out: &mut ListChange) {
        let back = self.wall_edits.iter().rposition(|e| {
            e.op == "delete" && e.level.trim().eq_ignore_ascii_case(n.level.trim()) && e.from.is_some_and(|f| same_line(f, line(n), tol))
        });
        if let Some(i) = back {
            // An entry put back where a deleted wall stood: the delete is taken back.
            let e = self.wall_edits.remove(i);
            out.dropped.push(e.id);
            return;
        }
        let e = WallEdit {
            op: "add".into(),
            level: n.level.clone(),
            to: Some(line(n)),
            wall_type: n.wall_type.clone(),
            height_ft: n.height_ft,
            ..WallEdit::default()
        };
        self.push_edit(e, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(level: &str, a: [f64; 2], b: [f64; 2], t: &str) -> WallMark {
        WallMark {
            level: level.into(),
            a,
            b,
            wall_type: t.into(),
            height_ft: None,
            guess: None,
            source: String::new(),
            status: None,
            height_source: String::new(),
        }
    }

    fn spec(walls: Vec<WallMark>) -> BuildingSpec {
        BuildingSpec { walls, ..BuildingSpec::default() }
    }

    #[test]
    fn a_type_change_on_the_same_centreline_is_no_edit() {
        let old = vec![mark("L1", [0.0, 0.0], [100.0, 0.0], "A"), mark("L1", [0.0, 0.0], [0.0, 50.0], "A")];
        // Retyped, turned round and reordered: the same walls.
        let mut sp = spec(vec![mark("L1", [0.0, 50.0], [0.0, 0.0], "B"), mark("L1", [0.0, 0.0], [100.0, 0.0], "C")]);
        assert!(sp.record_wall_list_changes(&old).is_empty());
        assert!(sp.wall_edits.is_empty());
    }

    #[test]
    fn moved_removed_and_added_entries_become_edits() {
        let old = vec![mark("L1", [0.0, 0.0], [100.0, 0.0], "A"), mark("L1", [0.0, 10.0], [100.0, 10.0], "A")];
        let mut sp = spec(vec![mark("L1", [0.0, 2.5], [100.0, 2.5], "M"), mark("L2", [5.0, 5.0], [5.0, 60.0], "M")]);
        let ch = sp.record_wall_list_changes(&old);
        assert_eq!(ch.made, ["WE-1", "WE-2", "WE-3"]);
        let ops: Vec<&str> = sp.wall_edits.iter().map(|e| e.op.as_str()).collect();
        assert_eq!(ops, ["move", "delete", "add"]);
        let mv = &sp.wall_edits[0];
        assert_eq!((mv.from, mv.to, mv.wall_type.as_str()), (Some([[0.0, 0.0], [100.0, 0.0]]), Some([[0.0, 2.5], [100.0, 2.5]]), "M"));
        assert_eq!(sp.wall_edits[1].from, Some([[0.0, 10.0], [100.0, 10.0]]));
        assert_eq!((sp.wall_edits[2].level.as_str(), sp.wall_edits[2].to), ("L2", Some([[5.0, 5.0], [5.0, 60.0]])));
        assert!(sp.wall_edits.iter().all(|e| e.via == "wall list" && e.by == "user"));
    }

    #[test]
    fn edits_follow_the_entry_and_are_taken_back_when_it_goes_back() {
        let first = vec![mark("L1", [0.0, 0.0], [100.0, 0.0], "A")];
        let mut sp = spec(vec![mark("L1", [0.0, 3.0], [100.0, 3.0], "A")]);
        sp.record_wall_list_changes(&first);
        // Moved again: the same edit, its new place.
        let second = sp.walls.clone();
        sp.walls = vec![mark("L1", [0.0, 4.0], [90.0, 4.0], "A")];
        sp.record_wall_list_changes(&second);
        assert_eq!(sp.wall_edits.len(), 1);
        assert_eq!(sp.wall_edits[0].to, Some([[0.0, 4.0], [90.0, 4.0]]));
        // Back where the plan built it: no edit.
        let third = sp.walls.clone();
        sp.walls = first.clone();
        let ch = sp.record_wall_list_changes(&third);
        assert_eq!(ch.dropped, ["WE-1"]);
        assert!(sp.wall_edits.is_empty());
        // Removed, then put back: the delete is taken back.
        sp.walls.clear();
        sp.record_wall_list_changes(&first);
        assert_eq!(sp.wall_edits.len(), 1);
        sp.walls = first.clone();
        sp.record_wall_list_changes(&[]);
        assert!(sp.wall_edits.is_empty());
        // An added entry removed again: nothing left.
        let added = vec![mark("L1", [0.0, 0.0], [100.0, 0.0], "A"), mark("L1", [50.0, 0.0], [50.0, 40.0], "A")];
        sp.walls = added.clone();
        sp.record_wall_list_changes(&first);
        assert_eq!(sp.wall_edits.len(), 1);
        sp.walls = first.clone();
        sp.record_wall_list_changes(&added);
        assert!(sp.wall_edits.is_empty());
        // A moved entry then removed: the plan's wall stays deleted.
        sp.walls = vec![mark("L1", [0.0, 5.0], [100.0, 5.0], "A")];
        sp.record_wall_list_changes(&first);
        let moved = sp.walls.clone();
        sp.walls.clear();
        sp.record_wall_list_changes(&moved);
        assert_eq!(sp.wall_edits.len(), 1);
        assert_eq!((sp.wall_edits[0].op.as_str(), sp.wall_edits[0].from, sp.wall_edits[0].to), ("delete", Some([[0.0, 0.0], [100.0, 0.0]]), None));
    }

    #[test]
    fn edits_survive_the_spec_round_trip_and_ids_count_on() {
        let mut sp = spec(vec![]);
        sp.record_wall_list_changes(&[mark("L1", [0.0, 0.0], [10.0, 0.0], "A")]);
        let back = BuildingSpec::from_json(&sp.to_json()).unwrap_or_default();
        assert_eq!(back.wall_edits, sp.wall_edits);
        assert_eq!(next_id(&back.wall_edits), "WE-2");
        assert!(back.wall_edits[0].basis().starts_with("deleted by wall edit WE-1 (user, wall list)"));
    }
}
