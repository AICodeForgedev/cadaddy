//! Ceiling assemblies, soffits and head of wall framing: the ceiling type legend, the soffit and
//! head of wall details (all on the ceiling types and details sheet), the room finish schedule
//! naming each room's ceiling type, and more soffits on the reflected ceiling plans.
//!
//! - **Ceiling types**: tile in an exposed grid; gypsum board on hat channel, on suspended metal
//!   framing (furring channel on cold-rolled main runners hung from the structure) or on wood
//!   furring; a toilet room ceiling of moisture-resistant board, one layer or two (the second
//!   Type X, rated); exposed structure.
//! - **Soffits**: built under a gypsum ceiling (the ceiling runs on above it), or hung from the
//!   structure beside a tile ceiling (the ceiling stops at its sides, which go 6" past it). Each
//!   has its own boards on the bottom and on the sides that show, and stud framing. The RCP tags
//!   each with its bottom (`GYP. BD. SOFFIT @ 7'-6"`) and most with their detail (`SEE 2/A-502`).
//! - **Head of wall**: per wall type, how it meets what is above — a slotted deflection track
//!   or a nested double track at the deck, a top track at the ceiling braced to the structure,
//!   steel angles at a masonry wall's head — and the bottom track every stud wall sits on.
//!
//! How the sheet words it varies by seed, as offices' standards do. Everything comes from a
//! random stream of its own, so the rest of a set stays as it was.

use crate::draw::Pen;
use crate::rng::Rng;
use crate::truth::*;
use crate::walls::{WallStyle, board_words, count_words, frac};
use cadaddy_doc::{HAlign, VAlign};

/// The details sheet's number (soffit callouts on the RCP name it).
pub const DETAILS_SHEET: &str = "A-502";

/// How the ceiling legend, the details and the finish schedule word things.
#[derive(Clone, Debug, Default)]
pub struct CeilStyle {
    /// Ceiling type tags: 0 `C1`, 1 `CLG-1`, 2 by kind (`ACT-1`, `GYP-1`, `GYP-2`, `EXP-1`).
    pub scheme: u8,
    /// Gypsum ceilings' framing: 0 hat channel, 1 suspended metal framing, 2 wood furring.
    pub framing: u8,
    /// The toilet room ceilings are two layers (moisture-resistant face, Type X behind), rated.
    pub double_wet: bool,
    /// Soffits under the toilet rooms' ceilings.
    pub toilet_soffits: bool,
    /// A soffit hung from the structure in one office per floor.
    pub office_soffit: bool,
    /// Share of soffits the RCP calls out with their detail.
    pub callouts: f64,
    /// Callouts: 0 `SEE 2/A-502`, 1 `2/A-502`, 2 `SOFFIT DETAIL 2/A-502`.
    pub callout_words: u8,
    /// Soffit tags: 0 `SOFFIT @`, 1 `GYP. BD. SOFFIT @`, 2 `BULKHEAD @`.
    pub soffit_words: u8,
    /// Deflection the deck heads take, inches.
    pub deflection_in: f64,
    /// The acoustic and plumbing walls get a double (nested) track rather than a slotted one.
    pub double_head: bool,
    /// `WALL TYPES:` (otherwise `AT TYPES` / `FOR PARTITION TYPES`).
    pub applies_words: u8,
    /// Finish schedule columns: `CEILING` / `CLG. HT.` (otherwise `CLG. TYPE` / `HEIGHT`).
    pub schedule_words: bool,
    /// Share of rooms the finish schedule says `SEE RCP` for (their type has to be worked out).
    pub see_rcp: f64,
    /// Where the details start numbering.
    pub first_detail: usize,
}

