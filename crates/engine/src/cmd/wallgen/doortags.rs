//! Doors the door schedule lists where the plan draws no opening (Q6609).
//!
//! A plan breaks its walls for its swing doors, and the build hangs a door in each gap. A counter
//! shutter over a counter top, an overhead coiling or sliding door drawn as a dashed line, or a
//! door the drafter left in a wall drawn through has no gap: no door was built for it, and the
//! opening was out of the door count, the hardware and the takeoff. Each door schedule tag (the
//! spec's `schedule.tags` of a door row) that no door built near it answers is built where the
//! field hangs it (trade practice rule [`RULE_ID`]): cut into the wall that holds it at its tag,
//! else set on the nearest line drawn there (a counter's edge, the shutter's line), else at the
//! tag along the nearest wall. Its size is the schedule's; its sill, head and how it opens are
//! the schedule's words, else what its size says ([`door_make`]: lower than a passage door, an
//! opening over a counter; wider than one leaf, a pair, or a coiling shutter over a counter).
//! Each carries its mark, how it was placed and its provenance (inferred: no opening is drawn),
//! so it never passes for a drawn door. A tag on another sheet registered onto the level's plan
//! (an enlarged plan) is put onto the plan first. A tag a person places (`plan.door` with a mark)
//! is one of these tags: kept in the spec, every build builds its door again.

use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, Family, FamilyKind, Level, ScheduleRow};
use serde_json::Value;

use super::{Cut, LEAF_IN, Leaf, MAX_OBJECTS, PlanLevel, host_wall, seg_dist};
use crate::cmd::perceive::geometry::Seg;

/// The trade practice rule (`tradepractice.json`) these doors are built by.
pub(crate) const RULE_ID: &str = "A-DOOR-AT-SCHEDULE-TAG";

/// Most door tags looked at on a level (hostile specs).
const MAX_TAGS: usize = 5_000;
/// Highest head a schedule may give a door above its floor, inches.
const MAX_HEAD_IN: f64 = 480.0;
/// Widest door a schedule may give, inches.
const MAX_WIDTH_IN: f64 = 600.0;

/// The rule's numbers and words ([`RULE_ID`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DoorRule {
    /// How far from its tag a door's wall or line may be; a door built this close to a tag is
    /// that tag's (feet).
    pub reach_ft: f64,
    pub connect_ft: f64,
    /// The lowest door people walk through, the counter top an opening lower than that closes
    /// onto, the widest single leaf, a row's width when it has none (inches).
    pub passage_min_in: f64,
    pub counter_sill_in: f64,
    pub max_leaf_in: f64,
    pub default_width_in: f64,
    /// How a door opens by the words of its type or frame, in order (`rolling`, `sliding`, …).
    pub operation_words: Vec<(String, Vec<String>)>,
    /// The words that put it over a counter.
    pub counter_words: Vec<String>,
    /// The rule's basis (every door it builds carries it).
    pub basis: String,
}

impl DoorRule {
    /// The rule as its data has it; none when the data has no such rule (nothing is built).
    pub(crate) fn load() -> Option<Self> {
        Self::from_value(&super::super::provenance::rule_data(RULE_ID))
    }

    fn from_value(v: &Value) -> Option<Self> {
        let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0);
        let words = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(200).collect()
        };
        // In the order they are tried: what a door is (coiling, sliding, folding) before how
        // many leaves it has (`DOUBLE SLIDING` slides).
        let ops = v.get("operation_words");
        let operation_words =
            OPERATIONS.iter().map(|op| (op.to_string(), words(ops.and_then(|o| o.get(op))))).filter(|(_, w)| !w.is_empty()).collect();
        Some(Self {
            reach_ft: num("reach_ft")?,
            connect_ft: num("connect_reach_ft")?,
            passage_min_in: num("passage_min_height_in")?,
            counter_sill_in: num("counter_sill_in")?,
            max_leaf_in: num("max_leaf_in")?,
            default_width_in: num("default_width_in").filter(|w| *w > 0.0)?,
            operation_words,
            counter_words: words(v.get("counter_words")),
            basis: v.get("basis").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    }

    /// How a door of these words opens, by the rule's word lists (the first list with a word).
    fn operation_by_words(&self, words: &str) -> Option<&'static str> {
        let up = spaced(words);
        self.operation_words
            .iter()
            .find(|(_, ws)| ws.iter().any(|w| !w.trim().is_empty() && up.contains(&spaced(w))))
            .and_then(|(op, _)| operation_name(op))
    }

    fn says_counter(&self, words: &str) -> bool {
        let up = spaced(words);
        self.counter_words.iter().any(|w| !w.trim().is_empty() && up.contains(&spaced(w)))
    }
}

