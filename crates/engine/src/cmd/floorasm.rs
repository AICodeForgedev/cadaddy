//! What each level's floor is built of (Q6408).
//!
//! A level's floor element is its documented assembly, not one slab thickness for the whole
//! model: a slab is as thick as its slab notes (or the spec's concrete slab) say; a framed floor
//! is its topping and deck, the joists or floor trusses under it being their own members (the
//! trade pieces lay them under the deck). Read, first to last:
//! - the notes on the level's plan (inside its region) and those the job read off the level's
//!   sheets ([`BuildingSpec::floor_notes`]): `4" THICK SLAB ON GRADE`; `3/4" GYPSUM CONCRETE ON
//!   3/4" SHEATHING ON 11 7/8" I-JOISTS`, a layered note's layers top to bottom;
//! - the floor framing read for the level ([`BuildingSpec::floor_framing`] and its note): a level
//!   framed by joists or trusses is a framed floor whatever slab a note on its plan names;
//! - the spec's concrete slab for the level;
//! - the trades' documents' facts: a slab's thickness for the slab on grade (the lowest storey),
//!   an underlayment for a framed floor;
//! - a framed floor's deck none of them gives: the trade rules' subfloor (derived, marked);
//! - nothing: the build's default slab, marked a default.
//!
//! The words and limits are rule data ([`RULE`] in `tradepractice.json`); nothing here is one
//! job's.

use std::sync::Arc;

use cadaddy_doc::props::{Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, Level};
use serde::Deserialize;
use serde_json::{Value, json};

/// The rule a floor's assembly is read and filled by.
pub(crate) const RULE: &str = "A-FLOOR-ASSEMBLY";
/// Longest note read, most layers of one note, most notes kept per level.
const MAX_TEXT: usize = 400;
const MAX_LAYERS: usize = 8;
const MAX_LEVEL_NOTES: usize = 64;
/// Longest text kept in an element's tag.
const MAX_TAG: usize = 200;

/// The rule's words and limits.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Words {
    slab_marks: Vec<String>,
    slab_lead: Vec<String>,
    separators: Vec<String>,
    topping_words: Vec<String>,
    deck_words: Vec<String>,
    member_words: Vec<String>,
    skip_words: Vec<String>,
    slab_in: Vec<f64>,
    topping_in: Vec<f64>,
    deck_in: Vec<f64>,
    member_in: Vec<f64>,
    max_notes: usize,
}

fn words() -> Words {
    serde_json::from_value(super::provenance::rule_data(RULE)).unwrap_or_default()
}

/// Whether `v` is inside the rule's `[least, most]` inches.
fn within(range: &[f64], v: f64) -> bool {
    match (range.first(), range.get(1)) {
        (Some(lo), Some(hi)) => v.is_finite() && v >= *lo && v <= *hi,
        _ => false,
    }
}

/// How a value of a floor's assembly is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Status {
    /// Read from the documents (a note, the spec's concrete, a submittal's fact).
    Documented,
    /// Filled in by a trade rule (the usual deck under a topping).
    Derived,
    /// The build's default slab: nothing gives the floor.
    Default,
}

impl Status {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Status::Documented => "documented",
            Status::Derived => "derived",
            Status::Default => "default",
        }
    }
}

/// What a layer of the floor element is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Slab,
    Topping,
    Deck,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Slab => "slab",
            Kind::Topping => "topping",
            Kind::Deck => "deck",
        }
    }
}

/// One layer of a floor element, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    pub kind: Kind,
    /// What it is, as read (`gypsum concrete`, `sheathing`, `slab on grade`).
    pub name: String,
    pub inches: f64,
    pub status: Status,
    /// The note, fact or rule it is from.
    pub source: String,
}

/// A value a note gives: inches, what it names, the note.
type Said = (f64, String, String);

/// What notes say about a floor (the first of each kind wins).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NoteRead {
    pub slab: Option<Said>,
    pub topping: Option<Said>,
    pub deck: Option<Said>,
    /// The joists or trusses a layered note names under the deck.
    pub member: Option<Said>,
}