impl CeilStyle {
    pub fn new(r: &mut Rng) -> CeilStyle {
        CeilStyle {
            scheme: r.pick(&[0, 1, 2]),
            framing: r.pick(&[0, 1, 2]),
            double_wet: r.chance(0.6),
            toilet_soffits: r.chance(0.75),
            office_soffit: r.chance(0.5),
            callouts: r.pick::<f64>(&[1.0, 0.8, 0.6]),
            callout_words: r.pick(&[0, 1, 2]),
            soffit_words: r.pick(&[0, 1, 2]),
            deflection_in: r.pick::<f64>(&[0.75, 1.0]),
            double_head: r.chance(0.5),
            applies_words: r.pick(&[0, 1, 2]),
            schedule_words: r.chance(0.5),
            see_rcp: r.pick::<f64>(&[0.0, 0.1, 0.2]),
            first_detail: r.pick(&[1, 1, 3, 5]),
        }
    }
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn layer(material: &str, thickness_in: f64, side: &str) -> WallLayer {
    WallLayer { material: material.into(), thickness_in, side: side.into(), gauge: None, spacing_in: None, in_cavity: false }
}

fn spaced(material: &str, thickness_in: f64, spacing_in: f64) -> WallLayer {
    WallLayer { spacing_in: Some(spacing_in), ..layer(material, thickness_in, "framing") }
}

fn studs() -> WallLayer {
    WallLayer { gauge: Some(20), ..spaced("stud", 3.625, 16.0) }
}

/// Gypsum ceilings' framing for a style.
fn gyp_framing(cs: &CeilStyle) -> Vec<WallLayer> {
    match cs.framing {
        1 => vec![spaced("furring", 0.875, 16.0), spaced("main_runner", 1.5, 48.0)],
        2 => vec![spaced("wood_furring", 1.5, 16.0)],
        _ => vec![spaced("hat_channel", 0.875, 16.0)],
    }
}

/// A room's use for its ceiling type.
fn wet(name: &str) -> bool {
    ["MEN", "WOMEN", "TOILET", "RESTROOM"].contains(&name)
}

fn tag_for(cs: &CeilStyle, i: usize, kind: &str) -> String {
    match cs.scheme {
        1 => format!("CLG-{}", i + 1),
        2 => kind.to_string(),
        _ => format!("C{}", i + 1),
    }
}

/// The ceiling types, soffit and head details, the rooms' ceiling types and the soffits.
pub fn design_ceilings(t: &mut Truth, cs: &CeilStyle, r: &mut Rng) {
    // ---- Ceiling types ----
    let grid = t.building.rooms.iter().find_map(|rm| rm.ceiling.as_ref().and_then(|c| c.grid_ft)).unwrap_or([2.0, 4.0]);
    let wet_layers =
        if cs.double_wet { vec![layer("gyp_mr", 0.625, "finish"), layer("gyp_x", 0.625, "finish")] } else { vec![layer("gyp_mr", 0.625, "finish")] };
    let mut wet_all = wet_layers;
    // Two layers want the stiffer suspended framing.
    wet_all.extend(if cs.double_wet { vec![spaced("furring", 0.875, 16.0), spaced("main_runner", 1.5, 48.0)] } else { gyp_framing(cs) });
    let mut plain = vec![layer("gyp", 0.625, "finish")];
    plain.extend(gyp_framing(cs));
    let defs: Vec<(&str, &str, String, Vec<WallLayer>, Option<[f64; 2]>, Option<String>)> = vec![
        (
            "act",
            "ACT-1",
            "SUSPENDED ACOUSTIC TILE CEILING".into(),
            vec![layer("act_tile", 0.625, "finish"), layer("act_grid", 1.5, "framing")],
            Some(grid),
            None,
        ),
        ("gyp", "GYP-1", "GYPSUM BOARD CEILING".into(), plain, None, None),
        (
            "wet",
            "GYP-2",
            if cs.double_wet {
                "1 HOUR RATED MOISTURE-RESISTANT GYPSUM BOARD CEILING".into()
            } else {
                "MOISTURE-RESISTANT GYPSUM BOARD CEILING".into()
            },
            wet_all,
            None,
            cs.double_wet.then(|| "1 HR".to_string()),
        ),
        ("open", "EXP-1", "EXPOSED STRUCTURE, PAINTED".into(), Vec::new(), None, None),
    ];
    let mut types: Vec<(&str, CeilingType)> = Vec::new();
    for (i, (role, by_kind, name, layers, grid_ft, fire)) in defs.into_iter().enumerate() {
        let kind = match role {
            "act" => "ACT",
            "open" => "OPEN",
            _ => "GYP",
        };
        types.push((role, CeilingType { tag: tag_for(cs, i, by_kind), name, kind: kind.into(), layers, grid_ft, fire_rating: fire }));
    }
    let tag_of = |role: &str| types.iter().find(|(x, _)| *x == role).map(|(_, c)| c.tag.clone()).unwrap_or_default();

    // ---- Rooms ----
    let mut room_ceilings = Vec::new();
    for rm in &t.building.rooms {
        let role = match &rm.ceiling {
            None => "open",
            Some(c) if c.kind == "ACT" => "act",
            Some(_) if wet(&rm.name) => "wet",
            Some(_) => "gyp",
        };
        room_ceilings.push(RoomCeiling {
            level: rm.level.clone(),
            room: rm.number.clone(),
            ceiling_type: tag_of(role),
            scheduled: !r.chance(cs.see_rcp),
        });
    }

    // ---- Soffit details ----
    let mut n = cs.first_detail;
    let mut next = || {
        let s = n.to_string();
        n += 1;
        s
    };
    let under_boards =
        if cs.double_wet { vec![layer("gyp_mr", 0.625, "finish"), layer("gyp_x", 0.625, "finish")] } else { vec![layer("gyp_mr", 0.625, "finish")] };
    let under = SoffitType {
        tag: next(),
        name: "GYP. BD. SOFFIT BUILT UNDER CEILING".into(),
        bottom: under_boards.clone(),
        sides: under_boards,
        framing: vec![studs()],
        hangs_from: "ceiling".into(),
        above_ceiling_in: 0.0,
    };
    let hung = SoffitType {
        tag: next(),
        name: "GYP. BD. SOFFIT HUNG FROM STRUCTURE".into(),
        bottom: vec![layer("gyp", 0.625, "finish")],
        sides: vec![layer("gyp", 0.625, "finish")],
        framing: vec![studs(), spaced("main_runner", 1.5, 48.0)],
        hangs_from: "structure".into(),
        above_ceiling_in: 6.0,
    };

    // ---- Soffits ----
    let callout = |r: &mut Rng| r.chance(cs.callouts);
    let ceiling_of = |t: &Truth, level: &str, room: &str| {
        t.building.rooms.iter().find(|rm| rm.level == level && rm.number == room).and_then(|rm| rm.ceiling.as_ref()).map(|c| c.height_ft)
    };
    // The corridor soffits (where the main duct runs): hung beside the tile ceiling.
    let corridor: Vec<usize> = (0..t.soffits.len()).collect();
    for i in corridor {
        let c = r.chance(cs.callouts);
        let Some(sf) = t.soffits.get(i).cloned() else { continue };
        let clg = ceiling_of(t, &sf.level, &sf.room).unwrap_or(sf.height_ft);
        // Mid-corridor both long sides show, and its west end; its east end meets the end wall.
        let sides = 2.0 * sf.length_ft + sf.width_ft;
        if let Some(s) = t.soffits.get_mut(i) {
            s.soffit_type = Some(hung.tag.clone());
            s.depth_ft = Some(r6(clg - sf.height_ft));
            s.sides_ft = Some(r6(sides));
            s.called_out = c;
        }
    }
    let mut k = t.soffits.len();
    let words = |h: f64| match cs.soffit_words {
        1 => format!("GYP. BD. SOFFIT @ {}", feet_inches(h)),
        2 => format!("BULKHEAD @ {}", feet_inches(h)),
        _ => format!("SOFFIT @ {}", feet_inches(h)),
    };
    let rooms = t.building.rooms.clone();
    for lv in ["1", "2"] {
        let y_top = rooms.iter().filter(|rm| rm.level == lv).map(|rm| rm.max_ft[1]).fold(f64::MIN, f64::max);
        if cs.toilet_soffits {
            let width: f64 = r.pick(&[2.0, 2.5, 3.0]);
            let drop: f64 = r.pick(&[1.0, 1.5]);
            for rm in rooms.iter().filter(|rm| rm.level == lv && wet(&rm.name)) {
                let Some(c) = rm.ceiling.as_ref() else { continue };
                // Along the exterior (north) wall, between the room's faces.
                let (x0, x1) = (rm.min_ft[0] + 0.25, rm.max_ft[0] - 0.25);
                let (y1, y0) = (rm.max_ft[1] - 0.25, rm.max_ft[1] - 0.25 - width);
                if (rm.max_ft[1] - y_top).abs() > 1e-6 || x1 - x0 < 3.0 {
                    continue;
                }
                // Its bottom no lower than 7'-0".
                let drop = drop.min(c.height_ft - 7.0);
                if drop < 0.5 {
                    continue;
                }
                k += 1;
                let h = c.height_ft - drop;
                let called = callout(r);
                t.soffits.push(Soffit {
                    id: format!("SF{k:03}"),
                    level: lv.into(),
                    room: rm.number.clone(),
                    height_ft: r6(h),
                    width_ft: r6(width),
                    length_ft: r6(x1 - x0),
                    min_ft: [r6(x0), r6(y0)],
                    max_ft: [r6(x1), r6(y1)],
                    tag: words(h),
                    soffit_type: Some(under.tag.clone()),
                    depth_ft: Some(r6(drop)),
                    // Only its long face shows; the rest meets walls.
                    sides_ft: Some(r6(x1 - x0)),
                    called_out: called,
                    callout_detail: None,
                });
            }
        }
        if cs.office_soffit {
            // A south office (tile ceiling) beside the corridor wall.
            let offices: Vec<&Room> = rooms
                .iter()
                .filter(|rm| rm.level == lv && rm.min_ft[1] < 1e-6 && rm.ceiling.as_ref().is_some_and(|c| c.kind == "ACT") && rm.name != "CORRIDOR")
                .collect();
            if offices.is_empty() {
                continue;
            }
            let Some(rm) = offices.get(r.below(offices.len())).copied() else { continue };
            let Some(c) = rm.ceiling.as_ref() else { continue };
            let width: f64 = r.pick(&[2.0, 3.0, 4.0]);
            let drop: f64 = r.pick::<f64>(&[1.0, 1.5, 2.0]).min(c.height_ft - 7.0);
            let (x0, x1) = (rm.min_ft[0] + 0.25, rm.max_ft[0] - 0.25);
            let (y1, y0) = (rm.max_ft[1] - 0.25, rm.max_ft[1] - 0.25 - width);
            if x1 - x0 < 3.0 || drop < 0.5 {
                continue;
            }
            k += 1;
            let h = c.height_ft - drop;
            let called = callout(r);
            t.soffits.push(Soffit {
                id: format!("SF{k:03}"),
                level: lv.into(),
                room: rm.number.clone(),
                height_ft: r6(h),
                width_ft: r6(width),
                length_ft: r6(x1 - x0),
                min_ft: [r6(x0), r6(y0)],
                max_ft: [r6(x1), r6(y1)],
                tag: words(h),
                soffit_type: Some(hung.tag.clone()),
                depth_ft: Some(r6(drop)),
                sides_ft: Some(r6(x1 - x0)),
                called_out: called,
                callout_detail: None,
            });
        }
    }

    // ---- Head of wall ----
    let mut heads: Vec<HeadDetail> = Vec::new();
    for wt in t.wall_types.iter_mut() {
        let name = wt.name.to_ascii_uppercase();
        let stud_wall = wt.layers.iter().any(|l| l.material == "stud");
        let (track, to) = if name.contains("MASONRY") {
            ("angle", "deck")
        } else if wt.height == "ceiling" || wt.height == "above_ceiling" {
            ("single", "ceiling")
        } else if cs.double_head && (name.contains("ACOUSTIC") || name.contains("PLUMBING")) {
            ("double", "deck")
        } else {
            ("deflection", "deck")
        };
        let deflection = matches!(track, "deflection" | "double").then_some(cs.deflection_in);
        let found = heads.iter().position(|h| h.track == track && h.to == to);
        let i = match found {
            Some(i) => i,
            None => {
                let title = match track {
                    "angle" => "HEAD OF MASONRY WALL AT DECK",
                    "single" => "HEAD OF PARTITION AT CEILING",
                    "double" => "HEAD OF WALL AT DECK - DOUBLE TRACK",
                    _ => "HEAD OF WALL AT DECK - DEFLECTION TRACK",
                };
                heads.push(HeadDetail {
                    tag: String::new(),
                    name: title.into(),
                    track: track.into(),
                    to: to.into(),
                    deflection_in: deflection,
                    wall_types: Vec::new(),
                });
                heads.len() - 1
            }
        };
        if let Some(h) = heads.get_mut(i) {
            h.wall_types.push(wt.tag.clone());
        }
        wt.bottom_track = stud_wall;
    }
    for h in heads.iter_mut() {
        h.tag = next();
        h.wall_types.sort();
    }
    for wt in t.wall_types.iter_mut() {
        wt.head = heads.iter().find(|h| h.wall_types.contains(&wt.tag)).map(|h| h.tag.clone());
    }
    t.ceiling_types = types.into_iter().map(|(_, c)| c).collect();
    t.soffit_types = vec![under, hung];
    t.head_details = heads;
    t.room_ceilings = room_ceilings;
}

// ---- How the sheet words it --------------------------------------------------------------------

/// A ceiling layer in the legend's words.
fn ceiling_layer_words(l: &WallLayer, ws: &WallStyle, grid: Option<[f64; 2]>) -> String {
    let d = frac(l.thickness_in, ws.dash);
    let o = l.spacing_in.map(|s| frac(s, ws.dash)).unwrap_or_default();
    match l.material.as_str() {
        "act_tile" => {
            let [a, b] = grid.unwrap_or([2.0, 4.0]);
            format!("{d} ACOUSTIC CEILING TILE, {a:.0}'x{b:.0}'")
        }
        "act_grid" => "15/16\" EXPOSED TEE GRID, SUSPENDED FROM STRUCTURE".into(),
        "hat_channel" => format!("ON {d} HAT CHANNEL @ {o} O.C."),
        "furring" => format!("ON {d} FURRING CHANNEL @ {o} O.C."),
        "main_runner" => format!("{d} COLD-ROLLED CHANNEL MAIN RUNNERS @ {o} O.C., HUNG FROM STRUCTURE"),
        "wood_furring" => format!("ON 2X WOOD FURRING @ {o} O.C."),
        "stud" => format!("{d} {} GA. METAL STUD FRAMING @ {o} O.C.", l.gauge.unwrap_or(20)),
        m => format!("{d} {}", board_words(m, ws)),
    }
}

/// Boards in the legend's words: `(2) LAYERS 5/8" TYPE X GYP. BD.`, runs of one kind grouped.
fn boards_words(layers: &[WallLayer], ws: &WallStyle) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while let Some(l) = layers.get(i) {
        let mut n = 1;
        while layers.get(i + n).is_some_and(|o| o.material == l.material && (o.thickness_in - l.thickness_in).abs() < 1e-9) {
            n += 1;
        }
        let what = format!("{} {}", frac(l.thickness_in, ws.dash), board_words(&l.material, ws));
        parts.push(if n > 1 { format!("{} {what}", count_words(n, ws)) } else { what });
        i += n;
    }
    // Face first, then the layer behind it.
    parts.join(" + ")
}