/// The operations the rule data's words may name, in the order they are tried; a door no word
/// names swings.
const OPERATIONS: [&str; 4] = ["rolling", "sliding", "folding", "pair"];

/// The operation names a door can have, as `'static` text.
fn operation_name(op: &str) -> Option<&'static str> {
    OPERATIONS.into_iter().chain(["swing"]).find(|o| o.eq_ignore_ascii_case(op.trim()))
}

/// Words as whole words: upper case, anything but letters and digits a space, a space each side.
fn spaced(s: &str) -> String {
    let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

/// How a scheduled door is made: its width, sill and head (inches above its floor), how it
/// opens, whether it closes onto a counter, and what of it the rule gave (the schedule didn't).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DoorMake {
    pub width_in: f64,
    pub sill_in: f64,
    pub head_in: f64,
    pub operation: &'static str,
    pub counter: bool,
    pub inferred: Vec<&'static str>,
}

/// A door row made into a door: the row's size (else its type family's, else the rule's default
/// width and the level's door head `default_head_in`); over a counter when its words say so or
/// it is lower than a passage door, its sill then the counter top (unless the row gives one);
/// how it opens by its words, else by its size (wider than one leaf: a coiling shutter over a
/// counter, a pair elsewhere), else a swing door.
pub(crate) fn door_make(row: &ScheduleRow, family: Option<&Family>, default_head_in: f64, rule: &DoorRule) -> DoorMake {
    let ok = |v: f64, max: f64| (v.is_finite() && v > 0.0 && v <= max).then_some(v);
    let words = [row.kind_type.as_deref(), row.frame.as_deref(), family.map(|f| f.name.as_str())].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let mut inferred = Vec::new();
    let width_in = match row.width_in.and_then(|w| ok(w, MAX_WIDTH_IN)).or_else(|| family.and_then(|f| f.width_in).and_then(|w| ok(w, MAX_WIDTH_IN)))
    {
        Some(w) => w,
        None => {
            inferred.push("width");
            rule.default_width_in
        }
    };
    let height_in = row.height_in.and_then(|h| ok(h, MAX_HEAD_IN));
    let counter = rule.says_counter(&words) || height_in.is_some_and(|h| h < rule.passage_min_in);
    let sill_in = match row.sill_ft.map(|s| s * 12.0).filter(|s| s.is_finite() && (0.0..MAX_HEAD_IN).contains(s)) {
        Some(s) => s,
        None if counter => {
            inferred.push("sill");
            rule.counter_sill_in
        }
        None => family.and_then(|f| f.sill_ft).map(|s| s * 12.0).filter(|s| s.is_finite() && (0.0..MAX_HEAD_IN).contains(s)).unwrap_or(0.0),
    };
    let head_in = match row.head_ft.map(|h| h * 12.0).and_then(|h| ok(h, MAX_HEAD_IN)).or_else(|| height_in.map(|h| sill_in + h)) {
        Some(h) => h,
        None => match family.and_then(|f| f.head_ft).map(|h| h * 12.0).and_then(|h| ok(h, MAX_HEAD_IN)) {
            Some(h) => h,
            None => {
                inferred.push("head");
                default_head_in
            }
        },
    };
    let head_in = if head_in > sill_in { head_in.min(MAX_HEAD_IN) } else { sill_in + height_in.unwrap_or(rule.passage_min_in) };
    let operation = match rule.operation_by_words(&words) {
        Some(op) => op,
        None => {
            let wide = width_in > rule.max_leaf_in;
            if wide {
                inferred.push("operation");
            }
            match (wide, counter) {
                (true, true) => "rolling",
                (true, false) => "pair",
                _ => "swing",
            }
        }
    };
    DoorMake { width_in, sill_in, head_in, operation, counter, inferred }
}

/// A door schedule tag on a level's plan: where (sheet units, on the level's plan), its mark and
/// row, and the sheet it was printed on when that is another sheet put onto the plan.
#[derive(Clone, Debug)]
pub(crate) struct DoorTag {
    pub at: Vec2,
    pub mark: String,
    pub row: ScheduleRow,
}

/// Whether `p` lies in the rectangle `r` (`[x0, y0, x1, y1]`, either way round).
fn inside(r: [f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0].min(r[2]) && p[0] <= r[0].max(r[2]) && p[1] >= r[1].min(r[3]) && p[1] <= r[1].max(r[3])
}