impl NoteRead {
    /// Whether it says what the floor is built of (a slab, a topping or a deck).
    pub(crate) fn is_assembly(&self) -> bool {
        self.slab.is_some() || self.topping.is_some() || self.deck.is_some()
    }

    fn framed(&self) -> bool {
        self.topping.is_some() || self.deck.is_some() || self.member.is_some()
    }

    /// Fill what this one leaves out from `other`.
    fn add(&mut self, other: NoteRead) {
        self.slab = self.slab.take().or(other.slab);
        self.topping = self.topping.take().or(other.topping);
        self.deck = self.deck.take().or(other.deck);
        self.member = self.member.take().or(other.member);
    }
}

/// A whole number, a fraction or both (`4`, `3/4`, `11 7/8`, `1-1/2`, `0.75`), inches.
fn inches_of(head: &str) -> Option<f64> {
    let toks: Vec<&str> = head.split([' ', '-']).filter(|t| !t.is_empty()).collect();
    let num = |t: &str| -> Option<f64> {
        match t.split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                (b > 0.0 && a >= 0.0 && a < b).then_some(a / b)
            }
            None => t.parse::<f64>().ok(),
        }
    };
    let v = match toks.as_slice() {
        [one] => num(one)?,
        [whole, frac] if !whole.contains(['/', '.']) && frac.contains('/') => whole.parse::<f64>().ok()? + num(frac)?,
        _ => return None,
    };
    (v.is_finite() && v > 0.0).then_some(v)
}

/// The inches written at the end of `s` (`TYP. 4` → 4; `11 7/8` → 11.875).
fn tail_inches(s: &str) -> Option<f64> {
    let keep = |c: char| c.is_ascii_digit() || matches!(c, '/' | ' ' | '-' | '.');
    let start = s.char_indices().rev().take_while(|(_, c)| keep(*c)).last().map_or(s.len(), |(i, _)| i);
    let tail = s.get(start..)?.trim_start_matches(|c: char| !c.is_ascii_digit()).trim();
    inches_of(tail).or_else(|| tail.rsplit([' ', '-']).next().and_then(inches_of))
}

/// A layer as written: its thickness first (`3/4" SHEATHING`), then what it is.
fn lead_inches(part: &str) -> Option<(f64, &str)> {
    let q = part.find('"')?;
    let head = part.get(..q)?.trim();
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_digit() || matches!(c, '/' | ' ' | '-' | '.')) {
        return None;
    }
    Some((inches_of(head)?, part.get(q + 1..)?.trim()))
}

fn has(u: &str, words: &[String]) -> bool {
    words.iter().any(|w| !w.is_empty() && u.contains(&w.to_ascii_uppercase()))
}

/// What a layer is called: its words, in lower case, to the first comma or full stop.
fn name_of(material: &str) -> String {
    let m = material.split([',', ';', '(']).next().unwrap_or_default().trim().trim_end_matches(['.', ':']).trim();
    m.to_ascii_lowercase().chars().take(48).collect()
}

/// What a note says a floor is built of.
#[cfg(test)]
pub(crate) fn read_note(text: &str) -> NoteRead {
    read_with(&words(), text)
}