fn rating_words(r: &str, ws: &WallStyle) -> String {
    let hrs = r.split_whitespace().next().unwrap_or("1");
    match ws.rating {
        1 => format!("{hrs}-HOUR RATED ASSEMBLY"),
        2 => format!("RATING: {hrs} HOUR"),
        _ => format!("FIRE RATING: {r}"),
    }
}

/// The legend's lines for a ceiling type (the tag is in its symbol).
pub fn ceiling_lines(ct: &CeilingType, ws: &WallStyle) -> Vec<String> {
    let mut out = vec![ct.name.clone()];
    if ct.kind == "OPEN" {
        out.push("NO CEILING - OPEN TO STRUCTURE ABOVE".into());
        return out;
    }
    let boards: Vec<WallLayer> = ct.layers.iter().filter(|l| l.side == "finish" && l.material.starts_with("gyp")).cloned().collect();
    if !boards.is_empty() {
        out.push(boards_words(&boards, ws));
    }
    for l in ct.layers.iter().filter(|l| !l.material.starts_with("gyp")) {
        out.push(ceiling_layer_words(l, ws, ct.grid_ft));
    }
    if let Some(r) = &ct.fire_rating {
        out.push(rating_words(r, ws));
    }
    out
}

/// A soffit detail's lines (the number is in its bubble).
pub fn soffit_lines(st: &SoffitType, ws: &WallStyle) -> Vec<String> {
    let mut out = vec![st.name.clone(), format!("BOTTOM: {}", boards_words(&st.bottom, ws)), format!("SIDES: {}", boards_words(&st.sides, ws))];
    for l in &st.framing {
        out.push(format!("FRAMING: {}", ceiling_layer_words(l, ws, None)));
    }
    if st.hangs_from == "structure" {
        out.push(format!("EXTEND SIDE BOARD {} ABOVE CEILING; CEILING STOPS AT SOFFIT", frac(st.above_ceiling_in, ws.dash)));
    } else {
        out.push("FRAME TO CEILING ABOVE; CEILING CONTINUOUS ABOVE SOFFIT".into());
    }
    out
}

