//! Doors, windows and firestopping as pieces.
//!
//! - Each door: its frame (hollow metal in a steel stud or masonry wall, wood in a wood wall,
//!   unless the hardware schedule names it) and its hardware set (the schedule's set for its
//!   size, its items per door listed), the door's and each window's size and area on the
//!   element (doors and windows are counted as themselves).
//! - Firestopping: a system at each place a pipe, duct, conduit or cable piece passes through a
//!   fire-rated wall (the wall type's rating), and where a riser passes through a floor.

use super::*;

/// Most doors, windows, penetrants and rated walls looked at.
const MAX_OPENINGS: usize = 100_000;
const MAX_PENETRANTS: usize = 1_000_000;

/// A door's or window's opening from its mesh: centre, width (its longer plan side), height,
/// bottom, the direction along its wall.
fn opening_of(e: &cadaddy_doc::Entity) -> Option<(Vec2, f64, f64, f64, Vec2)> {
    let (lo, hi) = super::super::fieldview::mesh_box(e)?;
    let (dx, dy) = (hi[0] - lo[0], hi[1] - lo[1]);
    let c = Vec2::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let along = if dx >= dy { Vec2::new(1.0, 0.0) } else { Vec2::new(0.0, 1.0) };
    Some((c, dx.max(dy), hi[2] - lo[2], lo[2], along))
}

/// A size as a door schedule writes it: `3-0 X 7-0`.
fn ft_in(v_ft: f64) -> String {
    let total = (v_ft * 12.0).round() as i64;
    format!("{}-{}", total / 12, total % 12)
}

/// A hardware set's door size (`3-0 X 7-0 X 1 3/4`): width and height, feet.
fn set_size(text: &str) -> Option<(f64, f64)> {
    let u = text.to_ascii_uppercase();
    let mut it = u.split('X').map(str::trim);
    let parse = |s: &str| {
        let (f, i) = s.split_once('-')?;
        Some(f.trim().parse::<f64>().ok()? + i.split_whitespace().next()?.parse::<f64>().ok()? / 12.0)
    };
    Some((parse(it.next()?)?, parse(it.next()?)?))
}

/// The kinds a door hardware piece can be: the rules' `openings.hardware_kinds` name them by
/// these; a name not here is a `hardware_item`.
const HW_KINDS: &[&str] = &[
    "continuous_hinge",
    "pivot",
    "hinge",
    "exit_trim",
    "exit_device",
    "flush_bolt",
    "deadbolt",
    "electric_strike",
    "strike",
    "card_reader",
    "lockset",
    "coordinator",
    "closer",
    "holder",
    "stop",
    "viewer",
    "door_bottom",
    "gasketing",
    "guard",
    "threshold",
    "silencer",
    "kick_plate",
    "push_pull",
    "astragal",
    "hardware_item",
];

/// A leaf's thickness, inches (where a piece sits on a face).
const LEAF_IN: f64 = 1.75;

/// A kind of door hardware the rules know, and where trade practice puts it on the door.
#[derive(Clone, Debug)]
pub(super) struct HwKind {
    pub kind: &'static str,
    /// How it is placed (`hinges`, `latch`, `head`, `behind_swing`, …).
    pub place: String,
    /// Its centre's height above the floor, inches.
    pub z_in: Option<f64>,
    /// From the leaf's latch edge, inches.
    pub backset_in: f64,
    /// Along the leaf, across it, high, inches (0: the opening's).
    pub size_in: [f64; 3],
}

/// The kind of a hardware schedule item: the first of the rules' kinds whose words its item type
/// names (else the whole row's words).
pub(super) fn hardware_kind(r: &Rules, ty: &str, row: &str) -> Option<HwKind> {
    let kinds = r.list("openings.hardware_kinds");
    for text in [ty, row] {
        let u = text.to_ascii_uppercase();
        if u.trim().is_empty() {
            continue;
        }
        for k in kinds.iter().take(64) {
            let words = k.get("words").and_then(Value::as_array).cloned().unwrap_or_default();
            let hit = words.iter().filter_map(Value::as_str).take(64).any(|w| !w.trim().is_empty() && u.contains(&w.to_ascii_uppercase()));
            if !hit {
                continue;
            }
            let name = k.get("kind").and_then(Value::as_str).unwrap_or_default();
            let kind = HW_KINDS.iter().copied().find(|x| x.eq_ignore_ascii_case(name.trim())).unwrap_or("hardware_item");
            let num = |key: &str| k.get(key).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= 240.0);
            let size: Vec<f64> = k
                .get("size_in")
                .and_then(Value::as_array)
                .map(|a| a.iter().take(3).map(|v| v.as_f64().filter(|x| x.is_finite() && *x >= 0.0 && *x <= 240.0).unwrap_or(0.0)).collect())
                .unwrap_or_default();
            return Some(HwKind {
                kind,
                place: k.get("place").and_then(Value::as_str).unwrap_or("latch").to_string(),
                z_in: num("z_in"),
                backset_in: num("backset_in").unwrap_or(2.75),
                size_in: [size.first().copied().unwrap_or(0.0), size.get(1).copied().unwrap_or(0.0), size.get(2).copied().unwrap_or(0.0)],
            });
        }
    }
    None
}