fn read_with(w: &Words, text: &str) -> NoteRead {
    let raw: String = text.chars().take(MAX_TEXT).map(|c| if matches!(c, '\u{201d}' | '\u{2033}' | '\u{201c}') { '"' } else { c }).collect();
    let u = raw.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let note = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = NoteRead::default();
    if !u.contains('"') {
        return out;
    }
    // A layered note: its layers top to bottom between the separators.
    let mut parts: Vec<String> = vec![u.clone()];
    for sep in w.separators.iter().filter(|s| !s.is_empty()).take(8) {
        let sep = sep.to_ascii_uppercase();
        parts = parts.iter().flat_map(|p| p.split(sep.as_str()).map(str::to_string).collect::<Vec<_>>()).take(MAX_LAYERS * 2).collect();
    }
    for part in parts.iter().take(MAX_LAYERS) {
        let Some((v, material)) = lead_inches(part.trim()) else { continue };
        let said = |v: f64| (v, name_of(material), note.clone());
        if has(material, &w.member_words) {
            if out.member.is_none() && within(&w.member_in, v) {
                out.member = Some(said(v));
            }
            continue;
        }
        if has(part, &w.skip_words) {
            continue;
        }
        if has(material, &w.topping_words) {
            if out.topping.is_none() && within(&w.topping_in, v) {
                out.topping = Some(said(v));
            }
        } else if has(material, &w.deck_words) && out.deck.is_none() && within(&w.deck_in, v) {
            out.deck = Some(said(v));
        }
    }
    // A slab: `4" THICK SLAB`, `SLAB THICKNESS = 5"` (not a framed floor's note).
    if !out.framed() && !has(&u, &w.member_words) {
        let mut slab = None;
        for mark in w.slab_marks.iter().filter(|m| !m.is_empty()) {
            if let Some(i) = u.find(&mark.to_ascii_uppercase())
                && let Some(v) = u.get(..i).and_then(tail_inches).filter(|v| within(&w.slab_in, *v))
            {
                slab = Some(v);
                break;
            }
        }
        if slab.is_none() {
            for lead in w.slab_lead.iter().filter(|m| !m.is_empty()) {
                let Some(i) = u.find(&lead.to_ascii_uppercase()) else { continue };
                let rest = u.get(i + lead.len()..).unwrap_or_default();
                let Some(q) = rest.find('"') else { continue };
                let head = rest.get(..q).unwrap_or_default().trim_start_matches(|c: char| !c.is_ascii_digit()).trim();
                if let Some(v) = inches_of(head).filter(|v| within(&w.slab_in, *v)) {
                    slab = Some(v);
                    break;
                }
            }
        }
        if let Some(v) = slab {
            let pt = ["POST-TENSIONED", "POST TENSIONED", "PT SLAB", "P.T."].iter().any(|k| u.contains(k));
            let grade = u.contains("GRADE") || u.contains("S.O.G");
            let name = match (pt, grade) {
                (true, true) => "post-tensioned slab on grade",
                (true, false) => "post-tensioned slab",
                (false, true) => "slab on grade",
                (false, false) => "concrete slab",
            };
            out.slab = Some((v, name.into(), note.clone()));
        }
    }
    out
}

/// A floor's assembly: its element's layers (top to bottom) and what is under them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Assembly {
    pub level: String,
    /// A framed floor (joists or trusses under its deck), else a slab.
    pub framed: bool,
    pub layers: Vec<Layer>,
    /// The members under a framed floor's deck (their depth, what they are, from where): their
    /// own pieces, never part of the element.
    pub member: Option<Said>,
    /// What was read and not used (a slab note on a framed floor).
    pub notes: Vec<String>,
}

impl Assembly {
    /// The element's thickness, inches.
    pub(crate) fn thickness_in(&self) -> f64 {
        self.layers.iter().map(|l| l.inches).sum::<f64>().max(0.0)
    }

    /// How its thickness is known: its least known layer's.
    pub(crate) fn status(&self) -> Status {
        self.layers.iter().map(|l| l.status).max().unwrap_or(Status::Default)
    }

    fn inches(&self, kind: Kind) -> Option<f64> {
        self.layers.iter().find(|l| l.kind == kind).map(|l| l.inches)
    }

    pub(crate) fn topping_in(&self) -> Option<f64> {
        self.inches(Kind::Topping)
    }

    pub(crate) fn deck_in(&self) -> Option<f64> {
        self.inches(Kind::Deck)
    }

    /// In words: `3/4" gypsum concrete on 3/4" sheathing on 11 7/8" i-joists (below)`.
    pub(crate) fn describe(&self) -> String {
        let mut parts: Vec<String> = self.layers.iter().map(|l| format!("{} {}", cadaddy_walk3d::inches(l.inches), l.name)).collect();
        if let Some((d, what, _)) = &self.member {
            parts.push(format!("{} {} (their own members)", cadaddy_walk3d::inches(*d), what));
        }
        if parts.is_empty() {
            return "no floor slab".into();
        }
        parts.join(" on ")
    }