/// A head of wall detail's lines.
pub fn head_lines(h: &HeadDetail, ws: &WallStyle, cs: &CeilStyle) -> Vec<String> {
    let mut out = vec![h.name.clone()];
    let d = h.deflection_in.map(|v| frac(v, ws.dash)).unwrap_or_default();
    match h.track.as_str() {
        "deflection" => {
            out.push(format!("CONTINUOUS SLOTTED DEFLECTION TOP TRACK, {d} DEFLECTION"));
            out.push("FASTEN TRACK TO DECK @ 24\" O.C.".into());
        }
        "double" => {
            out.push(format!("NESTED DOUBLE TOP TRACK, {d} DEFLECTION"));
            out.push("OUTER TRACK FASTENED TO DECK @ 24\" O.C.".into());
        }
        "angle" => out.push("CONTINUOUS STEEL ANGLE EACH SIDE, ANCHORED TO DECK".into()),
        _ => {
            out.push("TOP TRACK AT CEILING".into());
            out.push("BRACE TO STRUCTURE @ 48\" O.C.".into());
        }
    }
    let list = h.wall_types.join(", ");
    out.push(match cs.applies_words {
        1 => format!("AT TYPES {list}"),
        2 => format!("FOR PARTITION TYPES: {list}"),
        _ => format!("WALL TYPES: {list}"),
    });
    out
}

