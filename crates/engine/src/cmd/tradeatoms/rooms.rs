//! The rooms' pieces: flooring by each room's finish (pieces by the product's size the
//! documents give), base by length round the room less its doors, each ceiling's boards (with
//! their tape, compound, screws and paint) or its tiles, mains, tees and wall angle, and the
//! toilet accessories the submittals list, in the rooms they name.

use super::*;

/// Most rooms and ceilings laid out.
const MAX_ROOMS: usize = 100_000;

/// A polygon's perimeter.
fn perimeter(poly: &[Vec2]) -> f64 {
    let n = poly.len();
    (0..n).filter_map(|i| Some(poly.get(i)?.dist(*poly.get((i + 1) % n)?))).sum()
}

/// A thin plate over a polygon at `z` (`t` thick, up).
fn plate(poly: &[Vec2], z: f64, t: f64) -> Mesh {
    let mut m = super::super::wallgen::empty_mesh();
    let n = poly.len();
    if n < 3 || t.is_nan() || t <= 0.0 {
        return m;
    }
    for p in poly {
        m.vertices.push(Vec3::new(p.x, p.y, z));
    }
    for p in poly {
        m.vertices.push(Vec3::new(p.x, p.y, z + t));
    }
    let n32 = u32::try_from(n).unwrap_or(0);
    m.faces.push(cadaddy_doc::MeshFace { v: (0..n32).rev().collect(), hidden: 0 });
    m.faces.push(cadaddy_doc::MeshFace { v: (0..n32).map(|i| n32 + i).collect(), hidden: 0 });
    for i in 0..n32 {
        let j = (i + 1) % n32;
        m.faces.push(cadaddy_doc::MeshFace { v: vec![i, j, n32 + j, n32 + i], hidden: 0 });
    }
    m
}

/// A marker box at a point (an accessory, a bag of screws): `s` across, from `z` up `h`.
fn marker(c: Vec2, s: f64, z: f64, h: f64) -> Block {
    Block::new(c - Vec2::new(s / 2.0, 0.0), c + Vec2::new(s / 2.0, 0.0), s, z, z + h)
}

fn centroid(poly: &[Vec2]) -> Vec2 {
    let n = poly.len().max(1) as f64;
    let s = poly.iter().fold(Vec2::ZERO, |a, p| a + *p);
    s * (1.0 / n)
}

