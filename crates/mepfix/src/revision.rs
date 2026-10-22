//! Made-up revisions of a fixture set — a bulletin or ASI reissuing some sheets — with the changes
//! known, for tests of print-set comparison (Q270): the set as issued, the set as revised and
//! every change, with where it shows on the revised sheet.
//!
//! A revision resizes tagged rectangular ducts (their double lines and the size tags), raises
//! height-noted ducts by a foot (the notes) and takes out supply diffusers (from the mechanical
//! plan and the reflected ceiling plan). The sheets it changes
//! are re-plotted with their drawing shifted (`shift_in`), as a reissued sheet often is, so a
//! comparison has to align them first; the sheets it doesn't change are reissued byte for byte.

use crate::layout::{self, Kind};
use crate::rng::Rng;
use crate::style::Style;
use crate::truth::{Segment, Truth};
use crate::{FixtureSet, designed_set, draw_sheets};
use cadaddy_geom::{Mat3, Vec2};
use serde::{Deserialize, Serialize};

/// Changes of each kind, at most (they come from the set's own runs and terminals).
const MAX_EACH: usize = 50;

/// What a revision changes, and how.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevisionPlan {
    /// The revision's label (`ASI-01`, `BULLETIN 2`).
    pub label: String,
    /// Tagged rectangular ducts made 4" wider.
    pub resize: usize,
    /// Height-noted ducts raised by a foot.
    pub raise: usize,
    /// Supply diffusers taken out.
    pub remove_terminals: usize,
    /// How far the changed sheets' drawings move when they're re-plotted, model inches.
    pub shift_in: [f64; 2],
    /// Which of the candidates the changes go to.
    pub seed: u64,
}

impl Default for RevisionPlan {
    fn default() -> Self {
        RevisionPlan { label: "ASI-01".into(), resize: 2, raise: 1, remove_terminals: 1, shift_in: [36.0, -24.0], seed: 1 }
    }
}

/// One change a revision made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KnownChange {
    /// The sheet it shows on (`M-101`).
    pub sheet: String,
    /// `resize`, `raise` or `removed_terminal`.
    pub kind: String,
    /// The segment or terminal id.
    pub item: String,
    /// As issued and as revised (`24x12` → `28x12`; the height notes; the terminal's type tag →
    /// empty).
    pub from: String,
    pub to: String,
    /// Where the change shows on the revised sheet, model inches (the tag, the terminal).
    pub at_in: [f64; 2],
    /// The same place on the issued sheet.
    pub was_at_in: [f64; 2],
}

/// A set as issued and as revised.
pub struct Revision {
    pub issued: FixtureSet,
    pub revised: FixtureSet,
    pub plan: RevisionPlan,
    pub changes: Vec<KnownChange>,
}