/// A door's leaf as hardware is placed on it: the hinge end and the latch end of the shut leaf on
/// the wall line, the way it swings, its bottom and height, the wall's thickness (sheet units).
struct Leaf {
    hinge: Vec2,
    latch: Vec2,
    /// From the hinge end toward the latch end.
    inward: Vec2,
    /// The side it swings to.
    swing: Vec2,
    width: f64,
    bottom: f64,
    height: f64,
    t: f64,
    /// Sheet units per inch.
    k: f64,
}

/// The boxes of the `n` pieces of one item of kind `hk` on the leaf (`n` past one stacks or
/// spreads them the way the kind's place does).
fn hardware_blocks(lf: &Leaf, hk: &HwKind, r: &Rules, n: usize) -> Vec<Vec<Block>> {
    let k = lf.k;
    let [sa, sx, sz] = hk.size_in;
    let top = lf.bottom + lf.height;
    let at_z = |z_in: f64| lf.bottom + z_in * k;
    // Proud of a face: on the swing side (`1.0`) or the other (`-1.0`).
    let face = |side: f64, across: f64| lf.swing * (side * (LEAF_IN / 2.0 + across / 2.0) * k);
    let mut out = Vec::new();
    for i in 0..n {
        let fi = i as f64;
        let b = match hk.place.as_str() {
            "hinges" => {
                let hs = r.num("openings.hinge_size_in", 4.5).clamp(1.0, 12.0) * k;
                let hi = top - r.num("openings.hinge_top_in", 5.0).clamp(0.0, 48.0) * k - hs / 2.0;
                let lo = lf.bottom + r.num("openings.hinge_bottom_in", 10.0).clamp(0.0, 48.0) * k + hs / 2.0;
                let zc = if n <= 1 || hi <= lo { hi } else { hi - (hi - lo) * fi / (n - 1) as f64 };
                vec![Block::new(lf.hinge, lf.hinge + lf.inward * k, 2.0 * k, zc - hs / 2.0, zc + hs / 2.0)]
            }
            "hinge_edge" => vec![Block::new(lf.hinge, lf.hinge + lf.inward * k, 2.0 * k, lf.bottom + 0.5 * k, top - 0.5 * k)],
            "across" => {
                let z = at_z(hk.z_in.unwrap_or(38.0)) + fi * (sz + 1.0) * k;
                let o = face(-1.0, sx);
                vec![Block::new(
                    lf.hinge + lf.inward * (3.0 * k) + o,
                    lf.latch - lf.inward * (2.0 * k) + o,
                    sx.max(0.5) * k,
                    z - sz * k / 2.0,
                    z + sz * k / 2.0,
                )]
            }
            "head" => {
                let o = face(1.0, sx);
                let a = lf.hinge + lf.inward * (2.0 * k) + o;
                let len = if sa > 0.0 { (sa * k).min(lf.width - 3.0 * k) } else { lf.width - 3.0 * k };
                let z1 = top - (1.0 + fi * (sz + 0.5)) * k;
                vec![Block::new(a, a + lf.inward * len.max(k), sx.max(0.5) * k, z1 - sz.max(0.5) * k, z1)]
            }
            "behind_swing" => {
                let p = lf.hinge + lf.swing * (lf.width - 2.0 * k) + lf.inward * (fi * (sa + 1.0) * k);
                let z0 = at_z((hk.z_in.unwrap_or(2.0) - sz / 2.0).max(0.0));
                vec![Block::new(
                    p - lf.inward * (sa.max(0.5) * k / 2.0),
                    p + lf.inward * (sa.max(0.5) * k / 2.0),
                    sx.max(0.5) * k,
                    z0,
                    z0 + sz.max(0.5) * k,
                )]
            }
            "leaf" => {
                let p = lf.hinge + lf.inward * (lf.width / 2.0);
                let z = at_z(hk.z_in.unwrap_or(58.0)) - fi * (sz + 1.0) * k;
                vec![Block::new(
                    p - lf.inward * (sa.max(0.5) * k / 2.0),
                    p + lf.inward * (sa.max(0.5) * k / 2.0),
                    sx.max(0.5) * k,
                    z - sz.max(0.5) * k / 2.0,
                    z + sz.max(0.5) * k / 2.0,
                )]
            }
            "perimeter" => {
                let s = (0.5 + fi * 0.25) * k;
                vec![
                    Block::new(lf.hinge, lf.hinge + lf.inward * s, LEAF_IN * k, lf.bottom, top),
                    Block::new(lf.latch - lf.inward * s, lf.latch, LEAF_IN * k, lf.bottom, top),
                    Block::new(lf.hinge, lf.latch, LEAF_IN * k, top - s, top),
                ]
            }
            "bottom" => {
                let z0 = at_z(hk.z_in.unwrap_or(0.0)) + fi * sz.max(0.5) * k;
                let o = if hk.z_in.is_some() { face(-1.0, sx) } else { Vec2::new(0.0, 0.0) };
                vec![Block::new(lf.hinge + lf.inward * k + o, lf.latch - lf.inward * k + o, sx.max(0.25) * k, z0, z0 + sz.max(0.25) * k)]
            }
            "threshold" => vec![Block::new(lf.hinge, lf.latch, lf.t.max(sx * k), lf.bottom, lf.bottom + sz.max(0.25) * k)],
            "frame_latch" => {
                let p = lf.latch + lf.inward * (0.5 * k);
                let z = match hk.z_in {
                    Some(z) if n <= 1 => at_z(z),
                    _ => lf.bottom + lf.height * (fi + 1.0) / (n as f64 + 1.0),
                };
                let h = sz.max(0.5) * k;
                vec![Block::new(
                    p - lf.inward * (sa.max(0.5) * k / 2.0),
                    p + lf.inward * (sa.max(0.5) * k / 2.0),
                    sx.max(0.5) * k,
                    z - h / 2.0,
                    z + h / 2.0,
                )]
            }
            "meeting_edge" => {
                let (z0, z1) = if sz <= 0.0 {
                    (lf.bottom, top)
                } else if i % 2 == 0 {
                    (top - sz * k, top)
                } else {
                    (lf.bottom, lf.bottom + sz * k)
                };
                vec![Block::new(lf.latch - lf.inward * (sa.max(0.5) * k), lf.latch, sx.max(0.5) * k, z0, z1)]
            }
            "wall_latch" => {
                let p = lf.latch + lf.inward * ((6.0 + sa / 2.0) * k) - lf.swing * (lf.t / 2.0 + sx * k / 2.0);
                let z = at_z(hk.z_in.unwrap_or(42.0)) + fi * (sz + 1.0) * k;
                vec![Block::new(
                    p - lf.inward * (sa.max(0.5) * k / 2.0),
                    p + lf.inward * (sa.max(0.5) * k / 2.0),
                    sx.max(0.5) * k,
                    z - sz.max(0.5) * k / 2.0,
                    z + sz.max(0.5) * k / 2.0,
                )]
            }
            // `latch` and any other: on the leaf by its latch edge.
            _ => {
                let p = lf.latch - lf.inward * (hk.backset_in * k);
                let z = at_z(hk.z_in.unwrap_or(38.0)) + fi * (sz.max(0.5) + 1.0) * k;
                vec![Block::new(
                    p - lf.inward * (sa.max(0.5) * k / 2.0),
                    p + lf.inward * (sa.max(0.5) * k / 2.0),
                    sx.max(0.5) * k,
                    z - sz.max(0.5) * k / 2.0,
                    z + sz.max(0.5) * k / 2.0,
                )]
            }
        };
        out.push(b);
    }
    out
}