pub(super) fn lay_out(d: &Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, want: &dyn Fn(&str) -> bool) {
    let upf = ctx.upf;
    let k = upf / 12.0;
    // Doors by level: their plan centre and width (their leaves' extents).
    let doors: Vec<(String, Vec2, f64)> = d
        .model
        .iter()
        .filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("door"))
        .filter_map(|e| {
            let (lo, hi) = super::super::fieldview::mesh_box(e)?;
            let w = (hi[0] - lo[0]).max(hi[1] - lo[1]);
            Some((super::super::bspec::made_level(e).unwrap_or_default(), Vec2::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0), w))
        })
        .take(MAX_ELEMENTS)
        .collect();
    if want("finishes") {
        let piece_sf = r.facts_of("finishes.flooring_piece_sf").filter_map(|f| f.number.map(|n| (f.clone(), n))).collect::<Vec<_>>();
        for rm in ctx.rooms.iter().take(MAX_ROOMS) {
            let label = Ctx::room_label(rm);
            let area_sf = cadaddy_geom::shoelace(&rm.outline).abs() / (upf * upf);
            if area_sf < 1.0 {
                continue;
            }
            // Flooring by the room's finish; its pieces by the product the documents size.
            let finish = rm.finish.trim().to_string();
            let code = finish.to_ascii_uppercase();
            let named = piece_sf
                .iter()
                .find(|(f, _)| !code.is_empty() && (f.value.to_ascii_uppercase().contains(&code) || f.quote.to_ascii_uppercase().contains(&code)));
            let product = named.or_else(|| (piece_sf.len() == 1).then(|| piece_sf.first()).flatten());
            let (item, src) = match (&finish, product) {
                (f, Some(_)) if !f.is_empty() => (format!("flooring {f}"), Source::from_doc(SourceKind::Print, "the room's finish code on the plan")),
                (f, None) if !f.is_empty() => (format!("flooring {f}"), Source::from_doc(SourceKind::Print, "the room's finish code on the plan")),
                (_, Some((fact, _))) => {
                    (format!("flooring: {} (the only flooring the documents size; no finish on the plan)", fact.value), fact.source_of())
                }
                _ => {
                    ("flooring (no finish given)".into(), Source::from_doc(SourceKind::Default, "no finish code in the room or finish schedule read"))
                }
            };
            let mut p = Piece::new("finishes", "flooring", item, &rm.level, src);
            p.host = Some(rm.handle);
            p.room = label.clone();
            p.ty = finish.clone();
            p.area_sf = area_sf;
            if let Some((fact, sf)) = product.filter(|(_, sf)| *sf > 0.01) {
                p.qty = (area_sf / sf).ceil();
                p = p.with("piece_sf", num_text(*sf), fact.source_of()).with("product", fact.value.clone(), fact.source_of());
                if named.is_none() {
                    p = p.computed("product_match", "the only flooring the documents size (its finish code not in them)");
                }
            }
            p.custom = Some(plate(&rm.outline, rm.z, 0.25 * k));
            batch.push(p);
            // Base round the room, less its doors.
            let per = perimeter(&rm.outline);
            let mut cut = 0.0;
            for (lv, c, w) in &doors {
                if !lv.eq_ignore_ascii_case(&rm.level) {
                    continue;
                }
                let near = rm.outline.iter().enumerate().any(|(i, a)| {
                    let b = rm.outline.get((i + 1) % rm.outline.len()).copied().unwrap_or(*a);
                    seg_dist(*c, *a, b) < 1.0 * upf
                });
                if near {
                    cut += w;
                }
            }
            let len = ((per - cut) / upf).max(0.0);
            if len > 0.5 {
                let base_item = sp
                    .rooms
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(&rm.name) && x.number.as_deref().unwrap_or("") == rm.number)
                    .and_then(|x| x.base.clone());
                let (item, src) = match base_item {
                    Some(b) => (b, Source::from_doc(SourceKind::Schedule, "the room finish schedule's base")),
                    None => (
                        format!("{} {}", inches_text(r.num("finishes.base_height_in", 4.0)), r.text("finishes.base_item")),
                        r.source("finishes.base_height_in"),
                    ),
                };
                let mut b = Piece::new("finishes", "base", item, &rm.level, src);
                b.host = Some(rm.handle);
                b.room = label.clone();
                b.length_ft = len;
                b = b.computed("doors_less_ft", num_text(cut / upf));
                let c = centroid(&rm.outline);
                b.blocks.push(marker(c, 2.0 * k, rm.z, 4.0 * k));
                batch.push(b);
            }
        }
    }
    if want("drywall") || want("finishes") || want("framing") {
        ceilings(d, sp, r, ctx, batch, want, upf, k);
    }
    if want("specialties") {
        accessories(sp, r, ctx, batch, upf, k);
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 1e-12 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// Each built ceiling as its boards (gypsum) or tiles and grid.
#[allow(clippy::too_many_arguments)]
fn ceilings(d: &Drawing, sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, want: &dyn Fn(&str) -> bool, upf: f64, k: f64) {
    for e in d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("ceiling")).take(MAX_ROOMS) {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let level = super::super::bspec::made_level(e).unwrap_or_default();
        let polys: Vec<(Vec<Vec2>, f64)> = geo::flat_faces(m);
        let Some(z) = polys.iter().map(|p| p.1).reduce(f64::min) else { continue };
        let polys: Vec<Vec<Vec2>> = polys.into_iter().filter(|p| (p.1 - z).abs() < 1e-6).map(|p| p.0).collect();
        let area = polys.iter().map(|p| cadaddy_geom::shoelace(p).abs()).sum::<f64>() / (upf * upf);
        if area < 1.0 {
            continue;
        }
        let ty_name = super::super::bimlook::made_tag(e, "type").unwrap_or_default().to_string();
        let ct = sp.ceiling_types.iter().find(|t| !ty_name.is_empty() && t.name.eq_ignore_ascii_case(&ty_name));
        let room = super::super::bimlook::made_tag(e, "room").map(str::to_string).filter(|x| !x.is_empty()).or_else(|| {
            let c = polys.first().map(|p| centroid(p))?;
            ctx.room_at(&level, c).map(Ctx::room_label)
        });
        let room = room.unwrap_or_default();
        let grid = matches!(ct.map(|t| t.kind), Some(cadaddy_walk3d::CeilingKind::Grid));
        if matches!(ct.map(|t| t.kind), Some(cadaddy_walk3d::CeilingKind::Open)) {
            continue;
        }
        let per: f64 = polys.iter().map(|p| perimeter(p)).sum::<f64>() / upf;
        let c0 = polys.first().map(|p| centroid(p)).unwrap_or(Vec2::ZERO);
        if grid {
            if !want("finishes") {
                continue;
            }
            let tile = ct.and_then(|t| t.grid_in).unwrap_or_else(|| {
                let v = r.nums("finishes.act_tile_in");
                [v.first().copied().unwrap_or(24.0), v.get(1).copied().unwrap_or(24.0)]
            });
            let t_src = if ct.and_then(|t| t.grid_in).is_some() {
                Source::from_doc(SourceKind::Legend, &format!("ceiling type {ty_name}"))
            } else {
                r.source("finishes.act_tile_in")
            };
            let tile_sf = (tile[0] * tile[1] / 144.0).max(0.1);
            let mut p =
                Piece::new("finishes", "tile", format!("acoustical ceiling tile {}x{}", num_text(tile[0]), num_text(tile[1])), &level, t_src.clone());
            p.host = Some(e.handle);
            p.room = room.clone();
            p.area_sf = area;
            p.qty = (area / tile_sf).ceil();
            p.ty = ty_name.clone();
            p.custom = Some(plate(polys.first().map(Vec::as_slice).unwrap_or(&[]), z - 0.75 * k, 0.75 * k));
            let ti = batch.push(p);
            let mains = area / r.num("finishes.grid_main_spacing_ft", 4.0).max(1.0);
            let short = tile[0].min(tile[1]) / 12.0;
            let tees = area / short.max(0.5);
            for (kind, item, len) in [
                ("grid_main", "ceiling grid main runner", mains),
                ("grid_tee", "ceiling grid cross tee", tees),
                ("wall_angle", "ceiling wall angle", per),
            ] {
                let mut g = Piece::new("finishes", kind, item, &level, r.source("finishes.grid_main_spacing_ft"));
                g.host = Some(e.handle);
                g.room = room.clone();
                g.length_ft = len;
                g.blocks.push(marker(c0, 1.0 * k, z - 2.0 * k, 1.0 * k));
                let gi = batch.push(g);
                batch.link(gi, ti);
            }
            continue;
        }
        // Gypsum: its board layers (the ceiling type's, else one layer by rule).
        let layers: Vec<(String, f64, Source)> = match ct
            .filter(|t| t.layers.iter().any(|l| l.category == "board" || l.material.to_ascii_uppercase().contains("GYP")))
        {
            Some(t) => t
                .layers
                .iter()
                .filter(|l| l.category == "board" || l.material.to_ascii_uppercase().contains("GYP"))
                .map(|l| (l.material.clone(), l.thickness_in.max(0.25), Source::from_doc(SourceKind::Legend, &format!("ceiling type {}", t.name))))
                .collect(),
            None => vec![(
                "5/8\" gypsum board ceiling".into(),
                0.625,
                Source::from_doc(SourceKind::Default, "company default: one layer of 5/8\" gypsum board where the ceiling has no type"),
            )],
        };
        if want("drywall") {
            let lengths = r.nums("drywall.sheet_lengths_ft");
            let sl = lengths.iter().copied().fold(8.0f64, f64::max);
            let sw = r.num("drywall.sheet_width_ft", 4.0);
            let mut sheets_n = 0usize;
            for (li, (material, th, src)) in layers.iter().enumerate() {
                let ty = super::walls::board_type(r, material);
                let zl = z - th / 12.0 * upf * (li as f64 + 1.0);
                for poly in &polys {
                    sheets_n += ceiling_sheets(poly, material, &ty, *th, src, &level, &room, e.handle, zl, sw, sl, upf, li + 1, r, batch);
                }
            }
            // Its face's tape, compound and screws (the joints its layout makes: 4' and the
            // sheet length apart).
            let joints = area / sw + area / sl;
            let mut tp = Piece::new("drywall", "tape", r.text("drywall.tape_item"), &level, r.source("drywall.tape_item"));
            tp.host = Some(e.handle);
            tp.room = room.clone();
            tp.length_ft = joints + per;
            tp = tp.computed("at", "the ceiling sheets' joints and its edges");
            tp.blocks.push(marker(c0, 1.0 * k, z - 3.0 * k, 1.0 * k));
            batch.push(tp);
            let mut mud = Piece::new("drywall", "compound", r.text("drywall.compound_item"), &level, r.source("drywall.compound_lb_per_sf"));
            mud.host = Some(e.handle);
            mud.room = room.clone();
            mud.weight_lb = area * r.num("drywall.compound_lb_per_sf", 0.14);
            mud.extra.push(("finished_sf", num_text(area), Source::computed()));
            mud.blocks.push(marker(c0 + Vec2::new(3.0 * k, 0.0), 1.0 * k, z - 3.0 * k, 1.0 * k));
            batch.push(mud);
            let mut sc = Piece::new("drywall", "screws", r.text("drywall.screw_item"), &level, r.source("drywall.screws_per_sf"));
            sc.host = Some(e.handle);
            sc.room = room.clone();
            sc.qty = (area * layers.len() as f64 * r.num("drywall.screws_per_sf", 1.0)).round().max(1.0);
            sc.blocks.push(marker(c0 - Vec2::new(3.0 * k, 0.0), 1.0 * k, z - 3.0 * k, 1.0 * k));
            batch.push(sc);
            let _ = sheets_n;
        }
        if want("finishes") && !room.is_empty() {
            let coats = r.num("finishes.paint_coats", 2.0).clamp(0.0, 6.0).round() as usize;
            for coat in 1..=coats {
                let mut p = Piece::new("finishes", "paint", r.text("finishes.paint_item"), &level, r.source("finishes.paint_coats"));
                p.host = Some(e.handle);
                p.room = room.clone();
                p.ty = if coat == 1 && coats > 1 { "primer".into() } else { "finish coat".into() };
                p.area_sf = area;
                p = p.computed("coat", coat.to_string()).computed("face", "ceiling");
                p.custom = Some(plate(polys.first().map(Vec::as_slice).unwrap_or(&[]), z - 0.02 * k * (coat as f64 + 1.0) - 1.25 * k, 0.02 * k));
                batch.push(p);
            }
        }
    }
}