    /// Where each layer is from.
    pub(crate) fn basis(&self) -> String {
        let mut seen: Vec<String> = Vec::new();
        for l in &self.layers {
            let s = format!("{}: {}", l.kind.word(), l.source);
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
        seen.join("; ")
    }

    /// The element's build tags: `floor:framed|slab`, `thick_in:`, `thick_from:`, `topping_in:`,
    /// `deck_in:`, `assembly:` and `thick_basis:`.
    pub(crate) fn tags(&self) -> Vec<String> {
        let cut = |s: String| s.replace(':', " -").chars().take(MAX_TAG).collect::<String>();
        let mut t = vec![
            format!("floor:{}", if self.framed { "framed" } else { "slab" }),
            format!("thick_in:{}", round3(self.thickness_in())),
            format!("thick_from:{}", self.status().word()),
        ];
        if let Some(v) = self.topping_in() {
            t.push(format!("topping_in:{}", round3(v)));
        }
        if let Some(v) = self.deck_in() {
            t.push(format!("deck_in:{}", round3(v)));
        }
        if let Some((d, what, _)) = &self.member {
            t.push(format!("member_in:{}", round3(*d)));
            t.push(format!("member:{}", if what.to_ascii_uppercase().contains("TRUSS") { "truss" } else { "joist" }));
        }
        t.push(format!("assembly:{}", cut(self.describe())));
        t.push(format!("thick_basis:{}", cut(self.basis())));
        t
    }

    pub(crate) fn to_json(&self) -> Value {
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|l| json!({"kind": l.kind.word(), "name": l.name, "inches": round3(l.inches), "status": l.status.word(), "source": l.source}))
            .collect();
        let mut v = json!({
            "level": self.level, "kind": if self.framed { "framed" } else { "slab" }, "thickness_in": round3(self.thickness_in()),
            "status": self.status().word(), "assembly": self.describe(), "layers": layers,
        });
        if let Some((d, what, src)) = &self.member {
            v["members"] = json!({"depth_in": round3(*d), "what": what, "source": src});
        }
        if !self.notes.is_empty() {
            v["notes"] = json!(self.notes);
        }
        v
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// A floor element's build-up as its build tags keep it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Built {
    pub framed: bool,
    pub topping_in: Option<f64>,
    pub deck_in: Option<f64>,
    /// The members under the deck its notes name: depth, `joist` or `truss`.
    pub member: Option<(f64, String)>,
    /// Where its build-up is from.
    pub basis: String,
}

/// A floor element's build-up as its build tags keep it.
pub(crate) fn of_element(e: &Entity) -> Option<Built> {
    let kind = super::bimlook::made_tag(e, "floor")?;
    let num = |k: &str| super::bimlook::made_tag(e, k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0);
    let member = num("member_in").filter(|v| *v > 0.0).map(|d| (d, super::bimlook::made_tag(e, "member").unwrap_or("joist").to_string()));
    Some(Built {
        framed: kind.eq_ignore_ascii_case("framed"),
        topping_in: num("topping_in"),
        deck_in: num("deck_in"),
        member,
        basis: super::bimlook::made_tag(e, "thick_basis").unwrap_or_default().to_string(),
    })
}

/// A floor element's properties from its build tags: `type` (`slab`, `framed floor`),
/// `assembly` and `thickness` (inches), each sourced as the build knew it.
pub(crate) fn props_of(e: &Entity) -> Vec<(&'static str, cadaddy_doc::props::Prop)> {
    use cadaddy_doc::props::Prop;
    let tag = |k: &str| super::bimlook::made_tag(e, k).filter(|v| !v.is_empty());
    let Some(kind) = tag("floor") else { return Vec::new() };
    let status = match tag("thick_from") {
        Some("documented") => Status::Documented,
        Some("derived") => Status::Derived,
        _ => Status::Default,
    };
    let src = source(status, tag("thick_basis").unwrap_or_default());
    let mut out = vec![("type", Prop::new(if kind.eq_ignore_ascii_case("framed") { "framed floor" } else { "slab" }, src.clone()))];
    if let Some(a) = tag("assembly") {
        out.push(("assembly", Prop::new(a, src.clone())));
    }
    if let Some(t) = tag("thick_in").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0) {
        out.push(("thickness", Prop::new(t, src).with_unit("in")));
    }
    out
}

