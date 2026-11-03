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
    let walls: Vec<super::walls::Wall> = d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall")).filter_map(|e| super::walls::read_wall(e)).take(200_000).collect();
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
                let inside: Vec<f64> = m.vertices.iter().filter(|v| (Vec2::new(v.x, v.y) - w.o).dot(w.n).abs() <= w.t).map(|v| (Vec2::new(v.x, v.y) - w.o).dot(w.u)).collect();
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
                r.words("framing.wood_words").iter().any(|w| u.contains(w.as_str())) && !r.words("framing.steel_words").iter().any(|w| u.contains(w.as_str()))
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
                sets.iter().enumerate().find(|(_, (f, sz))| of_set(f) && fits(sz)).or_else(|| sets.iter().enumerate().find(|(_, (f, _))| of_set(f))).map(|(i, x)| (i, x.0))
            });
        let by_list = listed.is_some();
        let pick = listed.or_else(|| sets
            .iter()
            .enumerate()
            .filter(|(i, _)| left.get(*i).is_some_and(|n| *n >= 1.0))
            .find(|(_, (_, sz))| sz.is_some_and(|(w, hh)| (w - wf).abs() < 0.35 && (hh - hf).abs() < 0.6))
            .or_else(|| sets.iter().enumerate().find(|(i, _)| left.get(*i).is_some_and(|n| *n >= 1.0)))
            .map(|(i, x)| (i, x.0)));
        if let Some((i, _)) = pick
            && let Some(n) = left.get_mut(i)
        {
            *n -= 1.0;
        }
        // The set's door and frame (`… WOOD DOOR X HM FRAME`): the part that names a frame.
        let named_frame = pick.map(|(_, f)| f.quote.to_ascii_uppercase()).and_then(|q| q.split(" X ").find(|x| x.contains("FRAME")).map(|x| x.split_whitespace().take(4).collect::<Vec<_>>().join(" ")));
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
            None => (format!("{} (no hardware schedule read)", r.text("openings.hardware_item")), Source::from_doc(SourceKind::Default, "no door hardware schedule in the documents read"), String::new()),
        };
        let mut hp = Piece::new("openings", "hardware_set", item, &level, h_src);
        hp.host = Some(h);
        hp.room = room.clone();
        hp.size = size.clone();
        if !set_id.is_empty() {
            hp = hp.computed("set", set_id.clone()).computed(
                "assigned",
                if by_list { "the hardware schedule's door list, by the door's number" } else { "by the door's size among the schedule's sets (the door has no number in the door list)" },
            );
        }
        let lever = c + half * 0.8;
        hp.blocks.push(Block::new(lever - along * (1.0 * k), lever + along * (1.0 * k), t + 3.0 * k, bottom + 36.0 * k, bottom + 40.0 * k));
        let hi = batch.push(hp);
        batch.link(fi, hi);
        if let Some(p) = batch.pieces.get_mut(fi) {
            p.touches.push(h);
        }
        if let Some(w) = wall
            && let Some(p) = batch.pieces.get_mut(fi) {
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
                    let mut p = Piece::new("firestopping", "firestop", format!("{}, {} wall, {} {}", r.text("firestopping.item"), rating, size, kind).replace("  ", " "), level, src.clone());
                    p.host = Some(w.handle);
                    p.ty = rating.clone();
                    p.size = size.clone();
                    p = p.computed("penetrant", kind.clone()).with("assembly", w.wall_type.clone(), Source::from_doc(SourceKind::Legend, &format!("wall type {}", w.wall_type)));
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
        for (name, z) in floors.iter().skip(1) {
            if *z > z0 + 1e-6 && *z < z1 - 1e-6 {
                let mut p = Piece::new("firestopping", "firestop", format!("{}, floor, {} {}", r.text("firestopping.item"), size, kind).replace("  ", " "), name, src.clone());
                p.size = size.clone();
                p.ty = "floor (its rating not read)".into();
                p = p.computed("penetrant", kind.clone()).computed("riser_level", level.clone());
                p.touches = vec![*h];
                let at = Vec2::new(a.x, a.y);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_sizes_read() {
        assert_eq!(set_size("3-0 X 7-0 X 1 3/4"), Some((3.0, 7.0)));
        assert_eq!(set_size("6-0 X 7-0"), Some((6.0, 7.0)));
        assert_eq!(set_size("nothing"), None);
        assert_eq!(ft_in(3.0), "3-0");
        assert_eq!(ft_in(2.5), "2-6");
    }
}