/// A ceiling polygon's sheets (each where it lands); how many.
#[allow(clippy::too_many_arguments)]
fn ceiling_sheets(
    poly: &[Vec2],
    material: &str,
    ty: &str,
    th: f64,
    src: &Source,
    level: &str,
    room: &str,
    host: Handle,
    z: f64,
    sw: f64,
    sl: f64,
    upf: f64,
    layer: usize,
    r: &Rules,
    batch: &mut Batch,
) -> usize {
    let b = cadaddy_geom::Bounds2::from_points(poly.iter().copied());
    if b.is_empty() {
        return 0;
    }
    let (lo, hi) = (b.min, b.max);
    let long_x = hi.x - lo.x >= hi.y - lo.y;
    let (dx, dy) = if long_x { (sl * upf, sw * upf) } else { (sw * upf, sl * upf) };
    let mut n = 0usize;
    let mut y = lo.y;
    while y < hi.y - 1e-6 && n < 20_000 {
        let mut x = lo.x;
        while x < hi.x - 1e-6 && n < 20_000 {
            let (a, c) = (Vec2::new(x, y), Vec2::new((x + dx).min(hi.x), (y + dy).min(hi.y)));
            let land = cadaddy_geom::shoelace(&super::floors::clip_box(poly, a, c)).abs();
            if land > 0.01 * upf * upf {
                let mut p = Piece::new("drywall", "sheet", format!("{material}, {}x{}", num_text(sw), num_text(sl)), level, src.clone());
                p.host = Some(host);
                p.room = room.to_string();
                p.ty = ty.to_string();
                p.size = inches_text(th);
                p.area_sf = land / (upf * upf);
                p = p
                    .with("sheet_ft", format!("{}x{}", num_text(sw), num_text(sl)), r.source("drywall.sheet_lengths_ft"))
                    .computed("cut", if land < dx * dy * 0.999 { "true" } else { "false" })
                    .computed("face", "ceiling")
                    .computed("layer", layer.to_string());
                let cy = (a.y + c.y) / 2.0;
                p.blocks.push(Block::new(Vec2::new(a.x, cy), Vec2::new(c.x, cy), c.y - a.y, z, z + th / 12.0 * upf));
                batch.push(p);
                n += 1;
            }
            x += dx;
        }
        y += dy;
    }
    n
}

