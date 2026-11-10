//! Each room's ceiling height, by rule (Q6512; trade practice `A-CEILING-HEIGHT` in
//! `tradepractice.json`): the reflected ceiling plans set ceiling heights. A room takes, in the
//! rule's `order`, the first of:
//! 1. `room_rcp`: its own RCP tag or note (`9'-0" AFF`, `CLG HT 9'-0"`), or a height its own
//!    record already carries from the documents (the room finish schedule, a user's `rcp.read`);
//! 2. `unit_rcp`: the enlarged plan or unit RCP of its room type;
//! 3. `legend`: its ceiling type's height in the ceiling legend;
//! 4. `section`: a section or interior elevation that dimensions its ceiling;
//! 5. `derived`: only then the underside of the assembly over it (the floor to floor less the
//!    members, deck and ceiling membrane over it), never under the rule's least height, marked
//!    inferred with its basis and asked of the architect.
//!
//! Each room's `ceiling_source` says which source set it (`unit RCP: 8'-6" ceiling tag on …`), so
//! a derived height can never pass for a read one. A lower source that disagrees with the one
//! chosen is reported beside it, never silently dropped.

use std::collections::BTreeMap;

use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

/// The trade practice rule this module follows.
pub(crate) const RULE: &str = "A-CEILING-HEIGHT";
/// Most rooms looked at (hostile specs).
const MAX_ROOMS: usize = 50_000;
/// Most candidates looked at.
const MAX_CANDIDATES: usize = 500_000;
/// Two heights this close (feet) agree.
const AGREE_FT: f64 = 1.0 / 24.0;
/// Most disagreements and derived rooms listed in a report.
const MAX_LISTED: usize = 200;

/// Where a room's ceiling height comes from, the rule's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Src {
    RoomRcp,
    UnitRcp,
    Legend,
    Section,
    Derived,
}

impl Src {
    pub(crate) const ALL: [Src; 5] = [Src::RoomRcp, Src::UnitRcp, Src::Legend, Src::Section, Src::Derived];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Src::RoomRcp => "room_rcp",
            Src::UnitRcp => "unit_rcp",
            Src::Legend => "legend",
            Src::Section => "section",
            Src::Derived => "derived",
        }
    }

    fn default_label(self) -> &'static str {
        match self {
            Src::RoomRcp => "room RCP",
            Src::UnitRcp => "unit RCP",
            Src::Legend => "ceiling type legend",
            Src::Section => "section",
            Src::Derived => "derived",
        }
    }

    fn parse(key: &str) -> Option<Src> {
        Src::ALL.into_iter().find(|f| f.key().eq_ignore_ascii_case(key.trim()))
    }
}

/// One source's height for one room: the room (its index in the spec's rooms), the source, the
/// height (feet above its floor) and what it read (the tag and its sheet or view).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub room: usize,
    pub from: Src,
    pub ft: f64,
    pub said: String,
}

/// The rule's numbers and order, from its rules data.
#[derive(Clone, Debug)]
pub(crate) struct Rule {
    /// The sources, first first, with the label each room's source carries.
    pub order: Vec<(Src, String)>,
    pub clear_ft: f64,
    pub default_depth_in: f64,
    pub deck_in: f64,
    pub membrane_in: f64,
    pub basis: String,
}