/// The level's door schedule tags: the spec's tags of door rows, each on another sheet that is
/// registered onto the level's plan put onto it; a tag naming its level is that level's, one
/// naming none is the level's when it lies in the level's region (every level's without one).
/// Tags of one mark within `connect` (sheet units) of each other are one door.
pub(crate) fn level_tags(sp: &BuildingSpec, level: &Level, connect: f64) -> Vec<DoorTag> {
    let covers = |l: &cadaddy_walk3d::SheetLink| {
        l.level.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&level.name)) || l.levels.iter().any(|n| n.eq_ignore_ascii_case(&level.name))
    };
    let mut out: Vec<DoorTag> = Vec::new();
    for tag in sp.schedule.tags.iter().take(MAX_TAGS) {
        let Some(row) = sp.schedule.row(&tag.mark).filter(|r| r.kind == FamilyKind::Door) else { continue };
        if !tag.at.iter().all(|c| c.is_finite()) {
            continue;
        }
        let named = tag.level.as_deref().map(|l| l.eq_ignore_ascii_case(&level.name));
        if named == Some(false) {
            continue;
        }
        // Printed on another sheet registered onto this level's plan: put onto the plan.
        let reg = sp.sheets.iter().filter(|l| covers(l)).find_map(|l| {
            let r = l.registration.as_ref().filter(|r| !r.method.eq_ignore_ascii_case("reference"))?;
            l.rect.filter(|rect| inside(*rect, tag.at)).map(|_| r)
        });
        let at = reg.map_or(tag.at, |r| r.to_project(tag.at));
        if !at.iter().all(|c| c.is_finite()) || (named.is_none() && level.region.is_some_and(|r| !inside(r, at))) {
            continue;
        }
        let at = Vec2::new(at[0], at[1]);
        let mark = tag.mark.trim().to_string();
        if out.iter().any(|t| t.mark.eq_ignore_ascii_case(&mark) && t.at.dist(at) <= connect) {
            continue;
        }
        out.push(DoorTag { at, mark, row: row.clone() });
    }
    out
}

/// How a door built at its tag was placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placed {
    /// Cut into the wall that holds it at its tag.
    OnWall,
    /// No wall there holds it: on the nearest line drawn there (a counter's edge, the shutter's
    /// line).
    OnLine,
    /// Nothing drawn there: at its tag, along the nearest wall.
    AtTag,
}

impl Placed {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OnWall => "schedule_tag_on_wall",
            Self::OnLine => "schedule_tag_on_line",
            Self::AtTag => "schedule_tag_alone",
        }
    }

    fn why(self) -> &'static str {
        match self {
            Self::OnWall => "no opening drawn: cut into the wall at its tag",
            Self::OnLine => "no opening or wall drawn there: set on the line drawn at its tag",
            Self::AtTag => "nothing drawn at its tag: set there along the nearest wall",
        }
    }
}

/// A door built at its schedule tag: how it was placed, how it opens, whether it closes onto a
/// counter, and its basis (the rule, how it was placed, what the rule gave).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AtTag {
    pub placed: Placed,
    pub operation: &'static str,
    pub counter: bool,
    pub basis: String,
}

/// What [`build`] did on a level: each tag's door (its mark, where, how placed) and the tags no
/// door could be built for.
#[derive(Clone, Debug, Default)]
pub(crate) struct DoorTagReport {
    pub built: Vec<(String, Vec2, Placed)>,
    pub answered: usize,
    pub no_host: Vec<(String, Vec2)>,
}

/// Most doors listed in a level's report row.
const MAX_LISTED: usize = 200;

impl DoorTagReport {
    pub(crate) fn is_empty(&self) -> bool {
        self.built.is_empty() && self.no_host.is_empty() && self.answered == 0
    }

    /// The report row for `level` (positions in sheet units).
    pub(crate) fn to_json(&self, level: &str) -> Value {
        serde_json::json!({
            "level": level,
            "built": self.built.iter().take(MAX_LISTED).map(|(m, at, p)| serde_json::json!({"mark": m, "at": [at.x, at.y], "placed": p.as_str()})).collect::<Vec<_>>(),
            "built_count": self.built.len(),
            "answered": self.answered,
            "no_host": self.no_host.iter().take(MAX_LISTED).map(|(m, at)| serde_json::json!({"mark": m, "at": [at.x, at.y]})).collect::<Vec<_>>(),
        })
    }
}