/// A door's hand (`LH`/`RH`) from its swing: seen from the side it swings away from, hinges on
/// the left are left-handed.
fn hand_of(lf: &Leaf) -> &'static str {
    let right = Vec2::new(lf.swing.y, -lf.swing.x);
    if (lf.hinge - lf.latch).dot(right) < 0.0 { "LH" } else { "RH" }
}

pub(super) fn lay_out(d: &mut Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, want: &dyn Fn(&str) -> bool) {
    let upf = ctx.upf;
    let k = upf / 12.0;
    if want("openings") {
        doors_and_windows(d, sp, r, ctx, batch, upf, k);
    }
    if want("firestopping") {
        firestops(d, sp, r, batch, upf, k);
    }
}

#[allow(clippy::too_many_arguments)]
fn doors_and_windows(d: &mut Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, upf: f64, k: f64) {
    // The walls by level, to know a door's wall and so its frame.
    let walls: Vec<super::walls::Wall> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall"))
        .filter_map(|e| super::walls::read_wall(e))
        .take(200_000)
        .collect();
    let sets: Vec<(&Fact, Option<(f64, f64)>)> = r.facts_of("openings.hardware_set").map(|f| (f, set_size(&f.quote))).collect();
    // Each set's openings left to hand out.
    let mut left: Vec<f64> = sets.iter().map(|(f, _)| f.number.unwrap_or(0.0)).collect();
    let mut updates: Vec<(Handle, Vec<(&'static str, String, Source)>)> = Vec::new();
    let els: Vec<(Handle, String, String)> = d
        .model
        .iter()
        .filter_map(|e| {
            let part = super::super::wallgen::made_as(e)?;
            matches!(part.as_str(), "door" | "window").then(|| (e.handle, part, super::super::bspec::made_level(e).unwrap_or_default()))
        })
        .take(MAX_OPENINGS)
        .collect();
    for (h, part, level) in els {
        let Some(e) = d.model.get(h) else { continue };
        let Some((c0, width0, height, bottom, along0)) = opening_of(e) else { continue };
        // Its wall: the one whose line it sits on; its width along that wall (an open leaf
        // stands out across it).
        let wall = walls.iter().filter(|w| w.level.eq_ignore_ascii_case(&level)).find(|w| {
            let s = (c0 - w.o).dot(w.u);
            s >= w.s0 - 1e-6 && s <= w.s1 + 1e-6 && (c0 - w.o).dot(w.n).abs() < w.t.max(width0)
        });
        let (c, width, along) = match (wall, &e.kind) {
            (Some(w), EntityKind::Mesh(m)) => {
                // The frame and leaf in the wall's thickness: their extent along it.
                let inside: Vec<f64> = m
                    .vertices
                    .iter()
                    .filter(|v| (Vec2::new(v.x, v.y) - w.o).dot(w.n).abs() <= w.t)
                    .map(|v| (Vec2::new(v.x, v.y) - w.o).dot(w.u))
                    .collect();
                let (a, b) = inside.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
                if b > a {
                    let mid = (a + b) / 2.0;
                    (w.o + w.u * mid + w.n * (c0 - w.o).dot(w.n), b - a, w.u)
                } else {
                    (c0, width0, along0)
                }
            }
            _ => (c0, width0, along0),
        };
        // A built door's opening (its jambs on the wall's line) is where its frame and leaf are:
        // its mesh's box also holds the leaf standing open across the wall.
        let (c, width, along) = match super::super::wallgen::door_opening(e).filter(|_| part == "door") {
            Some([a, b]) if a.dist(b) > 1e-6 => (Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0), a.dist(b), (b - a) * (1.0 / a.dist(b))),
            _ => (c, width, along),
        };
        let (wf, hf) = (width / upf, height / upf);
        let size = format!("{} x {}", ft_in(wf), ft_in(hf));
        let room = ctx.room_at(&level, c).map(Ctx::room_label).unwrap_or_default();
        let mut props: Vec<(&'static str, String, Source)> = vec![
            ("size", size.clone(), Source::computed()),
            ("area_sf", num_text(wf * hf), Source::computed()),
            ("width_ft", num_text(wf), Source::computed()),
            ("height_ft", num_text(hf), Source::computed()),
        ];
        if part == "window" {
            props.push(("item", r.text("openings.window_item"), r.source("openings.window_item")));
            updates.push((h, props));
            continue;
        }
        let wt = wall.and_then(|w| sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(&w.wall_type)));
        let wood_wall = wt.and_then(|t| t.assembly.as_ref()).is_some_and(|a| {
            a.layers.iter().filter(|l| l.category == "framing").any(|l| {
                let u = l.material.to_ascii_uppercase();
                r.words("framing.wood_words").iter().any(|w| u.contains(w.as_str()))
                    && !r.words("framing.steel_words").iter().any(|w| u.contains(w.as_str()))
            })
        });
        // Its hardware set: the door list's for its number; else one of the schedule's of its
        // size with openings left, else the first with openings left.
        let number = props::read(&e.common.xdata).get("number").map(Prop::text).unwrap_or_default();
        let listed = (!number.trim().is_empty())
            .then(|| r.facts_of("openings.door_set").find(|f| f.scope.eq_ignore_ascii_case(number.trim())))
            .flatten()
            .and_then(|f| f.value.rsplit("set ").next().map(str::to_string))
            .and_then(|set| {
                // The set's group of the door's size, else its first.
                let of_set = |f: &Fact| f.scope.split('|').next().is_some_and(|id| id.eq_ignore_ascii_case(&set));
                let fits = |sz: &Option<(f64, f64)>| sz.is_some_and(|(w, hh)| (w - wf).abs() < 0.35 && (hh - hf).abs() < 0.6);
                sets.iter()
                    .enumerate()
                    .find(|(_, (f, sz))| of_set(f) && fits(sz))
                    .or_else(|| sets.iter().enumerate().find(|(_, (f, _))| of_set(f)))
                    .map(|(i, x)| (i, x.0))
            });
        let by_list = listed.is_some();
        let pick = listed.or_else(|| {
            sets.iter()
                .enumerate()
                .filter(|(i, _)| left.get(*i).is_some_and(|n| *n >= 1.0))
                .find(|(_, (_, sz))| sz.is_some_and(|(w, hh)| (w - wf).abs() < 0.35 && (hh - hf).abs() < 0.6))
                .or_else(|| sets.iter().enumerate().find(|(i, _)| left.get(*i).is_some_and(|n| *n >= 1.0)))
                .map(|(i, x)| (i, x.0))
        });
        if let Some((i, _)) = pick
            && let Some(n) = left.get_mut(i)
        {
            *n -= 1.0;
        }
        // The set's door and frame (`… WOOD DOOR X HM FRAME`): the part that names a frame.
        let named_frame = pick
            .map(|(_, f)| f.quote.to_ascii_uppercase())
            .and_then(|q| q.split(" X ").find(|x| x.contains("FRAME")).map(|x| x.split_whitespace().take(4).collect::<Vec<_>>().join(" ")));
        let (frame, f_src) = match named_frame {
            Some(fr) => (fr.to_ascii_lowercase(), pick.map_or_else(|| r.source("openings.frame_source"), |(_, f)| f.source_of())),
            None => (if wood_wall { r.text("openings.frame_wood") } else { r.text("openings.frame_steel") }, r.source("openings.frame_steel")),
        };
        let t = wall.map_or(6.0 * k, |w| w.t);
        let mut fp = Piece::new("openings", "frame", frame.clone(), &level, f_src);
        fp.host = Some(h);
        fp.room = room.clone();
        fp.size = size.clone();
        fp.length_ft = 2.0 * hf + wf;
        let fw = 2.0 * k;
        let half = along * (width / 2.0);
        fp.blocks.push(Block::new(c - half, c - half + along * fw, t + 1.5 * k, bottom, bottom + height));
        fp.blocks.push(Block::new(c + half - along * fw, c + half, t + 1.5 * k, bottom, bottom + height));
        fp.blocks.push(Block::new(c - half, c + half, t + 1.5 * k, bottom + height - fw, bottom + height));
        let fi = batch.push(fp);
        let (item, h_src, set_id) = match pick {
            Some((_, f)) => {
                let id = f.scope.split('|').next().unwrap_or_default().to_string();
                (format!("{} {id}", r.text("openings.hardware_item")), f.source_of(), id)
            }
            None => (
                format!("{} (no hardware schedule read)", r.text("openings.hardware_item")),
                Source::from_doc(SourceKind::Default, "no door hardware schedule in the documents read"),
                String::new(),
            ),
        };
        let mut hp = Piece::new("openings", "hardware_set", item, &level, h_src);
        hp.host = Some(h);
        hp.room = room.clone();
        hp.size = size.clone();
        if !set_id.is_empty() {
            hp = hp.computed("set", set_id.clone()).computed(
                "assigned",
                if by_list {
                    "the hardware schedule's door list, by the door's number"
                } else {
                    "by the door's size among the schedule's sets (the door has no number in the door list)"
                },
            );
        }
        let lever = c + half * 0.8;
        hp.blocks.push(Block::new(lever - along * (1.0 * k), lever + along * (1.0 * k), t + 3.0 * k, bottom + 36.0 * k, bottom + 40.0 * k));
        let hi = batch.push(hp);
        batch.link(fi, hi);
        // The set's items, each its own piece on the door, as many as the schedule lists for
        // one door, where trade practice puts it, sided by the door's swing.
        if !set_id.is_empty() {
            let swing = super::super::wallgen::door_swing(e);
            let hs = swing.map_or(-1.0, |[a, _, _]| if (a - c).dot(along) < 0.0 { -1.0 } else { 1.0 });
            let perp = Vec2::new(-along.y, along.x);
            let ns = swing.map_or(1.0, |[a, b, _]| if (b - a).dot(perp) < 0.0 { -1.0 } else { 1.0 });
            let hinge = c + along * (hs * width / 2.0);
            let inward = along * -hs;
            let lf = Leaf { hinge, latch: hinge + inward * width, inward, swing: perp * ns, width, bottom, height, t, k };
            let hand = if swing.is_some() {
                format!("{} (the door's drawn swing)", hand_of(&lf))
            } else {
                "not drawn: hinges taken at the opening's first end, swinging to its left; check the door's hand".to_string()
            };
            if swing.is_some() {
                props.push(("hand", hand_of(&lf).to_string(), Source::computed()));
            }
            let pairs = r.words("openings.hardware_pair_units");
            let most = r.num("openings.hardware_most_per_door", 24.0).clamp(1.0, 200.0) as usize;
            let place_src = r.source_text("openings.hardware_kinds");
            let items = r.facts_of("openings.hardware_item").filter(|f| f.scope.split('|').next().is_some_and(|x| x.eq_ignore_ascii_case(&set_id)));
            for f in items.take(200) {
                let Some(it) = super::docs::HwItem::of(f) else { continue };
                let hk = hardware_kind(r, &it.ty, &format!("{} {}", it.ty, it.model)).unwrap_or(HwKind {
                    kind: "hardware_item",
                    place: "latch".into(),
                    z_in: Some(42.0),
                    backset_in: 6.0,
                    size_in: [1.0, 1.0, 1.0],
                });
                let per = if pairs.contains(&it.uom.to_ascii_uppercase()) { 2.0 } else { 1.0 };
                let units = (it.qty * per).round().clamp(0.0, 10_000.0) as usize;
                if units == 0 {
                    continue;
                }
                // Past the most laid out one by one: one piece standing for all of them.
                let (pieces, each) = if units > most { (1, units as f64) } else { (units, 0.0) };
                let item = if it.model.is_empty() { it.ty.clone() } else { format!("{} {}", it.ty, it.model).trim().to_string() };
                let basis = if hk.place == "hinges" { r.source_text("openings.hinge_source") } else { place_src.clone() };
                for blocks in hardware_blocks(&lf, &hk, r, pieces) {
                    let mut p = Piece::new("openings", hk.kind, item.clone(), &level, f.source_of())
                        .computed("set", set_id.clone())
                        .computed("qty_per_door", num_text(it.qty))
                        .computed("position", basis.clone())
                        .computed("hand", hand.clone());
                    for (key, v) in [("model", &it.model), ("finish", &it.finish), ("manufacturer", &it.mfr), ("unit", &it.uom)] {
                        if !v.is_empty() {
                            p = p.with(key, v.clone(), f.source_of());
                        }
                    }
                    p.ty = it.ty.clone();
                    p.host = Some(h);
                    p.room = room.clone();
                    p.qty = each;
                    p.blocks = blocks;
                    let pi = batch.push(p);
                    batch.link(hi, pi);
                    if let Some(p) = batch.pieces.get_mut(pi) {
                        p.touches.push(h);
                    }
                }
            }
        }
        if let Some(p) = batch.pieces.get_mut(fi) {
            p.touches.push(h);
        }
        if let Some(w) = wall
            && let Some(p) = batch.pieces.get_mut(fi)
        {
            p.touches.push(w.handle);
        }
        if !set_id.is_empty() {
            props.push(("hardware_set", set_id, Source::computed()));
        }
        props.push(("frame", frame, Source::computed()));
        updates.push((h, props));
    }
    for (h, props) in updates {
        let _ = d.modify_entity(h, |e| {
            let mut ps = props::read(&e.common.xdata);
            for (key, v, src) in props {
                // A person's edit stays.
                if ps.get(key).is_some_and(|p| p.source.kind == SourceKind::User) {
                    continue;
                }
                ps.set(key, Prop::new(v, src));
            }
            e.common.xdata = props::write(&e.common.xdata, &ps).into();
        });
    }
    let unused: f64 = left.iter().sum();
    if unused >= 1.0 {
        batch.notes.push(format!("{unused} openings of the hardware schedule's sets have no door in the model"));
    }
}