impl Rule {
    /// The rule as its rules data has it (the usual numbers where the data gives none).
    pub(crate) fn load() -> Rule {
        let d = super::provenance::rule_data(RULE);
        let num = |k: &str, dflt: f64, max: f64| d.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= max).unwrap_or(dflt);
        let mut order: Vec<(Src, String)> = Vec::new();
        for e in d.get("order").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(16) {
            let Some(f) = e.get("key").and_then(Value::as_str).and_then(Src::parse) else { continue };
            if order.iter().any(|(o, _)| *o == f) {
                continue;
            }
            let label = e.get("label").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty()).unwrap_or(f.default_label());
            order.push((f, label.chars().take(40).collect()));
        }
        // Every source has a place; the derived height is always the last resort.
        for f in Src::ALL {
            if !order.iter().any(|(o, _)| *o == f) {
                order.push((f, f.default_label().to_string()));
            }
        }
        if let Some(i) = order.iter().position(|(f, _)| *f == Src::Derived) {
            let d = order.remove(i);
            order.push(d);
        }
        let rule = super::provenance::rule(RULE);
        Rule {
            order,
            clear_ft: num("clear_ft", 7.5, 30.0),
            default_depth_in: num("default_size_in", 24.0, 120.0),
            deck_in: num("deck_in", 1.5, 24.0),
            membrane_in: num("membrane_in", 1.5, 24.0),
            basis: rule.basis_for(""),
        }
    }

    fn rank(&self, f: Src) -> usize {
        self.order.iter().position(|(o, _)| *o == f).unwrap_or(usize::MAX)
    }

    fn label(&self, f: Src) -> &str {
        self.order.iter().find(|(o, _)| *o == f).map_or(f.default_label(), |(_, l)| l.as_str())
    }

    /// The source a room's `ceiling_source` names, when this rule set it (`unit RCP: …`).
    fn source_of(&self, said: &str) -> Option<Src> {
        let s = said.trim();
        self.order
            .iter()
            .find(|(_, l)| s.get(..l.len()).is_some_and(|p| p.eq_ignore_ascii_case(l)) && s.get(l.len()..).is_some_and(|r| r.starts_with(':')))
            .map(|(f, _)| *f)
    }
}

/// What the rule did: rooms set by each source, the rooms derived (asked of the architect), and
/// where a lower source disagrees with the one taken.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settled {
    pub by_source: BTreeMap<&'static str, usize>,
    /// Level, room, height (feet) and basis of each derived room.
    pub derived: Vec<(String, String, f64, String)>,
    /// Room, the source taken and its height, the source passed over and its height.
    pub disagree: Vec<(String, String, f64, String, f64)>,
    /// Rooms left at their level's ceiling (no level known to derive one on).
    pub unset: usize,
}

impl Settled {
    pub(crate) fn to_json(&self) -> Value {
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        json!({
            "rule": RULE,
            "by_source": self.by_source,
            "derived": self.derived.iter().take(MAX_LISTED).map(|(l, r, ft, b)| json!({"level": l, "room": r, "height_ft": r2(*ft), "basis": b})).collect::<Vec<_>>(),
            "derived_total": self.derived.len(),
            "disagree": self.disagree.iter().take(MAX_LISTED).map(|(r, a, af, b, bf)| json!({"room": r, "taken": a, "taken_ft": r2(*af), "passed_over": b, "passed_over_ft": r2(*bf)})).collect::<Vec<_>>(),
            "disagree_total": self.disagree.len(),
            "unset": self.unset,
        })
    }
}

/// The room's label for reports: its number, else its name.
fn room_label(r: &cadaddy_walk3d::SpecRoom) -> String {
    r.number.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| r.name.trim().to_string(), str::to_string)
}

/// `9'-6"` for 9.5.
pub(crate) fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{}'-{}\"", total / 12, total % 12)
}

/// A ceiling type's legend height: the one it carries, else a height its description notes
/// (`GYP BD CLG @ 9'-0" AFF`). With what it read.
pub(crate) fn legend_height(ct: &cadaddy_walk3d::CeilingType) -> Option<(f64, String)> {
    if let Some(h) = ct.height_ft.filter(|h| h.is_finite() && (6.0..=60.0).contains(h)) {
        return Some((h, format!("{} at {} in the ceiling legend", ct.name, ft_in(h))));
    }
    let d = ct.description.trim();
    let h = height_in_text(d)?;
    Some((h, format!("{} \"{}\" in the ceiling legend", ct.name, d.chars().take(80).collect::<String>())))
}