/// A soffit's callout on the RCP.
pub fn callout_words(tag: &str, cs: &CeilStyle) -> String {
    match cs.callout_words {
        1 => format!("{tag}/{DETAILS_SHEET}"),
        2 => format!("SOFFIT DETAIL {tag}/{DETAILS_SHEET}"),
        _ => format!("SEE {tag}/{DETAILS_SHEET}"),
    }
}

// ---- Drawing -----------------------------------------------------------------------------------

/// Soffit callouts on a level's RCP, under each soffit's tag.
pub fn draw_callouts(pen: &mut Pen, t: &Truth, cs: &CeilStyle, level: &str) {
    let rot = crate::draw::readable(pen.rot());
    let th = pen.st.text_in;
    for sf in t.soffits.iter().filter(|s| s.level == level && s.called_out) {
        let Some(tag) = sf.callout_detail.as_ref().or(sf.soffit_type.as_ref()) else { continue };
        let c = [(sf.min_ft[0] + sf.max_ft[0]) / 2.0, (sf.min_ft[1] + sf.max_ft[1]) / 2.0];
        let line = th * 1.5 / pen.sh.scale;
        let words = callout_words(tag, cs);
        let h = (th * 0.8).min(sf.length_ft * pen.sh.scale * 0.8 / (words.chars().count().max(1) as f64 * 1.1));
        // Just outside its south side.
        let down = (sf.max_ft[1] - sf.min_ft[1]) / 2.0 + line * 0.8;
        let p = pen.b([c[0], c[1] - down]);
        pen.text("TEXT", p, h, &words, rot, HAlign::Center, VAlign::Middle);
    }
}