/// A piece of a run as a segment: its two ends (a stick's stored ends, else its box's long
/// axis), its size and what it is.
fn penetrant(e: &cadaddy_doc::Entity) -> Option<(Vec3, Vec3, String, String)> {
    let ps = props::read(&e.common.xdata);
    let size = super::super::bimlook::made_tag(e, "size").unwrap_or_default().to_string();
    let kind = super::super::bimlook::made_tag(e, "kind").unwrap_or("pipe").to_string();
    if let Some((a, b)) = super::super::mepatoms::build::stick_ends(&ps) {
        return Some((a, b, size, kind));
    }
    let (lo, hi) = super::super::fieldview::mesh_box(e)?;
    let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let (a, b) = if d[2] >= d[0] && d[2] >= d[1] {
        (Vec3::new(c[0], c[1], lo[2]), Vec3::new(c[0], c[1], hi[2]))
    } else if d[0] >= d[1] {
        (Vec3::new(lo[0], c[1], c[2]), Vec3::new(hi[0], c[1], c[2]))
    } else {
        (Vec3::new(c[0], lo[1], c[2]), Vec3::new(c[0], hi[1], c[2]))
    };
    Some((a, b, size, kind))
}

/// Firestops at the penetrations of rated walls and of floors.
fn firestops(d: &Drawing, sp: &BuildingSpec, r: &Rules, batch: &mut Batch, upf: f64, k: f64) {
    let rated: Vec<(super::walls::Wall, String)> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall"))
        .filter_map(|e| {
            let w = super::walls::read_wall(e)?;
            let rating = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(&w.wall_type))?.assembly.as_ref()?.fire_rating.clone()?;
            (!rating.trim().is_empty()).then_some((w, rating))
        })
        .take(200_000)
        .collect();
    // Penetrants: duct, pipe and conduit segments and the electrical pieces that run (not their
    // fittings and boxes).
    let pens: Vec<(Handle, String, Vec3, Vec3, String, String)> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("mep"))
        .filter(|e| {
            let p = super::super::bimlook::made_tag(e, "part").unwrap_or("segment");
            p == "segment" || p == "atom:conduit" || p == "atom:cable" || p.is_empty()
        })
        .filter_map(|e| penetrant(e).map(|(a, b, s, kd)| (e.handle, super::super::bspec::made_level(e).unwrap_or_default(), a, b, s, kd)))
        .take(MAX_PENETRANTS)
        .collect();
    let src = r.source("firestopping.item");
    // A grid of the rated walls by level.
    let cell = 10.0 * upf;
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
    for (i, (w, _)) in rated.iter().enumerate() {
        let [a, b] = w.ends();
        let (k0, k1) = (key(Vec2::new(a.x.min(b.x), a.y.min(b.y))), key(Vec2::new(a.x.max(b.x), a.y.max(b.y))));
        if (k1.0 - k0.0 + 1).saturating_mul(k1.1 - k0.1 + 1) > 10_000 {
            continue;
        }
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                grid.entry((w.level.to_ascii_uppercase(), x, y)).or_default().push(i);
            }
        }
    }
    let mut made = 0usize;
    for (h, level, a, b, size, kind) in &pens {
        let (pa, pb) = (Vec2::new(a.x, a.y), Vec2::new(b.x, b.y));
        if pa.dist(pb) < 1e-6 {
            continue;
        }
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let (k0, k1) = (key(Vec2::new(pa.x.min(pb.x), pa.y.min(pb.y))), key(Vec2::new(pa.x.max(pb.x), pa.y.max(pb.y))));
        if (k1.0 - k0.0 + 1).saturating_mul(k1.1 - k0.1 + 1) > 400 {
            continue;
        }
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for &wi in grid.get(&(level.to_ascii_uppercase(), x, y)).into_iter().flatten() {
                    if !seen.insert(wi) {
                        continue;
                    }
                    let Some((w, rating)) = rated.get(wi) else { continue };
                    // Where the piece's plan line crosses the wall's centreline.
                    let (sa, sb) = ((pa - w.o).dot(w.n), (pb - w.o).dot(w.n));
                    if sa.signum() == sb.signum() || (sa - sb).abs() < 1e-9 {
                        continue;
                    }
                    let t = sa / (sa - sb);
                    let at = pa + (pb - pa) * t;
                    let z = a.z + (b.z - a.z) * t;
                    let s = (at - w.o).dot(w.u);
                    if s < w.s0 || s > w.s1 || z < w.zf || z > w.zt {
                        continue;
                    }
                    // Not through an opening.
                    if w.holes.iter().any(|hh| s > hh.s0 && s < hh.s1 && z > hh.z0 && z < hh.z1) {
                        continue;
                    }
                    let mut p = Piece::new(
                        "firestopping",
                        "firestop",
                        format!("{}, {} wall, {} {}", r.text("firestopping.item"), rating, size, kind).replace("  ", " "),
                        level,
                        src.clone(),
                    );
                    p.host = Some(w.handle);
                    p.ty = rating.clone();
                    p.size = size.clone();
                    p = p.computed("penetrant", kind.clone()).with(
                        "assembly",
                        w.wall_type.clone(),
                        Source::from_doc(SourceKind::Legend, &format!("wall type {}", w.wall_type)),
                    );
                    p.touches = vec![*h, w.handle];
                    let r0 = 3.0 * k;
                    p.blocks.push(Block::new(at - w.n * (w.t / 2.0), at + w.n * (w.t / 2.0), r0 * 2.0, z - r0, z + r0));
                    batch.push(p);
                    made += 1;
                }
            }
        }
    }
    // Risers through the floors above the lowest.
    let mut floors: Vec<(&str, f64)> = sp.levels.iter().filter(|l| l.sheet.is_none()).map(|l| (l.name.as_str(), l.elevation_ft * upf)).collect();
    floors.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (h, level, a, b, size, kind) in &pens {
        if (Vec2::new(a.x, a.y)).dist(Vec2::new(b.x, b.y)) > 1.0 * k {
            continue;
        }
        let (z0, z1) = (a.z.min(b.z), a.z.max(b.z));
        // A riser's piece cut at the floors (Q6493) stands on the floor it passes: that floor is
        // named on it.
        let named: Vec<String> = d
            .model
            .get(*h)
            .and_then(|e| super::super::bimlook::made_prop(e, super::super::mep::FLOOR_PENETRATION).map(str::to_string))
            .map(|v| v.split(", ").map(|f| f.trim().to_string()).collect())
            .unwrap_or_default();
        for (name, z) in floors.iter().skip(1) {
            let passed = named.iter().any(|f| f.eq_ignore_ascii_case(name)) && *z >= z0 - 1e-6 && *z < z1 - 1e-6;
            if passed || *z > z0 + 1e-6 && *z < z1 - 1e-6 {
                let mut p = Piece::new(
                    "firestopping",
                    "firestop",
                    format!("{}, floor, {} {}", r.text("firestopping.item"), size, kind).replace("  ", " "),
                    name,
                    src.clone(),
                );
                p.size = size.clone();
                p.ty = "floor (its rating not read)".into();
                p = p.computed("penetrant", kind.clone()).computed("riser_level", level.clone());
                p.touches = vec![*h];
                let at = on_floor(sp, level, name, Vec2::new(a.x, a.y));
                let r0 = 3.0 * k;
                p.blocks.push(Block::new(at - Vec2::new(r0, 0.0), at + Vec2::new(r0, 0.0), 2.0 * r0, z - r0, z + r0));
                batch.push(p);
                made += 1;
            }
        }
    }
    if rated.is_empty() {
        batch.missing.insert("firestopping (walls)".into(), "no fire-rated wall types (the partition legend gives no ratings)".into());
    }
    let _ = made;
}