/// A ceiling height inside a longer text (a legend entry: `GYP BD CLG @ 9'-0" AFF`): a
/// feet-and-inches height of one to three words with a ceiling word (`AFF`, `CLG`, `CEILING`)
/// or `@` right beside it. The first.
pub(crate) fn height_in_text(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    let words: Vec<&str> = up.split_whitespace().take(64).collect();
    let ceiling_word = |w: &str| {
        let w = w.trim_matches(|c: char| c == ',' || c == ';' || c == '(' || c == ')');
        ["AFF", "A.F.F.", "A.F.F", "CLG", "CLG.", "CEILING", "@", "HT", "HT.", "=", "CLG.HT."].contains(&w)
    };
    for i in 0..words.len() {
        for n in 1..=3usize {
            let Some(span) = words.get(i..i + n) else { break };
            if !span.first().is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit())) {
                break;
            }
            let Some((ft, _)) = height_note(&span.join(" ")) else { continue };
            let before = i.checked_sub(1).and_then(|j| words.get(j)).is_some_and(|w| ceiling_word(w));
            let after = words.get(i + n).is_some_and(|w| ceiling_word(w));
            if before || after {
                return Some(ft);
            }
        }
    }
    None
}

/// A ceiling height note (`9'-0" AFF`, `9'-0" A.F.F.`, `CLG HT 9'-0"`, `CLG. HT. = 8'-6"`,
/// `CEILING @ 9'-0"`, `8'-6" CLG`): the height (feet) and whether it carries a ceiling word
/// (`AFF`, `CLG`, `CEILING`) — a bare `9'-0"` is any dimension. None for a height above
/// something else (`ABOVE`) or out of a ceiling's range.
pub(crate) fn height_note(t: &str) -> Option<(f64, bool)> {
    let mut u = t.trim().to_ascii_uppercase();
    if u.is_empty() || u.len() > 60 || u.contains("ABOVE") {
        return None;
    }
    let mut worded = false;
    // Words before the number, longest first.
    for pre in
        ["CEILING HEIGHT", "CEILING HT.", "CEILING HT", "CEILING", "CEIL. HT.", "CEIL. HT", "CLG. HT.", "CLG. HT", "CLG HT.", "CLG HT", "CLG.", "CLG"]
    {
        if let Some(rest) = u.strip_prefix(pre) {
            u = rest.trim_start_matches([' ', ':', '=', '@', '.']).to_string();
            worded = true;
            break;
        }
    }
    // Words after it.
    for post in ["A.F.F.", "A.F.F", "AFF", "CLG. HT.", "CLG HT", "CLG.", "CLG", "CEILING"] {
        if let Some(rest) = u.trim_end().strip_suffix(post) {
            u = rest.trim_end().trim_end_matches(['@', ' ']).to_string();
            worded = true;
            break;
        }
    }
    let core = u.trim();
    if core.chars().any(|c| !(c.is_ascii_digit() || " '\"-/.".contains(c))) || !core.contains('\'') {
        return None;
    }
    let ft = super::perceive::feet_inches(core)? / 12.0;
    (6.5..=20.0).contains(&ft).then_some((ft, worded))
}