/// The rooms of unit types (guest rooms): the room elements whose number a unit instance has.
fn unit_rooms<'a>(sp: &BuildingSpec, ctx: &'a Ctx) -> Vec<(&'a RoomEl, String)> {
    let units: Vec<(String, String, String)> = sp
        .unit_instances
        .as_array()
        .into_iter()
        .flatten()
        .take(50_000)
        .filter_map(|u| {
            let t = |k: &str| u.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let number = t("number");
            (!number.is_empty()).then(|| (t("level"), number, if t("unit_type").is_empty() { t("type") } else { t("unit_type") }))
        })
        .collect();
    let mut out = Vec::new();
    for rm in &ctx.rooms {
        if rm.number.trim().is_empty() {
            continue;
        }
        if let Some((_, _, ty)) =
            units.iter().find(|(lv, n, _)| n.eq_ignore_ascii_case(rm.number.trim()) && (lv.is_empty() || lv.eq_ignore_ascii_case(&rm.level)))
        {
            out.push((rm, ty.clone()));
        }
    }
    out
}

/// The toilet accessories the submittals list, in the rooms their titles name: a unit's rooms
/// (guest rooms) or the public toilets. Each room in scope takes its share of the listed
/// quantity; a quantity smaller than the rooms goes to the accessible units first.
fn accessories(sp: &BuildingSpec, r: &Rules, ctx: &Ctx, batch: &mut Batch, upf: f64, k: f64) {
    let items: Vec<&Fact> = r.facts_of("specialties.accessory").collect();
    if items.is_empty() {
        batch.missing.insert("toilet accessories".into(), "no toilet accessory submittal read (bim.atoms.docs)".into());
        return;
    }
    let units = unit_rooms(sp, ctx);
    let public_words = r.words("specialties.public_room_words");
    // A public toilet by its name's words (whole words: `RR`, not the `RR` in a corridor).
    let public: Vec<&RoomEl> = ctx
        .rooms
        .iter()
        .filter(|rm| {
            let name = rm.name.to_ascii_uppercase();
            let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
            public_words.iter().any(|w| if w.contains(' ') { name.contains(w.as_str()) } else { words.contains(&w.as_str()) })
        })
        .collect();
    let accessible = |ty: &str| {
        let u = ty.to_ascii_uppercase();
        u.contains("ADA") || u.contains("ACCESSIBLE") || u.contains("HC")
    };
    let mut unplaced = 0.0;
    for f in items {
        let Some(n) = f.number.filter(|n| *n >= 1.0) else { continue };
        let scope: Vec<(&RoomEl, bool)> = if f.scope.contains("unit rooms") {
            units.iter().map(|(rm, ty)| (*rm, accessible(ty))).collect()
        } else if f.scope.contains("public") {
            public.iter().map(|rm| (*rm, false)).collect()
        } else {
            Vec::new()
        };
        if scope.is_empty() {
            unplaced += n;
            continue;
        }
        let rooms: Vec<&RoomEl> = if n < scope.len() as f64 && scope.iter().any(|(_, a)| *a) {
            scope.iter().filter(|(_, a)| *a).map(|(rm, _)| *rm).collect()
        } else {
            scope.iter().map(|(rm, _)| *rm).collect()
        };
        let per = (n / rooms.len() as f64).round().max(1.0);
        let mut left = n;
        for rm in rooms.iter() {
            if left < 1.0 {
                break;
            }
            let q = per.min(left);
            left -= q;
            let mut p = Piece::new("specialties", "accessory", f.value.clone(), &rm.level, f.source_of());
            p.host = Some(rm.handle);
            p.room = Ctx::room_label(rm);
            p.qty = q;
            p = p.computed("position", "the room's centre: the documents give no position").computed("rooms", f.scope.clone());
            let c = centroid(&rm.outline);
            p.blocks.push(marker(c, 4.0 * k, rm.z + 4.0 * upf, 4.0 * k));
            batch.push(p);
        }
        unplaced += left.max(0.0);
    }
    if unplaced > 0.0 {
        batch.notes.push(format!("{unplaced} toilet accessories listed for rooms the model has none of (no unit rooms or public toilets matched)"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perimeters_and_plates() {
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        assert!((perimeter(&sq) - 40.0).abs() < 1e-9);
        assert_eq!(plate(&sq, 0.0, 1.0).faces.len(), 6);
        assert!(plate(&sq[..2], 0.0, 1.0).faces.is_empty());
        assert!((seg_dist(Vec2::new(5.0, 3.0), Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)) - 3.0).abs() < 1e-9);
    }
}