/// One column of the details sheet: its heading, then entries (symbol, lines) down it.
fn column(pen: &mut Pen, x: f64, top: f64, bottom: f64, heading: &str, entries: &[(String, Vec<String>, bool)], symbol: u8) {
    pen.ptext("TEXT", [x, top], 0.16, heading, HAlign::Left);
    pen.pline("TEXT", [x, top - 0.06], [x + heading.len() as f64 * 0.135, top - 0.06]);
    let mut y = top - 0.6;
    for (tag, lines, bubble) in entries {
        let h = (lines.len() as f64 * 0.2 + 0.15).max(0.8);
        if y - h < bottom {
            break;
        }
        let c = pen.p([x + 0.3, y - 0.05]);
        let r = pen.pl(0.22);
        if *bubble {
            pen.circle("A-ANNO-WTAG", c, r);
        } else if tag.chars().count() > 2 {
            // Longer tags in a box that fits them.
            let w = pen.pl(tag.chars().count() as f64 * 0.06 + 0.08);
            let hh = pen.pl(0.14);
            pen.rect("A-ANNO-WTAG", c - cadaddy_geom::Vec2::new(w, hh), c + cadaddy_geom::Vec2::new(w, hh));
        } else {
            crate::walls::tag_symbol(pen, "A-ANNO-WTAG", c, r, symbol);
        }
        pen.text("A-ANNO-WTAG", c, 0.12, tag, 0.0, HAlign::Center, VAlign::Middle);
        for (i, line) in lines.iter().enumerate() {
            let size = if i == 0 { 0.1 } else { 0.085 };
            pen.ptext("TEXT", [x + 0.85, y - i as f64 * 0.2], size, line, HAlign::Left);
        }
        if *bubble && let Some(first) = lines.first() {
            pen.pline("TEXT", [x + 0.85, y - 0.05], [x + 0.85 + first.len() as f64 * 0.085, y - 0.05]);
        }
        y -= h + 0.3;
    }
}