/// The height under the assembly over a level's rooms, feet above its floor, and why: the next
/// storey's floor (its framing's members, deck and the ceiling membrane under them) or roof (its
/// members' bearing, else their depth and deck) over the floor to floor, else the rule's usual
/// depth; never under the rule's least ceiling height, never at the next floor.
pub(crate) fn derived_height(sp: &BuildingSpec, level: &str, rule: &Rule) -> Option<(f64, String)> {
    let lv = sp.level(level)?;
    if !(lv.elevation_ft.is_finite() && lv.height_ft.is_finite() && lv.height_ft > 0.0) {
        return None;
    }
    let top = lv.elevation_ft + lv.height_ft;
    // The storey over it: the one at its top.
    let above = sp.storeys().into_iter().filter(|(_, e)| (*e - top).abs() < 0.5 && *e > lv.elevation_ft).map(|(n, _)| n).next();
    let m = rule.membrane_in / 12.0;
    let (underside, why) = match above.as_deref() {
        Some(a) if sp.floor_framing_of(a).is_some_and(|f| f.depth_in.is_finite() && f.depth_in > 0.0) => {
            let f = sp.floor_framing_of(a).map_or(0.0, |f| f.depth_in);
            let d = (f + rule.deck_in) / 12.0;
            (top - d - m, format!("{a}'s floor framing {}\" deep and {}\" deck over it", f, rule.deck_in))
        }
        Some(a) if sp.exterior.level_framing(a).is_some() => {
            let f = sp.exterior.level_framing(a).cloned();
            match f {
                Some(f) if f.bearing_ft.is_some_and(|b| b.is_finite() && b > lv.elevation_ft && b <= top) => {
                    let b = f.bearing_ft.unwrap_or(top);
                    (b - m, format!("the {} of {a} bearing at {}", f.member, ft_in(b - lv.elevation_ft)))
                }
                Some(f) if f.depth_in.is_finite() && f.depth_in > 0.0 => {
                    let d = (f.depth_in + rule.deck_in) / 12.0;
                    (top - d - m, format!("{a}'s {} {}\" deep and {}\" deck", f.member, f.depth_in, rule.deck_in))
                }
                _ => (top - rule.default_depth_in / 12.0, format!("{a}'s framing depth not read: the usual {}\"", rule.default_depth_in)),
            }
        }
        Some(a) => (top - rule.default_depth_in / 12.0, format!("{a}'s floor framing not read: the usual {}\" assembly", rule.default_depth_in)),
        None => (top - rule.default_depth_in / 12.0, format!("nothing read over it: the usual {}\" assembly", rule.default_depth_in)),
    };
    let h = underside - lv.elevation_ft;
    let most = lv.height_ft - 0.5;
    let ft = h.max(rule.clear_ft).min(most);
    if !(ft.is_finite() && ft > 0.0) {
        return None;
    }
    let clamp = if h < rule.clear_ft - 1e-6 && ft >= rule.clear_ft - 1e-6 {
        format!("; raised to the least ceiling height, {}", ft_in(rule.clear_ft))
    } else {
        String::new()
    };
    Some((
        ft,
        format!(
            "inferred: no RCP, unit RCP, legend or section gives one; the underside of the assembly over the room, {} floor to floor less {}, {}\" membrane{clamp}; {}",
            ft_in(lv.height_ft),
            why,
            rule.membrane_in,
            rule.basis
        ),
    ))
}