/// The plan's lines within a distance of a point (sheet units), each with the object it is drawn
/// by.
pub(crate) type LinesNear<'a> = dyn Fn(Vec2, f64) -> Vec<(Seg, cadaddy_doc::Handle)> + 'a;

/// Build the doors of the level's door schedule `tags` that no door built answers (a door within
/// the rule's connect reach; one door answers one tag, nearest first). `lines`: the plan's lines
/// near a point; `slot`: the narrowest opening (sheet units); `k`: sheet units an inch.
pub(crate) fn build(
    pl: &mut PlanLevel,
    tags: &[DoorTag],
    spec: &BuildingSpec,
    lines: &LinesNear,
    rule: &DoorRule,
    k: f64,
    slot: f64,
) -> DoorTagReport {
    let mut report = DoorTagReport::default();
    if tags.is_empty() || !(k.is_finite() && k > 0.0) {
        return report;
    }
    let (reach, connect) = (rule.reach_ft * 12.0 * k, rule.connect_ft * 12.0 * k);
    // Each tag against the doors built: nearest first, one door per tag.
    let doors: Vec<usize> = pl.leaves.iter().enumerate().filter(|(_, l)| !l.window).map(|(i, _)| i).take(MAX_OBJECTS).collect();
    let dist = |t: &DoorTag, l: &Leaf| {
        let d = seg_dist(t.at, l.a, l.b);
        l.frame.map_or(d, |(a, b)| d.min(seg_dist(t.at, a, b)))
    };
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (ti, t) in tags.iter().enumerate() {
        for &li in &doors {
            if let Some(l) = pl.leaves.get(li) {
                let d = dist(t, l);
                if d.is_finite() && d <= connect && pairs.len() < MAX_OBJECTS {
                    pairs.push((d, ti, li));
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut tag_done, mut leaf_done) = (vec![false; tags.len()], vec![false; pl.leaves.len()]);
    for (_, ti, li) in pairs {
        if tag_done.get(ti).copied().unwrap_or(true) || leaf_done.get(li).copied().unwrap_or(true) {
            continue;
        }
        if let (Some(t), Some(l)) = (tag_done.get_mut(ti), leaf_done.get_mut(li)) {
            *t = true;
            *l = true;
            report.answered += 1;
        }
    }
    let default_head_in = pl.door_head / k;
    for (t, _) in tags.iter().zip(&tag_done).filter(|(_, done)| !**done) {
        let family = t.row.kind_type.as_deref().and_then(|ty| spec.family_by_type(FamilyKind::Door, ty));
        let make = door_make(&t.row, family, default_head_in, rule);
        let width = make.width_in * k;
        let (head, sill) = (make.head_in * k, (make.sill_in > 0.0).then_some(make.sill_in * k));
        let placed = place(pl, t.at, width, reach, slot, &lines(t.at, reach), k);
        let Some((placed, a, b, thickness, source)) = placed else {
            report.no_host.push((t.mark.clone(), t.at));
            continue;
        };
        if placed == Placed::OnWall {
            cut_wall(pl, t.at, width, reach, slot, k, head, sill);
        }
        // What is particular to this door first (a basis is kept to its first 240 characters).
        let gave = if make.inferred.is_empty() { String::new() } else { format!(" (the rule gave its {})", make.inferred.join(", ")) };
        let basis = format!("{RULE_ID}: door schedule mark {}, {}{gave}; {}", t.mark, placed.why(), rule.basis);
        let pair = (make.operation == "pair").then(|| ((a + b) * 0.5, b));
        let leaf_b = if pair.is_some() { (a + b) * 0.5 } else { b };
        if pl.leaves.len() >= MAX_OBJECTS {
            break;
        }
        pl.leaves.push(Leaf {
            a,
            b: leaf_b,
            window: false,
            thickness,
            source,
            head: Some(head),
            sill,
            along: None,
            pair,
            frame: Some((a, b)),
            mark: Some(t.mark.chars().take(40).collect()),
            basis: None,
            at_tag: Some(AtTag { placed, operation: make.operation, counter: make.counter, basis }),
        });
        report.built.push((t.mark.clone(), t.at, placed));
    }
    report
}

/// Where a door `width` wide at the tag `at` goes: the wall that holds it there (its ends on the
/// wall, centred at the tag's foot or slid along the wall to stand in it), else the nearest line
/// drawn within `reach` at least half its width long (centred at the tag's foot on it), else at
/// the tag along the nearest wall. Its ends, its frame depth and its source; none with no wall on
/// the level to take a direction and a source from.
fn place(
    pl: &PlanLevel,
    at: Vec2,
    width: f64,
    reach: f64,
    slot: f64,
    segs: &[(Seg, cadaddy_doc::Handle)],
    k: f64,
) -> Option<(Placed, Vec2, Vec2, f64, cadaddy_doc::Handle)> {
    let half = width / 2.0;
    if let Some(h) = host_wall(&pl.walls, at, width, reach, k, slot, &|_, _, _| true)
        && let Some(w) = pl.walls.get(h.wall)
    {
        let len = w.a.dist(w.b);
        let u = (w.b - w.a) * (1.0 / len.max(1e-12));
        let on = w.a + u * h.at;
        return Some((Placed::OnWall, on - u * half, on + u * half, w.thickness, w.source));
    }
    let depth = 2.0 * LEAF_IN * k;
    let line = segs
        .iter()
        .take(MAX_OBJECTS)
        .filter(|(s, _)| s.a.dist(s.b) >= half && s.a.dist(s.b).is_finite())
        .map(|(s, h)| (seg_dist(at, s.a, s.b), s, *h))
        .filter(|(d, ..)| d.is_finite() && *d <= reach)
        .min_by(|x, y| x.0.total_cmp(&y.0));
    if let Some((_, s, h)) = line {
        let len = s.a.dist(s.b);
        let u = (s.b - s.a) * (1.0 / len);
        let foot = s.a + u * u.dot(at - s.a).clamp(0.0, len);
        return Some((Placed::OnLine, foot - u * half, foot + u * half, depth, h));
    }
    let w = pl
        .walls
        .iter()
        .take(MAX_OBJECTS)
        .filter(|w| w.a.dist(w.b) > 1e-9)
        .min_by(|x, y| seg_dist(at, x.a, x.b).total_cmp(&seg_dist(at, y.a, y.b)))?;
    let u = (w.b - w.a) * (1.0 / w.a.dist(w.b));
    Some((Placed::AtTag, at - u * half, at + u * half, depth, w.source))
}

/// Cut the door's opening out of every piece of the wall line that holds it ([`host_wall`]), and
/// take out the slots it spans that were built solid.
#[allow(clippy::too_many_arguments)]
fn cut_wall(pl: &mut PlanLevel, at: Vec2, width: f64, reach: f64, slot: f64, k: f64, head: f64, sill: Option<f64>) {
    let Some(host) = host_wall(&pl.walls, at, width, reach, k, slot, &|_, _, _| true) else { return };
    for (j, s0, s1) in &host.spans {
        if let Some(o) = pl.walls.get_mut(*j) {
            o.cuts.push(Cut { s0: *s0, s1: *s1, window: false, head: Some(head), sill });
        }
    }
    let half = width / 2.0;
    for o in pl.walls.iter_mut().take(MAX_OBJECTS) {
        let t = o.thickness;
        o.extra.retain(|(e0, e1, ..)| {
            let m = (*e0 + *e1) * 0.5 - host.mid;
            !(host.u.perp().dot(m).abs() <= t / 2.0 && host.u.dot(m).abs() < half)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> DoorRule {
        DoorRule::load().expect("the rule data has the door-at-tag rule")
    }

    fn row(mark: &str, w: Option<f64>, h: Option<f64>, ty: &str) -> ScheduleRow {
        ScheduleRow {
            kind: FamilyKind::Door,
            mark: mark.into(),
            width_in: w,
            height_in: h,
            sill_ft: None,
            head_ft: None,
            kind_type: Some(ty.into()).filter(|t: &String| !t.is_empty()),
            frame: None,
            fire_rating: None,
            hardware: None,
        }
    }

    #[test]
    fn the_rule_data_gives_every_number_and_word_list() {
        let r = rule();
        assert!(r.reach_ft > 0.0 && r.connect_ft >= r.reach_ft, "{r:?}");
        assert!(r.passage_min_in >= 78.0 && r.counter_sill_in > 30.0 && r.max_leaf_in >= 36.0 && r.default_width_in > 0.0, "{r:?}");
        for op in ["rolling", "sliding", "folding", "pair"] {
            assert!(r.operation_words.iter().any(|(k, w)| k == op && !w.is_empty()), "{op}: {r:?}");
        }
        assert!(!r.counter_words.is_empty() && r.basis.contains("trade practice"));
        assert!(DoorRule::from_value(&Value::Null).is_none(), "no rule data, nothing built");
    }

    #[test]
    fn a_door_row_says_its_size_sill_and_how_it_opens_by_its_words_else_its_size() {
        let r = rule();
        // A coiling shutter its words name, over a counter (lower than a passage door).
        let m = door_make(&row("X1", Some(126.0), Some(58.0), "ALUM. OVERHEAD ROLLING GATE"), None, 84.0, &r);
        assert_eq!((m.operation, m.counter), ("rolling", true));
        assert_eq!((m.width_in, m.sill_in, m.head_in), (126.0, r.counter_sill_in, r.counter_sill_in + 58.0));
        assert_eq!(m.inferred, vec!["sill"]);
        // The same opening with only a type letter: its size says it (wide and low).
        let m = door_make(&row("X1", Some(126.0), Some(58.0), "type J AL"), None, 84.0, &r);
        assert_eq!((m.operation, m.counter), ("rolling", true));
        assert!(m.inferred.contains(&"operation") && m.inferred.contains(&"sill"));
        // A sill the schedule gives wins over the counter top.
        let mut given = row("X1", Some(96.0), Some(48.0), "COUNTER SHUTTER");
        given.sill_ft = Some(3.0);
        let m = door_make(&given, None, 84.0, &r);
        assert_eq!((m.sill_in, m.head_in, m.operation), (36.0, 84.0, "rolling"));
        // A sliding door its words name, a passage door from the floor.
        let m = door_make(&row("X2", Some(72.0), Some(84.0), "SLIDING GLASS"), None, 84.0, &r);
        assert_eq!((m.operation, m.counter, m.sill_in, m.head_in), ("sliding", false, 0.0, 84.0));
        // Wider than one leaf at a passage height: a pair; one leaf wide: a swing door.
        assert_eq!(door_make(&row("X3", Some(72.0), Some(84.0), "HM"), None, 84.0, &r).operation, "pair");
        assert_eq!(door_make(&row("X4", Some(36.0), Some(84.0), "WD"), None, 84.0, &r).operation, "swing");
        // No size: the rule's width and the level's door head.
        let m = door_make(&row("X5", None, None, ""), None, 80.0, &r);
        assert_eq!((m.width_in, m.head_in, m.sill_in), (r.default_width_in, 80.0, 0.0));
        assert_eq!(m.inferred, vec!["width", "head"]);
    }

    #[test]
    fn a_tag_on_an_enlarged_plan_is_put_onto_its_levels_plan_and_another_levels_tag_is_not_its() {
        let mut sp = BuildingSpec::default();
        sp.schedule.rows = vec![row("D1", Some(36.0), Some(84.0), ""), row("D2", Some(36.0), Some(84.0), "")];
        let tag = |mark: &str, at: [f64; 2], level: Option<&str>| cadaddy_walk3d::OpeningTag {
            mark: mark.into(),
            at,
            level: level.map(str::to_string),
            ..Default::default()
        };
        sp.schedule.tags = vec![
            tag("D1", [5_100.0, 300.0], None),
            tag("D2", [200.0, 200.0], Some("Level 1")),
            tag("D2", [204.0, 200.0], Some("Level 1")),
            tag("D2", [8_200.0, 200.0], None),
            tag("W9", [210.0, 210.0], None),
        ];
        sp.sheets = vec![
            serde_json::from_value(serde_json::json!({
                "number": "X-2", "level": "Level 1", "rect": [5_000.0, 0.0, 6_000.0, 1_000.0],
                "registration": {"reference": "X-1", "method": "tags", "a": 0.5, "b": 0.0, "tx": -2_300.0, "ty": 100.0, "matched": 6},
            }))
            .unwrap(),
        ];
        let level = Level {
            name: "Level 1".into(),
            region: Some([0.0, 0.0, 4_000.0, 3_000.0]),
            ..serde_json::from_value(serde_json::json!({"name": "", "elevation_ft": 0.0, "height_ft": 10.0})).unwrap()
        };
        let tags = level_tags(&sp, &level, 72.0);
        let got: Vec<(String, [f64; 2])> = tags.iter().map(|t| (t.mark.clone(), [t.at.x, t.at.y])).collect();
        // D1 from the enlarged sheet onto the plan; D2 once (two prints 4 units apart); the tag
        // off the level's region (another level's plan) and the window mark are not its doors.
        assert_eq!(got, vec![("D1".to_string(), [250.0, 250.0]), ("D2".to_string(), [200.0, 200.0])]);
    }
}
