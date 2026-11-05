//! What a cable method's takeoff needs besides the cable and its supports (Q6202): what each
//! cable takes where it enters a box or a panel, the boxes cable work sets the devices in instead
//! of the design's (by the devices' plate gang), nail plates where the cable passes framing too
//! near its face, the low-voltage outlets' brackets in its rooms, and an SE cable's connector
//! sizes and panel-end fittings. All by rules data with their cites (`verules.json`
//! `cable_work`); the cable, its staples or straps and their spacing are the atom rules'.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cadaddy_doc::props::{self, Prop, Props};
use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::BuildingSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::mepatoms::{self, Atom, X3};
use super::price::Line;
use super::scope::Item;

/// Design pieces read, boxes looked at, pieces made, at most (hostile models).
const MAX_PIECES: usize = 2_000_000;
const MAX_MADE: usize = 400_000;
/// Devices in one box at most (a plate group larger is split).
const MAX_GANG: usize = 4;

/// One cable's rules.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CableWork {
    entry: BTreeMap<String, Entry>,
    boxes: Option<BoxSwap>,
    guards: Option<Guards>,
    lv_bracket: Option<Bracket>,
    connector_size: Vec<[String; 2]>,
    connector_size_source: String,
    panel_end: Vec<Fitting>,
}

/// What a cable takes entering a `steel` or `nonmetallic` box or `gear`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Entry {
    item: String,
    size: String,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct BoxSwap {
    swap_sizes: Vec<String>,
    by_gang: BTreeMap<String, String>,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Guards {
    item: String,
    size: String,
    edge_min_in: f64,
    stud_depth_in: f64,
    hole_in: f64,
    stud_spacing_in: f64,
    per_plate_crossing: f64,
    reach_in: f64,
    source: String,
    plate_source: String,
    stud_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Bracket {
    box_kinds: Vec<String>,
    item: String,
    size: String,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Fitting {
    kind: String,
    item: String,
    source: String,
}

/// The cable work of one alternative: the design's pieces it replaces beyond its circuits' (the
/// boxes and rings it sets the devices in otherwise), their lines, and what it found.
#[derive(Clone, Debug, Default)]
pub(crate) struct Work {
    pub replaces: Vec<Handle>,
    pub design: Vec<Line>,
    pub summary: Value,
}

fn text(ps: &Props, k: &str) -> String {
    ps.get(k).map(Prop::text).unwrap_or_default()
}

/// A mesh piece's centre (its vertices' extent).
fn centre(d: &Drawing, h: Handle) -> Option<Vec3> {
    let e = d.model.get(h)?;
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in m.vertices.iter().take(10_000) {
        for (i, x) in [v.x, v.y, v.z].into_iter().enumerate() {
            if x.is_finite()
                && let (Some(l), Some(u)) = (lo.get_mut(i), hi.get_mut(i))
            {
                *l = l.min(x);
                *u = u.max(x);
            }
        }
    }
    let c = Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0);
    (c.x.is_finite() && c.y.is_finite() && c.z.is_finite()).then_some(c)
}

fn handles(t: &str) -> Vec<Handle> {
    t.split(',').filter_map(|x| u64::from_str_radix(x.trim(), 16).ok()).map(Handle).take(64).collect()
}

/// A conductor size's order, small to large (`14` … `4/0`, then kcmil).
fn awg_rank(size: &str) -> Option<i64> {
    let s = size.trim().trim_start_matches('#');
    if let Some(n) = s.strip_suffix("/0") {
        return n.parse::<i64>().ok().map(|n| 100 + n);
    }
    let n = s.split_whitespace().next()?.parse::<i64>().ok()?;
    Some(if n >= 250 { 1_000 + n } else { -n })
}

/// A cable's trade size for its connector: the row of its largest conductor (the next larger
/// row when none matches).
fn connector_trade(cw: &CableWork, makeup: &str) -> Option<String> {
    let largest = mepatoms::conductor_parts(makeup).into_iter().filter(|p| !p.2).filter_map(|p| awg_rank(&p.1)).max()?;
    cw.connector_size
        .iter()
        .filter_map(|[s, t]| Some((awg_rank(s)?, t)))
        .filter(|(r, _)| *r >= largest)
        .min_by_key(|(r, _)| *r)
        .map(|(_, t)| t.clone())
}

/// The cable work of `cable` for the alternative's items: its pieces into `atoms`, the design's
/// boxes and rings it replaces.
pub(crate) fn work(
    d: &Drawing,
    sp: &BuildingSpec,
    items: &[&Item],
    cable: &str,
    cw: Option<&CableWork>,
    atoms: &mut Vec<(Atom, String, String, String)>,
) -> Work {
    let Some(cw) = cw else { return Work::default() };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ar = mepatoms::rules();
    let mut summary = serde_json::Map::new();
    let mut sources: BTreeSet<String> = BTreeSet::new();
    let mut made: Vec<(Atom, String, String, String)> = Vec::new();

    // -- An SE cable's connectors by its size, each with its panel-end fittings. --
    if !cw.connector_size.is_empty() || !cw.panel_end.is_empty() {
        let makeups: HashMap<&str, String> = items
            .iter()
            .filter_map(|it| it.feeder.and_then(|i| sp.mep.feeders.get(i)).map(|f| (it.id.as_str(), super::scope::feeder_makeup(f))))
            .collect();
        let (mut sized, mut ends) = (0usize, 0usize);
        for (a, level, circuit, of) in atoms.iter_mut().filter(|x| x.0.kind == "connector") {
            if let Some(t) = makeups.get(of.as_str()).and_then(|m| connector_trade(cw, m)) {
                a.size = t;
                a.extra.push(("sized_by", cw.connector_size_source.clone(), cw.connector_size_source.clone()));
                sized += 1;
            }
            for f in &cw.panel_end {
                let kind = match f.kind.as_str() {
                    "locknut" => "locknut",
                    "bushing" => "bushing",
                    _ => "fitting",
                };
                made.push((
                    Atom::fitting(kind, f.item.clone(), &a.ty, &a.size, a.a, a.b - a.a, a.body_in * 1.4, &f.source),
                    level.clone(),
                    circuit.clone(),
                    of.clone(),
                ));
                sources.insert(f.source.clone());
                ends += 1;
            }
        }
        if sized > 0 {
            sources.insert(cw.connector_size_source.clone());
        }
        summary.insert("connectors_sized".into(), json!(sized));
        summary.insert("panel_end_fittings".into(), json!(ends));
    }

    // -- Where each cable enters a box or a panel: the design's raceway connectors (one per
    // raceway end), a homerun's one per circuit's cable. --
    let mut swapped_boxes: Vec<Handle> = Vec::new();
    let mut replaces: Vec<Handle> = Vec::new();
    let mut design: BTreeMap<(String, String, String), f64> = BTreeMap::new();
    let design_set: HashSet<Handle> = items.iter().flat_map(|it| it.design.iter().copied()).take(MAX_PIECES).collect();
    let branch: Vec<&&Item> = items.iter().filter(|it| it.kind == "branch").collect();
    let mut rooms: HashSet<(String, String)> = HashSet::new();
    for it in &branch {
        for r in &it.area.rooms {
            rooms.insert((it.level.to_ascii_lowercase(), r.to_ascii_uppercase()));
        }
    }
    if !cw.entry.is_empty() || cw.boxes.is_some() {
        // (where, what it enters: a box or gear, cables, its level, circuit, item).
        let mut landings: Vec<(Vec3, Vec3, Option<Handle>, usize, String, String, String)> = Vec::new();
        for it in &branch {
            let sets = it.members.len().max(1);
            for h in it.design.iter().take(MAX_PIECES) {
                let Some(e) = d.model.get(*h) else { continue };
                let ps = props::read(&e.common.xdata);
                if text(&ps, mepatoms::ATOM_KEY) != "connector" {
                    continue;
                }
                let mut into: Option<Option<Handle>> = None;
                for c in handles(&text(&ps, "connects")) {
                    if design_set.contains(&c) {
                        continue;
                    }
                    let Some(n) = d.model.get(c) else { continue };
                    match text(&props::read(&n.common.xdata), mepatoms::ATOM_KEY).as_str() {
                        "box" => {
                            into = Some(Some(c));
                            break;
                        }
                        // Not a piece: the gear (or the equipment) it ends at.
                        "" => into = into.or(Some(None)),
                        _ => {}
                    }
                }
                let Some(into) = into else { continue };
                let at = centre(d, *h).unwrap_or(Vec3::new(0.0, 0.0, 0.0));
                let dir = into.and_then(|b| centre(d, b)).map(|b| b - at).filter(|v| v.len() > 1e-9).unwrap_or(X3);
                landings.push((
                    at,
                    dir,
                    into,
                    sets,
                    it.level.clone(),
                    if it.members.is_empty() { it.label.clone() } else { String::new() },
                    it.id.clone(),
                ));
            }
        }
        // The device boxes it sets in its own box: their plate groups.
        let mut new_box: HashMap<Handle, &'static str> = HashMap::new();
        if let Some(bs) = &cw.boxes {
            let mut groups: BTreeMap<(String, String), Vec<(Handle, Props, String)>> = BTreeMap::new();
            let mut seen: HashSet<Handle> = HashSet::new();
            for b in landings.iter().filter_map(|l| l.2) {
                if !seen.insert(b) {
                    continue;
                }
                let Some(e) = d.model.get(b) else { continue };
                let ps = props::read(&e.common.xdata);
                let size = text(&ps, "size");
                if !bs.swap_sizes.iter().any(|s| s.eq_ignore_ascii_case(size.trim())) {
                    continue;
                }
                let Some(dev) = handles(&text(&ps, "box_of")).first().and_then(|h| d.model.get(*h)) else { continue };
                let dps = props::read(&dev.common.xdata);
                let Some(bk) = super::super::mepatoms::build::box_kind(&text(&dps, "kind"), &dev.common.layer) else { continue };
                let Some((cb, _)) = ar.box_for(bk, true) else { continue };
                if cb.item == text(&ps, "item") {
                    continue;
                }
                let plate = text(&dps, "plate_group");
                let level = super::super::bspec::made_level(e).unwrap_or_default();
                let key = (level.to_ascii_lowercase(), if plate.is_empty() { b.hex() } else { plate });
                groups.entry(key).or_default().push((b, ps, level));
            }
            let mut by_gang: BTreeMap<String, usize> = BTreeMap::new();
            for members in groups.values() {
                for chunk in members.chunks(MAX_GANG) {
                    let gang = chunk.len();
                    let id = bs.by_gang.get(&gang.to_string());
                    let Some(br) = id.and_then(|i| ar.boxes.get(i)) else { continue };
                    let Some((_, ps, level)) = chunk.first() else { continue };
                    let pts: Vec<Vec3> = chunk.iter().filter_map(|(h, ..)| centre(d, *h)).collect();
                    let n = pts.len().max(1) as f64;
                    let at = pts.iter().fold(Vec3::new(0.0, 0.0, 0.0), |a, p| a + *p * (1.0 / n));
                    let mut a =
                        Atom::fitting("box", br.item.clone(), &br.size, &br.size, at, X3, br.dims_in[0].max(br.dims_in[1]).max(1.0), &bs.source);
                    a.extra.push(("cubic_in", format!("{}", br.cubic_in), br.source.clone()));
                    a.extra.push(("material", br.material.clone(), br.source.clone()));
                    a.extra.push(("gang", gang.to_string(), "computed: the devices of its plate group".into()));
                    a.extra.push(("in_place_of", chunk.iter().map(|(h, ..)| h.hex()).collect::<Vec<_>>().join(","), "computed".into()));
                    made.push((a, level.clone(), text(ps, "circuit"), String::new()));
                    *by_gang.entry(br.size.clone()).or_default() += 1;
                    for (h, ps, _) in chunk {
                        swapped_boxes.push(*h);
                        new_box.insert(*h, "nonmetallic");
                        *design.entry(("box".into(), text(ps, "item"), text(ps, "size"))).or_default() += 1.0;
                        // Its ring goes with it.
                        for r in handles(&text(ps, "connects")) {
                            let Some(re) = d.model.get(r) else { continue };
                            let rps = props::read(&re.common.xdata);
                            if text(&rps, mepatoms::ATOM_KEY) == "ring" {
                                replaces.push(r);
                                *design.entry(("ring".into(), text(&rps, "item"), text(&rps, "size"))).or_default() += 1.0;
                            }
                        }
                    }
                    sources.insert(bs.source.clone());
                    sources.insert(br.source.clone());
                }
            }
            summary.insert("boxes_replaced".into(), json!(swapped_boxes.len()));
            summary.insert("new_boxes".into(), json!(by_gang));
        }
        replaces.extend(swapped_boxes.iter().copied());
        // What each entry takes.
        let mut entries: BTreeMap<&str, usize> = BTreeMap::new();
        for (at, dir, into, sets, level, circuit, of) in &landings {
            let what = match into {
                // A box of the design's circuits (a junction) goes with them.
                Some(b) if design_set.contains(b) => continue,
                Some(b) => new_box.get(b).copied().unwrap_or_else(|| {
                    let m = d.model.get(*b).map(|e| text(&props::read(&e.common.xdata), "material")).unwrap_or_default();
                    if m.eq_ignore_ascii_case("nonmetallic") { "nonmetallic" } else { "steel" }
                }),
                None => "gear",
            };
            *entries.entry(what).or_default() += sets;
            let Some(r) = cw.entry.get(what) else { continue };
            sources.insert(r.source.clone());
            if r.item.is_empty() {
                continue;
            }
            for _ in 0..(*sets).min(12) {
                if made.len() >= MAX_MADE {
                    break;
                }
                made.push((
                    Atom::fitting("connector", r.item.clone(), cable, &r.size, *at, *dir, 0.75, &r.source),
                    level.clone(),
                    circuit.clone(),
                    of.clone(),
                ));
            }
        }
        summary.insert("cable_entries".into(), json!(entries));
    }

    // -- Nail plates: where a cable passes a wall's top or bottom plate (cables through one hole
    // share it), and at each stud where the bored hole leaves too little to the face. --
    if let Some(g) = &cw.guards {
        let mut at_plate: HashSet<(String, i64, i64, bool)> = HashSet::new();
        let (mut plates, mut studs) = (0usize, 0usize);
        let edge = (g.stud_depth_in - g.hole_in) / 2.0;
        let stud_plates = edge.is_finite() && edge < g.edge_min_in && g.stud_spacing_in > 0.5;
        let reach = g.reach_in.max(0.0) / 12.0 * upf;
        let cell = 3.0 / 12.0 * upf;
        for (a, level, circuit, of) in atoms.iter().filter(|x| x.0.kind == "cable") {
            if made.len() >= MAX_MADE {
                break;
            }
            let Some(l) = sp.level(level) else { continue };
            let floor = l.elevation_ft * upf;
            let top = floor + if l.ceiling_ft > 0.0 { l.ceiling_ft } else { l.height_ft } * upf;
            for w in a.path.windows(2).take(10_000) {
                let [p, q] = w else { continue };
                let v = *q - *p;
                let len = v.len();
                if len < 1e-9 {
                    continue;
                }
                let (lo, hi) = (p.z.min(q.z), p.z.max(q.z));
                if v.z.abs() > 0.9 * len {
                    // Up or down a wall: through its top plate, or its bottom plate.
                    for (z, bottom) in [(top, false), (floor, true)] {
                        let crosses = if bottom { lo < z - 1e-6 && hi > z + reach } else { lo < z - reach && hi >= z - reach };
                        if !crosses {
                            continue;
                        }
                        let key = (level.to_ascii_lowercase(), (p.x / cell).round() as i64, (p.y / cell).round() as i64, bottom);
                        if !at_plate.insert(key) {
                            continue;
                        }
                        for _ in 0..(g.per_plate_crossing.max(0.0).round() as usize).min(4) {
                            let mut np =
                                Atom::fitting("nail_plate", g.item.clone(), &g.size, &g.size, Vec3::new(p.x, p.y, z), X3, 1.5, &g.plate_source);
                            np.extra.push(("at", if bottom { "bottom plate" } else { "top plate" }.into(), "computed".into()));
                            made.push((np, level.clone(), circuit.clone(), of.clone()));
                            plates += 1;
                        }
                    }
                } else if stud_plates && hi < top - reach {
                    // Along a wall through its studs.
                    let n = ((len / upf * 12.0) / g.stud_spacing_in).floor().clamp(0.0, 1_000.0) as usize;
                    for k in 1..=n {
                        let at = *p + v * (k as f64 / (n + 1) as f64);
                        made.push((
                            Atom::fitting("nail_plate", g.item.clone(), &g.size, &g.size, at, v, 1.5, &g.source),
                            level.clone(),
                            circuit.clone(),
                            of.clone(),
                        ));
                        studs += 1;
                    }
                }
            }
        }
        sources.insert(g.source.clone());
        sources.insert(if stud_plates { g.source.clone() } else { g.stud_source.clone() });
        if plates > 0 {
            sources.insert(g.plate_source.clone());
        }
        summary.insert("nail_plates".into(), json!({"top_bottom_plates": plates, "studs": studs, "stud_hole_edge_in": edge}));
    }

    // -- A low-voltage outlet's box and ring in the scenario's rooms: a bracket. --
    if let Some(br) = &cw.lv_bracket
        && !rooms.is_empty()
    {
        let mut n = 0usize;
        let taken: HashSet<Handle> = replaces.iter().copied().collect();
        for e in d.model.iter().take(MAX_PIECES) {
            if made.len() >= MAX_MADE {
                break;
            }
            let ps = props::read(&e.common.xdata);
            if text(&ps, mepatoms::ATOM_KEY) != "box" || taken.contains(&e.handle) || design_set.contains(&e.handle) {
                continue;
            }
            let Some(dev) = handles(&text(&ps, "box_of")).first().and_then(|h| d.model.get(*h)) else { continue };
            let dps = props::read(&dev.common.xdata);
            let Some(bk) = super::super::mepatoms::build::box_kind(&text(&dps, "kind"), &dev.common.layer) else { continue };
            if !br.box_kinds.iter().any(|k| k.eq_ignore_ascii_case(bk)) {
                continue;
            }
            let level = super::super::bspec::made_level(e).unwrap_or_default();
            let room = Some(text(&ps, "room")).filter(|r| !r.is_empty()).unwrap_or_else(|| text(&dps, "room"));
            if !rooms.contains(&(level.to_ascii_lowercase(), room.to_ascii_uppercase())) {
                continue;
            }
            let at = centre(d, e.handle).unwrap_or(Vec3::new(0.0, 0.0, 0.0));
            let mut a = Atom::fitting("bracket", br.item.clone(), &br.size, &br.size, at, X3, 2.0, &br.source);
            a.extra.push(("in_place_of", e.handle.hex(), "computed".into()));
            made.push((a, level, String::new(), String::new()));
            replaces.push(e.handle);
            *design.entry(("box".into(), text(&ps, "item"), text(&ps, "size"))).or_default() += 1.0;
            for r in handles(&text(&ps, "connects")) {
                let Some(re) = d.model.get(r) else { continue };
                let rps = props::read(&re.common.xdata);
                if text(&rps, mepatoms::ATOM_KEY) == "ring" {
                    replaces.push(r);
                    *design.entry(("ring".into(), text(&rps, "item"), text(&rps, "size"))).or_default() += 1.0;
                }
            }
            n += 1;
        }
        if n > 0 {
            sources.insert(br.source.clone());
        }
        summary.insert("lv_brackets".into(), json!(n));
    }

    // -- The cable's own supports, as the atom rules make them. --
    if let Some(cr) = ar.cables.get(cable) {
        let straps = atoms.iter().filter(|x| x.0.kind == "strap").count();
        summary.insert("supports".into(), json!({"item": cr.strap, "count": straps, "source": cr.strap_source}));
    }
    atoms.extend(made);
    replaces.sort_unstable();
    replaces.dedup();
    summary.insert("sources".into(), json!(sources.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>()));
    Work {
        replaces,
        design: design.into_iter().map(|((kind, item, size), qty)| Line { kind, item, size, unit: "ea".into(), qty }).collect(),
        summary: Value::Object(summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_se_cables_connector_is_sized_by_its_largest_conductor() {
        let rules = super::super::rules();
        let ser = rules.cable_work.get("SER").expect("SER rules");
        assert_eq!(connector_trade(ser, "3#2/0 AL + 1#4G").as_deref(), Some("1-1/4\""));
        assert_eq!(connector_trade(ser, "3#4/0 + 1#2/0 + 1#4G").as_deref(), Some("1-1/2\""));
        assert_eq!(connector_trade(ser, "3#250 + 1#4G").as_deref(), Some("2\""));
        // Between two rows: the next larger; past the table: none (not guessed).
        assert_eq!(connector_trade(ser, "3#8 + 1#10G").as_deref(), Some("3/4\""));
        assert_eq!(connector_trade(ser, "3#750 + 1#2G"), None);
        assert_eq!(connector_trade(ser, "DRYER"), None);
        assert!(awg_rank("#12") < awg_rank("#4") && awg_rank("#1") < awg_rank("1/0") && awg_rank("4/0") < awg_rank("250 kcmil"));
    }
}