/// The ceiling types and details sheet: ceiling types, soffit details, head of wall details, and
/// the general notes on track.
pub fn draw_details(pen: &mut Pen, t: &Truth, ws: &WallStyle, cs: &CeilStyle, area: [[f64; 2]; 2]) {
    let [[x0, y0], [x1, y1]] = area;
    let col = ((x1 - x0) / 3.0).max(6.0);
    let top = y1 - 0.4;
    let bottom = y0 + 1.4;
    // Ceiling type tags in a symbol other than the wall tags'.
    let symbol = (ws.symbol + 1) % 4;
    let ceil: Vec<(String, Vec<String>, bool)> = t.ceiling_types.iter().map(|c| (c.tag.clone(), ceiling_lines(c, ws), false)).collect();
    column(pen, x0 + 0.3, top, bottom, "CEILING TYPES", &ceil, symbol);
    let soff: Vec<(String, Vec<String>, bool)> = t.soffit_types.iter().map(|s| (s.tag.clone(), soffit_lines(s, ws), true)).collect();
    column(pen, x0 + 0.3 + col, top, bottom, "SOFFIT DETAILS", &soff, symbol);
    let heads: Vec<(String, Vec<String>, bool)> = t.head_details.iter().map(|h| (h.tag.clone(), head_lines(h, ws, cs), true)).collect();
    column(pen, x0 + 0.3 + 2.0 * col, top, bottom, "HEAD OF WALL DETAILS", &heads, symbol);
    let notes = [
        "GENERAL NOTES:",
        "1. ALL METAL STUD WALLS: BOTTOM TRACK FASTENED TO SLAB @ 24\" O.C.",
        "2. CEILING HEIGHTS: SEE REFLECTED CEILING PLANS AND ROOM FINISH SCHEDULE.",
        "3. SOFFIT BOTTOMS: SEE REFLECTED CEILING PLANS.",
    ];
    let mut y = y0 + 1.0;
    for n in notes {
        pen.ptext("TEXT", [x0 + 0.3, y], 0.09, n, HAlign::Left);
        y -= 0.17;
    }
}