/// Set each room's ceiling height from its candidates by the rule: the first source in the
/// rule's order that gives one; a room with none keeps a height its own record carries from the
/// documents, else is derived (marked inferred). `ceiling_source` names the source.
pub(crate) fn settle(sp: &mut BuildingSpec, cands: &[Candidate], rule: &Rule) -> Settled {
    let mut out = Settled::default();
    let mut by_room: BTreeMap<usize, Vec<&Candidate>> = BTreeMap::new();
    for c in cands.iter().take(MAX_CANDIDATES).filter(|c| c.ft.is_finite() && c.ft > 0.0) {
        by_room.entry(c.room).or_default().push(c);
    }
    let n = sp.rooms.len().min(MAX_ROOMS);
    let mut derive: Vec<usize> = Vec::new();
    for i in 0..n {
        let Some(r) = sp.rooms.get(i) else { break };
        // A height already on the room: this rule's own (from an earlier pass, read again now),
        // else the room's own record from the documents (its finish schedule row, a user's read).
        let mine = r.ceiling_source.as_deref().and_then(|s| rule.source_of(s));
        let own = match (r.ceiling_ft.filter(|c| c.is_finite() && *c > 0.0), mine) {
            (Some(c), None) => Some(Candidate {
                room: i,
                from: Src::RoomRcp,
                ft: c,
                said: r.ceiling_source.clone().unwrap_or_else(|| "the room's own record".into()),
            }),
            _ => None,
        };
        let mut list: Vec<&Candidate> = by_room.get(&i).cloned().unwrap_or_default();
        if let Some(o) = own.as_ref() {
            list.insert(0, o);
        }
        // Stable: of one source, the first given stands.
        list.sort_by_key(|c| rule.rank(c.from));
        let Some(best) = list.first().copied() else {
            derive.push(i);
            continue;
        };
        let label = rule.label(best.from).to_string();
        for o in list.iter().skip(1).filter(|o| o.from != best.from && (o.ft - best.ft).abs() > AGREE_FT) {
            out.disagree.push((room_label(r), format!("{label}: {}", best.said), best.ft, format!("{}: {}", rule.label(o.from), o.said), o.ft));
        }
        let (ft, said) =
            (best.ft, if own.as_ref().is_some_and(|o| std::ptr::eq(o, best)) { best.said.clone() } else { format!("{label}: {}", best.said) });
        *out.by_source.entry(best.from.key()).or_default() += 1;
        if let Some(r) = sp.rooms.get_mut(i) {
            r.ceiling_ft = Some(ft);
            r.ceiling_source = Some(said.chars().take(240).collect());
        }
    }
    // The rest: the underside of the assembly over them.
    let only = (sp.levels.len() == 1).then(|| sp.levels.first().map(|l| l.name.clone())).flatten();
    let label = rule.label(Src::Derived).to_string();
    for i in derive {
        let Some(level) = sp.rooms.get(i).and_then(|r| r.level.clone()).or_else(|| only.clone()) else {
            out.unset += 1;
            continue;
        };
        let Some((ft, why)) = derived_height(sp, &level, rule) else {
            out.unset += 1;
            continue;
        };
        if let Some(r) = sp.rooms.get_mut(i) {
            r.ceiling_ft = Some(ft);
            r.ceiling_source = Some(format!("{label}: {} {why}", ft_in(ft)).chars().take(480).collect());
            out.derived.push((level, room_label(r), ft, why));
        }
        *out.by_source.entry(Src::Derived.key()).or_default() += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> BuildingSpec {
        BuildingSpec::from_json(
            r#"{"version":1,"units_per_foot":12,
                "levels":[{"name":"L1","elevation_ft":0,"height_ft":10,"ceiling_ft":8},{"name":"L2","elevation_ft":10,"height_ft":10,"ceiling_ft":8}],
                "rooms":[{"name":"Suite A","number":"R-11","level":"L1"},{"name":"Suite A","number":"R-12","level":"L1"},{"name":"Store","number":"R-13","level":"L1"},{"name":"Office","number":"R-14","level":"L1","ceiling_ft":9}],
                "floor_framing":[{"level":"L2","member":"joist","spacing_in":16,"depth_in":14}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn cand(room: usize, from: Src, ft: f64, said: &str) -> Candidate {
        Candidate { room, from, ft, said: said.into() }
    }

    #[test]
    fn the_rule_reads_its_order_from_the_rules_data() {
        let r = Rule::load();
        let keys: Vec<&str> = r.order.iter().map(|(f, _)| f.key()).collect();
        assert_eq!(keys, ["room_rcp", "unit_rcp", "legend", "section", "derived"]);
        assert_eq!(r.label(Src::UnitRcp), "unit RCP");
        assert!((r.clear_ft - 7.5).abs() < 1e-9);
        assert!(r.basis.starts_with("A-CEILING-HEIGHT"));
    }

    #[test]
    fn the_first_source_in_the_order_sets_the_room() {
        let rule = Rule::load();
        let mut sp = spec();
        // Unit RCP and section agree at 8'-6"; the legend has none for the type; the room's own
        // tag beats the unit RCP on R-12.
        let c = vec![
            cand(0, Src::Section, 8.5, "8'-6\" measured on S-1 SECTION"),
            cand(0, Src::UnitRcp, 8.5, "8'-6\" ceiling tag on A-5 UNIT RCP"),
            cand(1, Src::UnitRcp, 8.5, "8'-6\" ceiling tag on A-5 UNIT RCP"),
            cand(1, Src::RoomRcp, 9.0, "9'-0\" ceiling tag on A-2 RCP"),
        ];
        let s = settle(&mut sp, &c, &rule);
        assert_eq!(sp.rooms[0].ceiling_ft, Some(8.5));
        assert!(sp.rooms[0].ceiling_source.as_deref().is_some_and(|s| s.starts_with("unit RCP: ")), "{:?}", sp.rooms[0].ceiling_source);
        assert_eq!(sp.rooms[1].ceiling_ft, Some(9.0));
        assert!(sp.rooms[1].ceiling_source.as_deref().is_some_and(|s| s.starts_with("room RCP: ")));
        // R-12's unit RCP disagrees with its own tag: reported, not dropped.
        assert_eq!(s.disagree.len(), 1);
        assert_eq!(s.disagree[0].0, "R-12");
        // R-14 keeps the height its record carries from the documents.
        assert_eq!(sp.rooms[3].ceiling_ft, Some(9.0));
        assert_eq!(s.by_source.get("unit_rcp"), Some(&1));
        assert_eq!(s.by_source.get("room_rcp"), Some(&2));
    }

    #[test]
    fn a_room_no_source_gives_is_derived_and_marked_inferred() {
        let rule = Rule::load();
        let mut sp = spec();
        let s = settle(&mut sp, &[], &rule);
        // 10' floor to floor less 14" joists, 1.5" deck and 1.5" membrane: 8'-5".
        let r = &sp.rooms[2];
        let ft = r.ceiling_ft.unwrap_or_default();
        assert!((ft - (10.0 - 17.0 / 12.0)).abs() < 1e-9, "{ft}");
        let src = r.ceiling_source.clone().unwrap_or_default();
        assert!(src.starts_with("derived: "), "{src}");
        assert!(src.contains("inferred") && src.contains("A-CEILING-HEIGHT") && src.contains("floor framing"), "{src}");
        assert_eq!(s.derived.len(), 3, "{:?}", s.derived);
        // Settled again: the derived rooms are derived again, not kept as documented.
        let again = settle(&mut sp, &[], &rule);
        assert_eq!(again.derived.len(), 3);
    }

    #[test]
    fn a_derived_height_is_never_under_the_least_ceiling_height() {
        let rule = Rule::load();
        let mut sp = BuildingSpec::from_json(
            r#"{"version":1,"units_per_foot":12,"levels":[{"name":"L1","elevation_ft":0,"height_ft":8.5,"ceiling_ft":7},{"name":"L2","elevation_ft":8.5,"height_ft":10}],"rooms":[{"name":"Hall","level":"L1"}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        settle(&mut sp, &[], &rule);
        assert_eq!(sp.rooms[0].ceiling_ft, Some(7.5));
        assert!(sp.rooms[0].ceiling_source.as_deref().is_some_and(|s| s.contains("least ceiling height")));
    }

    #[test]
    fn the_legend_gives_a_types_height() {
        let ct: cadaddy_walk3d::CeilingType =
            serde_json::from_value(json!({"name": "C1", "description": "GYP BD CLG @ 9'-0\" AFF"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(legend_height(&ct).map(|x| x.0), Some(9.0));
        let none: cadaddy_walk3d::CeilingType =
            serde_json::from_value(json!({"name": "C2", "description": "5/8\" GYP BD ON 7/8\" CHANNEL"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(legend_height(&none), None);
        let after: cadaddy_walk3d::CeilingType =
            serde_json::from_value(json!({"name": "C4", "description": "ACT 2X2 GRID, 8'-6\" CLG, SEE RCP"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(legend_height(&after).map(|x| x.0), Some(8.5));
        let dim: cadaddy_walk3d::CeilingType =
            serde_json::from_value(json!({"name": "C5", "description": "HANGERS 4'-0\" O.C."})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(legend_height(&dim), None);
        let given: cadaddy_walk3d::CeilingType = serde_json::from_value(json!({"name": "C3", "height_ft": 8.0})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(legend_height(&given).map(|x| x.0), Some(8.0));
    }

    #[test]
    fn height_notes_in_their_usual_forms() {
        for (t, ft, worded) in [
            ("9'-0\" AFF", 9.0, true),
            ("9'-0\" A.F.F.", 9.0, true),
            ("CLG HT 8'-6\"", 8.5, true),
            ("CLG. HT. = 8'-6\"", 8.5, true),
            ("CEILING @ 9'-0\"", 9.0, true),
            ("8'-6\" CLG", 8.5, true),
            ("9'-0\"", 9.0, false),
        ] {
            assert_eq!(height_note(t), Some((ft, worded)), "{t}");
        }
        for t in ["12'-0\" ABOVE", "CLG", "C1", "30'-0\" AFF", "ROOM 9'-0\""] {
            assert_eq!(height_note(t), None, "{t}");
        }
    }
}