impl Revision {
    /// The sheets the revision changes, by number.
    pub fn changed_sheets(&self) -> Vec<String> {
        let mut v: Vec<String> = self.changes.iter().map(|c| c.sheet.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

/// `n` of `items`, spread out, the same for a seed.
fn pick<T: Clone>(items: &[T], n: usize, r: &mut Rng) -> Vec<T> {
    let n = n.min(items.len()).min(MAX_EACH);
    if n == 0 {
        return Vec::new();
    }
    let step = items.len() / n;
    let off = if step > 1 { r.below(step) } else { 0 };
    (0..n).filter_map(|i| items.get(i * step + off).cloned()).collect()
}

fn tagged(s: &Segment) -> bool {
    !s.vertical && s.kind == "duct" && s.shape == "rect" && s.tag.as_ref().is_some_and(|t| !t.text.is_empty())
}

/// The sheets that show a level's terminals: its mechanical plan and its reflected ceiling plan.
fn terminal_sheets(t: &Truth, kinds: &[(String, Kind)], level: &str) -> Vec<String> {
    t.sheets
        .iter()
        .filter(|s| s.level.as_deref() == Some(level))
        .filter(|s| kinds.iter().any(|(n, k)| *n == s.number && matches!(k, Kind::Mech(l) | Kind::Rcp(l) if l == level)))
        .map(|s| s.number.clone())
        .collect()
}

/// Apply the plan to the truth; the changes, without their places yet.
fn apply(t: &mut Truth, st: &Style, kinds: &[(String, Kind)], plan: &RevisionPlan) -> Vec<KnownChange> {
    let mut r = Rng::new(plan.seed ^ 0x5245_5649_5349_4F4E);
    let mut changes = Vec::new();
    let resizable: Vec<String> = t.segments.iter().filter(|s| tagged(s)).map(|s| s.id.clone()).collect();
    let resized = pick(&resizable, plan.resize, &mut r.fork(1));
    for id in &resized {
        let Some(s) = t.segments.iter_mut().find(|s| &s.id == id) else { continue };
        let Some(tag) = s.tag.as_mut() else { continue };
        let from = st.rect_size(s.size_in[0], s.size_in[1]);
        s.size_in[0] += 4.0;
        let to = st.rect_size(s.size_in[0], s.size_in[1]);
        if let Some(line) = tag.text.first_mut() {
            *line = line.replacen(&from, &to, 1);
        }
        changes.push(KnownChange { sheet: s.sheet.clone(), kind: "resize".into(), item: id.clone(), from, to, ..KnownChange::default() });
    }
    let raisable: Vec<String> = t
        .segments
        .iter()
        .filter(|s| tagged(s) && !resized.contains(&s.id))
        .filter(|s| s.height.as_ref().is_some_and(|h| h.note.is_some()) && s.tag.as_ref().is_some_and(|t| t.text.len() >= 2))
        .map(|s| s.id.clone())
        .collect();
    for id in pick(&raisable, plan.raise, &mut r.fork(2)) {
        let Some(s) = t.segments.iter_mut().find(|s| s.id == id) else { continue };
        let (Some(h), Some(tag)) = (s.height.as_mut(), s.tag.as_mut()) else { continue };
        let Some(from) = h.note.clone() else { continue };
        h.bottom_ft += 1.0;
        h.top_ft += 1.0;
        h.centre_ft += 1.0;
        let to = st.height_note(h.bottom_ft, h.top_ft, h.centre_ft);
        h.note = Some(to.clone());
        if let Some(line) = tag.text.iter_mut().find(|l| **l == from) {
            *line = to.clone();
        }
        changes.push(KnownChange { sheet: s.sheet.clone(), kind: "raise".into(), item: id.clone(), from, to, ..KnownChange::default() });
    }
    let removable: Vec<(String, Vec<String>)> = t
        .terminals
        .iter()
        .filter(|x| x.kind == "supply_diffuser")
        .map(|x| (x.id.clone(), terminal_sheets(t, kinds, &x.level)))
        .filter(|(_, sheets)| !sheets.is_empty())
        .collect();
    for (id, sheets) in pick(&removable, plan.remove_terminals, &mut r.fork(3)) {
        let Some(i) = t.terminals.iter().position(|x| x.id == id) else { continue };
        let x = t.terminals.remove(i);
        for sheet in sheets {
            changes.push(KnownChange {
                sheet,
                kind: "removed_terminal".into(),
                item: id.clone(),
                from: x.type_tag.clone(),
                to: String::new(),
                ..KnownChange::default()
            });
        }
    }
    changes
}

/// Where a change shows on a sheet, model inches (before any shift).
fn place(t: &Truth, c: &KnownChange, issued: &Truth) -> Option<[f64; 2]> {
    let sh = t.sheets.iter().find(|s| s.number == c.sheet)?;
    let ft = match c.kind.as_str() {
        "removed_terminal" => issued.terminals.iter().find(|x| x.id == c.item).map(|x| [x.at_ft[0], x.at_ft[1]])?,
        _ => t.segment(&c.item)?.tag.as_ref()?.at_ft,
    };
    Some(layout::to_model(sh, ft))
}

/// The office set for `seed` as issued and as revised by `plan`.
pub fn revise(seed: u64, plan: &RevisionPlan) -> Result<Revision, String> {
    let shift = plan.shift_in;
    if !shift.iter().all(|v| v.is_finite() && v.abs() <= 1e6) {
        return Err("the shift must be finite and at most a million inches".into());
    }
    let (mut issued_t, st, kinds) = designed_set(seed, false);
    let mut revised_t = issued_t.clone();
    let mut changes = apply(&mut revised_t, &st, &kinds, plan);
    for c in &mut changes {
        let at = place(&revised_t, c, &issued_t).unwrap_or_default();
        c.was_at_in = at;
        c.at_in = [at[0] + shift[0], at[1] + shift[1]];
    }
    let changed: Vec<String> = changes.iter().map(|c| c.sheet.clone()).collect();
    let issued_files = draw_sheets(&mut issued_t, &st, &kinds, &mut |_, d| d)?;
    let m = Mat3::translate(Vec2::new(shift[0], shift[1]));
    let mut shifted = |number: &str, mut d: cadaddy_doc::Drawing| {
        if changed.iter().any(|c| c == number) {
            for h in d.model.handles() {
                d.model.modify(h, |e| e.kind.transform(&m));
            }
        }
        d
    };
    let revised_files = draw_sheets(&mut revised_t, &st, &kinds, &mut shifted)?;
    Ok(Revision {
        issued: FixtureSet { truth: issued_t, files: issued_files },
        revised: FixtureSet { truth: revised_t, files: revised_files },
        plan: plan.clone(),
        changes,
    })
}