/// A riser's spot on the plan of a floor it passes, where that floor's firestop goes: each
/// level's plan is drawn in its own place, so the riser's spot moves from its own level's plan
/// origin to the floor's (their base points, else their plan regions' corners; a roof with no
/// plan of its own takes the storey's under it). Levels with no such origin share one plan: the
/// spot stays (Q6313: a roof unit's line set dropping to a unit below put its floors' firestops
/// thousands of feet off the building).
fn on_floor(sp: &BuildingSpec, riser_level: &str, floor: &str, at: Vec2) -> Vec2 {
    match super::super::schedequip::origins(sp, riser_level, floor) {
        Some((from, to)) if from.iter().chain(to.iter()).all(|v| v.is_finite()) => Vec2::new(at.x - from[0] + to[0], at.y - from[1] + to[1]),
        _ => at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A riser drawn on the first floor's plan, through the floors drawn beside it: each floor's
    /// firestop is on that floor's plan at the riser's spot, the roof's (no plan of its own) on
    /// the top storey's; levels sharing one plan leave it where it is.
    #[test]
    fn a_risers_floor_firestop_is_on_the_floors_own_plan() {
        let lv = |name: &str, el: f64, x0: f64, base: Option<[f64; 2]>| cadaddy_walk3d::Level {
            name: name.into(),
            elevation_ft: el,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: Some([x0, 0.0, x0 + 1000.0, 800.0]),
            base,
            attr: None,
            sheet: None,
        };
        let sp = BuildingSpec {
            levels: vec![
                lv("Level 1", 0.0, 0.0, Some([100.0, 50.0])),
                lv("Level 2", 10.0, 5000.0, Some([5100.0, 60.0])),
                lv("Roof", 20.0, 5000.0, None),
            ],
            ..Default::default()
        };
        let at = Vec2::new(400.0, 300.0);
        let l2 = on_floor(&sp, "Level 1", "Level 2", at);
        assert!((l2.x - 5400.0).abs() < 1e-9 && (l2.y - 310.0).abs() < 1e-9, "{l2:?}");
        let roof = on_floor(&sp, "Level 1", "Roof", at);
        assert!((roof.x - 5400.0).abs() < 1e-9 && (roof.y - 310.0).abs() < 1e-9, "{roof:?}");
        assert_eq!(on_floor(&sp, "Level 1", "Level 1", at), at);
        // One plan for every level (no regions, no base points): where it is.
        let mut one = sp.clone();
        for l in &mut one.levels {
            l.region = None;
            l.base = None;
        }
        assert_eq!(on_floor(&one, "Level 1", "Level 2", at), at);
        // A riser of no known level: where it is.
        assert_eq!(on_floor(&sp, "", "Level 2", at), at);
    }

    #[test]
    fn schedule_sizes_read() {
        assert_eq!(set_size("3-0 X 7-0 X 1 3/4"), Some((3.0, 7.0)));
        assert_eq!(set_size("6-0 X 7-0"), Some((6.0, 7.0)));
        assert_eq!(set_size("nothing"), None);
        assert_eq!(ft_in(3.0), "3-0");
        assert_eq!(ft_in(2.5), "2-6");
    }
}