/// Made-up finishes for the schedule's other columns.
fn finishes(name: &str) -> [&'static str; 3] {
    if wet(name) {
        ["CT-1", "CT-1", "EPX-1"]
    } else if name == "MECH" {
        ["SC", "RB-1", "PT-2"]
    } else if name == "CORRIDOR" {
        ["VCT-1", "RB-1", "PT-1"]
    } else {
        ["CPT-1", "RB-1", "PT-1"]
    }
}

/// The room finish schedule: each room's floor, base, walls, ceiling type and height.
pub fn draw_schedule(pen: &mut Pen, t: &Truth, cs: &CeilStyle, area: [[f64; 2]; 2]) {
    let [[x0, y0], [_, y1]] = area;
    let (clg, ht) = if cs.schedule_words { ("CEILING", "CLG. HT.") } else { ("CLG. TYPE", "HEIGHT") };
    let head = ["ROOM", "NAME", "FLOOR", "BASE", "WALLS", clg, ht, "REMARKS"].map(String::from).to_vec();
    let mut rooms: Vec<&Room> = t.building.rooms.iter().collect();
    rooms.sort_by(|a, b| a.number.cmp(&b.number));
    let per = ((y1 - y0 - 1.0) / 0.24) as usize;
    let mut x = x0 + 0.3;
    for chunk in rooms.chunks(per.clamp(4, 60).saturating_sub(2)) {
        let mut rows = vec![head.clone()];
        for rm in chunk {
            let rc = t.room_ceilings.iter().find(|c| c.level == rm.level && c.room == rm.number);
            let hidden = rm.ceiling.as_ref().is_some_and(|c| c.height_hidden);
            let ctype = match rc {
                Some(c) if c.scheduled && !hidden => c.ceiling_type.clone(),
                Some(_) => "SEE RCP".into(),
                None => String::new(),
            };
            let height = match &rm.ceiling {
                Some(c) if c.height_hidden => String::new(),
                Some(c) => feet_inches(c.height_ft),
                None => "--".to_string(),
            };
            let soffit = t.soffits.iter().any(|s| s.level == rm.level && s.room == rm.number);
            let [f, b, w] = finishes(&rm.name);
            rows.push(vec![
                rm.number.clone(),
                rm.name.clone(),
                f.into(),
                b.into(),
                w.into(),
                ctype,
                height,
                if soffit { "SOFFIT, SEE RCP".into() } else { String::new() },
            ]);
        }
        crate::draw::table(pen, [x, y1 - 0.5], "ROOM FINISH SCHEDULE", &rows);
        x += 11.0;
    }
}