/// A property source for how a value of the assembly is known.
pub(crate) fn source(status: Status, basis: &str) -> Source {
    let kind = match status {
        Status::Documented => SourceKind::Note,
        Status::Derived => SourceKind::Rule,
        Status::Default => SourceKind::Default,
    };
    Source::from_doc(kind, &basis.chars().take(MAX_TAG).collect::<String>())
}

/// The notes on the plans in the drawing that say what a floor is built of: (words, where).
pub(crate) fn plan_notes(d: &Drawing) -> Vec<(String, Vec2)> {
    let w = words();
    let max = w.max_notes.clamp(1, 10_000);
    let ents: Vec<&Arc<Entity>> = d
        .model
        .iter()
        .filter(|e| super::wallgen::made_as(e).is_none() && matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_)))
        .take(2_000_000)
        .collect();
    let mut out = Vec::new();
    for (_, t, at, _) in super::perceive::texts(&ents) {
        if out.len() >= max {
            break;
        }
        if t.contains('"') && read_with(&w, &t).is_assembly() {
            out.push((t, at));
        }
    }
    out
}

/// Whether sheet point `p` is in `level`'s plan region (`whole`: a level without one holds the
/// whole drawing, as one storey's does).
fn in_region(level: &Level, p: Vec2, whole: bool) -> bool {
    match level.region {
        Some([x0, y0, x1, y1]) => p.x >= x0.min(x1) && p.x <= x0.max(x1) && p.y >= y0.min(y1) && p.y <= y0.max(y1),
        None => whole,
    }
}

/// Everything a level's floor is resolved from beside its notes.
pub(crate) struct Context<'a> {
    pub spec: Option<&'a BuildingSpec>,
    /// The trades' documents' facts and rules.
    pub rules: &'a super::tradeatoms::Rules,
    /// The build's slab where nothing gives the floor, inches.
    pub default_in: f64,
}

/// A fact for `level` (one kept to it, else one for the whole building): its number and source.
fn fact_for(rules: &super::tradeatoms::Rules, key: &str, level: &str) -> Option<(f64, String)> {
    let mine = |f: &&super::tradeatoms::Fact| {
        f.scope.strip_prefix("level:").is_some_and(|code| code.trim().eq_ignore_ascii_case(level.trim()) || same_level(code, level))
    };
    let building = |f: &&super::tradeatoms::Fact| !f.scope.starts_with("level:") && !f.scope.starts_with("site");
    let f = rules.facts_of(key).find(mine).or_else(|| rules.facts_of(key).find(building))?;
    let n = f.number.filter(|n| n.is_finite() && *n > 0.0)?;
    let src = if f.quote.is_empty() { f.source.clone() } else { format!("{} (\"{}\")", f.source, f.quote.chars().take(60).collect::<String>()) };
    Some((n, src))
}

fn same_level(code: &str, name: &str) -> bool {
    let canon = |s: &str| cadaddy_walk3d::levels::level_or_code(s).map(|(c, _)| c);
    canon(code).is_some_and(|c| Some(c) == canon(name))
}

/// Each level's floor assembly (not the roofs, nor levels on other sheets), by name.
pub(crate) fn by_level(d: &Drawing, levels: &[Level], cx: &Context) -> Vec<Assembly> {
    let storeys: Vec<&Level> = levels.iter().filter(|l| l.sheet.is_none() && !super::wallgen::roof_level(levels, &l.name)).collect();
    let lowest =
        storeys.iter().filter(|l| l.elevation_ft.is_finite()).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).map(|l| l.name.clone());
    let notes = plan_notes(d);
    let whole = storeys.len() == 1;
    storeys
        .iter()
        .map(|l| {
            let mine: Vec<(String, String)> = notes
                .iter()
                .filter(|(_, at)| in_region(l, *at, whole))
                .take(MAX_LEVEL_NOTES)
                .map(|(t, _)| (t.clone(), format!("{} plan", l.name)))
                .collect();
            resolve(l, lowest.as_deref() == Some(l.name.as_str()), &mine, cx)
        })
        .collect()
}

/// A level's floor assembly from its plan's notes (`(words, where)`) and the context.
pub(crate) fn resolve(level: &Level, lowest: bool, plan: &[(String, String)], cx: &Context) -> Assembly {
    let w = words();
    let name = level.name.clone();
    let mut read = NoteRead::default();
    let mut add = |text: &str, from: &str| {
        let mut r = read_with(&w, text);
        let tag = |s: &mut Option<Said>| {
            if let Some(x) = s.as_mut() {
                x.2 = format!("{from}: {}", x.2.chars().take(120).collect::<String>());
            }
        };
        for s in [&mut r.slab, &mut r.topping, &mut r.deck, &mut r.member] {
            tag(s);
        }
        read.add(r);
    };
    for (t, from) in plan.iter().take(MAX_LEVEL_NOTES) {
        add(t, from);
    }
    let framing = cx.spec.and_then(|sp| sp.floor_framing_of(&name));
    if let Some(sp) = cx.spec {
        for n in sp.floor_notes_of(&name).into_iter().take(MAX_LEVEL_NOTES) {
            add(&n.text, if n.source.is_empty() { "the level's sheets" } else { n.source.as_str() });
        }
    }
    if let Some(f) = framing
        && !f.source.is_empty()
    {
        // The framing note itself may give the deck and topping (`... ON 3/4" SHEATHING ON 16"
        // JOISTS`); its sheet is its prefix.
        let (from, text) = f.source.split_once(": ").unwrap_or(("the framing notes", f.source.as_str()));
        add(text, from);
    }
    let mut notes = Vec::new();
    let mut layers = Vec::new();
    let framed = read.framed() || framing.is_some();
    let mut member = read.member.clone();
    if framed {
        if let Some((v, what, src)) = read.slab.take() {
            notes.push(format!("a {} {what} note on a framed floor is not its floor ({src})", cadaddy_walk3d::inches(v)));
        }
        match read.topping.take() {
            Some((v, what, src)) => layers.push(Layer { kind: Kind::Topping, name: what, inches: v, status: Status::Documented, source: src }),
            None => {
                if let Some((v, src)) = fact_for(cx.rules, "concrete.underlayment_in", &name).filter(|(v, _)| within(&w.topping_in, *v)) {
                    layers.push(Layer { kind: Kind::Topping, name: "underlayment".into(), inches: v, status: Status::Documented, source: src });
                }
            }
        }
        match read.deck.take() {
            Some((v, what, src)) => layers.push(Layer { kind: Kind::Deck, name: what, inches: v, status: Status::Documented, source: src }),
            None => {
                let v = cx.rules.num("framing.subfloor_in", 0.75);
                let documented = !cx.rules.is_default("framing.subfloor_in");
                let src = if documented {
                    cx.rules.source("framing.subfloor_in").doc.unwrap_or_default()
                } else {
                    super::provenance::rule(RULE).basis_for("the deck under the topping")
                };
                if within(&w.deck_in, v) {
                    layers.push(Layer {
                        kind: Kind::Deck,
                        name: "subfloor sheathing".into(),
                        inches: v,
                        status: if documented { Status::Documented } else { Status::Derived },
                        source: src,
                    });
                }
            }
        }
        if member.is_none()
            && let Some(f) = framing
            && f.depth_in.is_finite()
            && f.depth_in > 0.0
        {
            let what = if f.member == "truss" { "floor trusses" } else { "joists" };
            member = Some((f.depth_in, what.into(), if f.source.is_empty() { "the floor framing".into() } else { f.source.clone() }));
        }
    } else if let Some((v, what, src)) = read.slab.take() {
        layers.push(Layer { kind: Kind::Slab, name: what, inches: v, status: Status::Documented, source: src });
    } else if let Some(el) = cx.spec.and_then(|sp| {
        sp.concrete.elements.iter().find(|e| {
            e.kind.eq_ignore_ascii_case("slab")
                && (e.level.eq_ignore_ascii_case(&name) || sp.level_matching(&e.level).is_some_and(|i| sp.level_matching(&name) == Some(i)))
                && within(&w.slab_in, e.depth_in)
        })
    }) {
        layers.push(Layer {
            kind: Kind::Slab,
            name: "concrete slab".into(),
            inches: el.depth_in,
            status: Status::Documented,
            source: format!("the spec's concrete slab {}", el.id),
        });
    } else if let Some((v, src)) =
        lowest.then(|| fact_for(cx.rules, "concrete.slab_thickness_in", &name)).flatten().filter(|(v, _)| within(&w.slab_in, *v))
    {
        layers.push(Layer { kind: Kind::Slab, name: "slab on grade".into(), inches: v, status: Status::Documented, source: src });
    } else if cx.default_in > 0.0 && cx.default_in.is_finite() {
        layers.push(Layer {
            kind: Kind::Slab,
            name: "slab (default)".into(),
            inches: cx.default_in,
            status: Status::Default,
            source: "no slab or floor build-up in the documents read: the build's default slab thickness".into(),
        });
    }
    Assembly { level: name, framed, layers, member, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(name: &str, el: f64, region: Option<[f64; 4]>) -> Level {
        Level { name: name.into(), elevation_ft: el, height_ft: 10.0, ceiling_ft: 9.0, region, base: None, attr: None, sheet: None }
    }

    #[test]
    fn the_rule_data_loads() {
        let w = words();
        assert!(!w.slab_marks.is_empty() && !w.topping_words.is_empty() && !w.deck_words.is_empty() && !w.member_words.is_empty());
        assert!(within(&w.slab_in, 4.0) && within(&w.deck_in, 0.75) && within(&w.topping_in, 1.5) && within(&w.member_in, 11.875));
        assert!(!super::super::provenance::rule(RULE).basis.is_empty());
    }

    #[test]
    fn a_layered_note_reads_top_to_bottom() {
        let r = read_note("3/4\" GYPSUM CONCRETE ON 3/4\" SHEATHING ON 11 7/8\" I-JOISTS");
        assert_eq!(r.topping.as_ref().map(|t| (t.0, t.1.as_str())), Some((0.75, "gypsum concrete")));
        assert_eq!(r.deck.as_ref().map(|t| (t.0, t.1.as_str())), Some((0.75, "sheathing")));
        assert_eq!(r.member.as_ref().map(|t| t.0), Some(11.875));
        assert!(r.slab.is_none());
        // Other ways of writing it.
        let r = read_note("1-1/2\" gypcrete over 23/32\" T&G OSB subfloor over 16\" floor trusses");
        assert_eq!((r.topping.map(|t| t.0), r.deck.map(|t| t.0), r.member.map(|t| t.0)), (Some(1.5), Some(23.0 / 32.0), Some(16.0)));
    }

    #[test]
    fn slab_notes_give_the_slab() {
        let t = |s: &str| read_note(s).slab.map(|x| (x.0, x.1));
        assert_eq!(t("4\" THICK SLAB ON GRADE"), Some((4.0, "slab on grade".into())));
        assert_eq!(t("TYP. 5\" CONCRETE SLAB W/ 6X6 W2.9XW2.9 WWF"), Some((5.0, "concrete slab".into())));
        assert_eq!(t("SLAB THICKNESS = 6\""), Some((6.0, "concrete slab".into())));
        assert_eq!(t("4\" POST-TENSIONED SLAB ON GRADE"), Some((4.0, "post-tensioned slab on grade".into())));
        // Not a slab's thickness: out of range, a spacing, no slab named.
        assert_eq!(t("SLAB EDGE 24\" FROM GRID"), None);
        assert_eq!(t("2\" SLAB"), None);
        assert_eq!(t("12\" CMU WALL"), None);
    }

    #[test]
    fn spacings_and_fastenings_are_no_layers() {
        let r = read_note("SHEATHING NAILING: 6\" O.C. AT EDGES ON 16\" JOISTS");
        assert!(r.deck.is_none() && r.topping.is_none());
        let r = read_note("6\" O.C. SHEATHING");
        assert!(!r.is_assembly());
        // Hostile input never panics.
        for s in ["\"\"\"", "1/0\" SHEATHING", "99999999999999999999\" SLAB", "-\" ON \" ON", "\u{201d}", &"3/4\" ON ".repeat(500)] {
            let _ = read_note(s);
        }
        assert_eq!(inches_of("1/0"), None);
        assert_eq!(inches_of("11 7/8"), Some(11.875));
        assert_eq!(inches_of("1-1/2"), Some(1.5));
        assert_eq!(tail_inches("TYP. 4"), Some(4.0));
    }

    #[test]
    fn each_level_resolves_its_own_floor() {
        let rules = super::super::tradeatoms::Rules::default();
        let cx = Context { spec: None, rules: &rules, default_in: 6.0 };
        let l1 = level("1", 0.0, None);
        let a = resolve(&l1, true, &[("4\" THICK SLAB ON GRADE".into(), "1 plan".into())], &cx);
        assert!(!a.framed);
        assert_eq!((a.thickness_in(), a.status()), (4.0, Status::Documented));
        let l2 = level("2", 10.0, None);
        let a = resolve(&l2, false, &[("3/4\" GYPSUM CONCRETE ON 3/4\" SHEATHING ON 11 7/8\" I-JOISTS".into(), "2 plan".into())], &cx);
        assert!(a.framed);
        assert_eq!((a.thickness_in(), a.topping_in(), a.deck_in(), a.status()), (1.5, Some(0.75), Some(0.75), Status::Documented));
        assert_eq!(a.member.as_ref().map(|m| m.0), Some(11.875));
        assert!(a.tags().iter().any(|t| t == "floor:framed") && a.tags().iter().any(|t| t == "thick_in:1.5"));
        // A topping alone: the deck is the trade rule's, marked derived.
        let a = resolve(&l2, false, &[("1 1/2\" GYPSUM CONCRETE TOPPING".into(), "2 plan".into())], &cx);
        assert_eq!((a.thickness_in(), a.status()), (2.25, Status::Derived));
        // Nothing: the default, marked.
        let a = resolve(&l2, false, &[], &cx);
        assert_eq!((a.thickness_in(), a.status()), (6.0, Status::Default));
        assert!(a.tags().iter().any(|t| t == "thick_from:default"));
    }

    #[test]
    fn a_framed_level_ignores_a_slab_note_and_takes_its_framing() {
        let rules = super::super::tradeatoms::Rules::default();
        let sp: BuildingSpec = serde_json::from_value(json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}, {"name": "L2", "elevation_ft": 10, "height_ft": 10}],
            "floor_framing": [{"level": "L2", "member": "joist", "spacing_in": 24, "depth_in": 14, "source": "S-2: 14\" DEEP JOISTS @ 24\" OC"}],
            "floor_notes": [{"level": "L1", "text": "5\" CONCRETE SLAB ON GRADE", "source": "S-1"}]
        }))
        .unwrap();
        let cx = Context { spec: Some(&sp), rules: &rules, default_in: 6.0 };
        let a = resolve(&level("L2", 10.0, None), false, &[("4\" CONCRETE SLAB AT PAD".into(), "L2 plan".into())], &cx);
        assert!(a.framed && a.notes.len() == 1, "{a:?}");
        assert_eq!((a.thickness_in(), a.status()), (0.75, Status::Derived));
        assert_eq!(a.member.as_ref().map(|m| m.0), Some(14.0));
        let a = resolve(&level("L1", 0.0, None), true, &[], &cx);
        assert_eq!((a.framed, a.thickness_in(), a.status()), (false, 5.0, Status::Documented));
        assert!(a.basis().contains("S-1"), "{}", a.basis());
    }
}
