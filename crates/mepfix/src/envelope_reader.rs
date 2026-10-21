//! Envelope reader (Q337): the roof assembly and exterior wall assemblies, the roof's drainage,
//! flashing and penetrations, and the details, from the architectural sheets and the spec; then
//! the takeoff and coordination flags from what was read (through [`crate::envelope::takeoff`]
//! and [`crate::envelope::flags`], the same as the truth's).
//!
//! - **Roof drainage and flashing plan** (`… DRAINAGE … PLAN`, level `R`): overflow scuppers
//!   (boxes) and overflow drains (circles) on an overflow layer; crickets and saddles (closed
//!   outlines on a cricket layer) with their `CRICKET`/`SADDLE` label and `x" HIGH` note; pipes
//!   through the roof (circles on a penetration layer, `3" VTR`); walkway pad strips; the edge
//!   note (`COPING` or `EDGE AND FASCIA`, gauge); the tapered insulation note (`MIN. AT
//!   DRAINS`); and detail callouts: a bubble with the detail number over its sheet, and a leader
//!   to what it details (a curb's corner, a drain, a pipe, the roof edge).
//! - **Assemblies sheet** (`… ASSEMBLIES`): the roof assembly table (`ROOF ASSEMBLY R-1` /
//!   `ROOF TYPE R-1`, one row per layer, top down: component, description, thickness with
//!   `60 MIL`, `(2) 2"`, `1/2" MIN.`), the exterior wall assembly tables (`EXTERIOR WALL
//!   ASSEMBLY EW-1: LEVEL 1` / `LEVEL 2 TO TOP OF PARAPET` / `ALL LEVELS`), the wall section's
//!   datums (`LEVEL 2` over `EL. 13'-6"`, or `ROOF = 27'-0"`), and the detail index (numbered
//!   bubbles with their titles).
//! - **Spec** (Division 07 text): insulation R per inch and the least average R, membrane mils,
//!   coping gauge.
//! - From the other readers: the roof edge, parapet, slope, drains and curbs (Q307), windows,
//!   storefront and exterior doors (Q307), the exterior walls' outer face (Q293) and the roof
//!   beams (Q263).

use crate::arch_reader::{ArchResult, Shapes, Txt, cell, dist, length_in, number, shapes, tables, upper};
use crate::envelope::{EnvOpening, Model, flags, takeoff};
use crate::sheet_reader::{self, Alignment, SheetMap};
use crate::struct_reader::StructuralModel;
use crate::truth::*;
use crate::wall_reader::WallResult;
use serde_json::{Value, json};

/// Most items of a kind kept from one sheet (hostile input).
const MAX_ITEMS: usize = 5_000;

#[derive(Clone, Debug, Default)]
pub struct EnvelopeResult {
    pub model: Model,
    pub takeoff: Vec<QtyItem>,
    pub flags: Vec<EnvFlag>,
    /// The tapered insulation note's least thickness on the drainage plan, inches.
    pub tapered_note_in: Option<f64>,
    /// Whether a spec was given and understood.
    pub spec_read: bool,
    /// What was seen but not understood.
    pub notes: Vec<String>,
}

// ---- The drainage plan -------------------------------------------------------------------------

fn label_near<'a>(s: &'a Shapes, p: [f64; 2], within: f64, f: &dyn Fn(&str) -> bool) -> Option<&'a Txt> {
    s.texts.iter().filter(|t| f(&upper(&t.s)) && dist(t.at, p) < within).min_by(|a, b| dist(a.at, p).total_cmp(&dist(b.at, p)))
}

/// `3"` at the start of `s` (or `1 1/2"`) → inches.
fn lead_inches(s: &str) -> Option<f64> {
    let i = s.find('"')?;
    length_in(&format!("{}\"", s.get(..i)?.trim()))
}

fn centroid(pts: &[[f64; 2]]) -> [f64; 2] {
    let n = pts.len().max(1) as f64;
    let s = pts.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [s[0] / n, s[1] / n]
}

fn box_dist(lo: [f64; 2], hi: [f64; 2], p: [f64; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(0.0).max(p[0] - hi[0]);
    let dy = (lo[1] - p[1]).max(0.0).max(p[1] - hi[1]);
    dx.hypot(dy)
}

fn read_drainage(s: &Shapes, m: &mut Model, res: &mut EnvelopeResult) {
    for (_, lo, hi) in s.boxes.iter().filter(|b| b.0.contains("OVFL")).take(MAX_ITEMS) {
        m.overflows.push(Overflow { kind: "scupper".into(), at_ft: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0] });
    }
    for (_, c, _) in s.circles.iter().filter(|c| c.0.contains("OVFL")).take(MAX_ITEMS) {
        m.overflows.push(Overflow { kind: "drain".into(), at_ft: *c });
    }
    for (_, pts) in s.polys.iter().filter(|p| p.0.contains("CRKT")).take(MAX_ITEMS) {
        let near = |t: &Txt| pts.iter().map(|p| dist(*p, t.at)).fold(f64::MAX, f64::min);
        let label = s
            .texts
            .iter()
            .filter(|t| {
                let u = upper(&t.s);
                u == "CRICKET" || u == "SADDLE"
            })
            .filter(|t| near(t) < 3.0)
            .min_by(|a, b| near(a).total_cmp(&near(b)));
        let Some(label) = label else {
            res.notes.push(format!("a cricket-layer outline at ({:.1}, {:.1}) has no label", centroid(pts)[0], centroid(pts)[1]));
            continue;
        };
        let kind = if upper(&label.s) == "CRICKET" { "cricket" } else { "saddle" };
        let height_in = label_near(s, label.at, 2.0, &|u| u.ends_with("HIGH")).and_then(|t| lead_inches(&t.s)).unwrap_or(0.0);
        let c = centroid(pts);
        let at = if kind == "cricket" {
            m.curbs
                .iter()
                .min_by(|a, b| box_dist(a.min_ft, a.max_ft, c).total_cmp(&box_dist(b.min_ft, b.max_ft, c)))
                .map(|k| k.tag.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        m.ridges.push(RoofRidge { kind: kind.into(), at, outline: pts.clone(), height_in });
    }
    for (_, c, _) in s.circles.iter().filter(|c| c.0.contains("PENE")).take(MAX_ITEMS) {
        let t = label_near(s, *c, 3.0, &|u| u.contains("VTR") || u.contains("PIPE") || u.contains("VENT"));
        let (tag, size_in) = match t {
            Some(t) => {
                let u = upper(&t.s);
                (u.rsplit(' ').next().unwrap_or("VTR").to_string(), lead_inches(&u).unwrap_or(0.0))
            }
            None => ("PIPE".to_string(), 0.0),
        };
        m.pipes.push(RoofPipe { tag, at_ft: *c, size_in });
    }
    for (_, lo, hi) in s.boxes.iter().filter(|b| b.0.contains("WALK")).take(MAX_ITEMS) {
        m.walkways.push(Walkway { min_ft: *lo, max_ft: *hi });
    }
    for t in &s.texts {
        let u = upper(&t.s);
        if u.contains("COPING") {
            m.edge = "coping".into();
        } else if u.contains("EDGE") && u.contains("FASCIA") {
            m.edge = "edge_metal".into();
        }
        if u.contains("MIN. AT DRAINS") || u.contains("MIN AT DRAINS") {
            let head = u.split("MIN").next().unwrap_or_default();
            res.tapered_note_in = head.rsplit(',').next().and_then(lead_inches);
        }
    }
    // Detail callouts: a bubble on the detail layer with a number and a sheet in it.
    for (_, c, r) in s.circles.iter().filter(|c| c.0.contains("DETL")).take(MAX_ITEMS) {
        let inside: Vec<&Txt> = s.texts.iter().filter(|t| dist(t.at, *c) < *r).collect();
        let Some(number) = inside.iter().find_map(|t| t.s.trim().parse::<u32>().ok()) else { continue };
        let sheet = inside.iter().find(|t| t.s.contains('-')).map(|t| upper(t.s.trim())).unwrap_or_default();
        // The leader: a line starting at the bubble's edge and going away from it.
        let target = s
            .lines
            .iter()
            .filter(|l| l.0.contains("DETL"))
            .find_map(|(_, a, b)| {
                let (da, db) = (dist(*a, *c), dist(*b, *c));
                if (da - r).abs() < 0.1 * r && db > 1.3 * r {
                    Some(*b)
                } else if (db - r).abs() < 0.1 * r && da > 1.3 * r {
                    Some(*a)
                } else {
                    None
                }
            })
            .unwrap_or(*c);
        let mut best: (f64, String) = ((target[1] - m.roof_max[1]).abs().min((target[1] - m.roof_min[1]).abs()), "edge".into());
        let mut consider = |d: f64, subject: &str| {
            if d < best.0 {
                best = (d, subject.to_string());
            }
        };
        for k in &m.curbs {
            consider(dist(target, k.max_ft).min(box_dist(k.min_ft, k.max_ft, target) + 0.01), &k.tag);
        }
        for d in &m.drains {
            consider(dist(target, *d), "drain");
        }
        for p in &m.pipes {
            consider(dist(target, p.at_ft), "pipe");
        }
        m.callouts.push(Callout { number, sheet, at_ft: *c, subject: best.1 });
    }
}

// ---- The assemblies sheet ----------------------------------------------------------------------

fn component(words: &str) -> Option<&'static str> {
    let u = upper(words);
    let u = u.trim();
    Some(match u {
        _ if u.contains("MEMBRANE") || u == "ROOFING" => "membrane",
        _ if u.contains("COVER") => "cover_board",
        _ if u.contains("TAPERED") => "tapered",
        _ if u.contains("CONTINUOUS") || u == "CI" => "ci",
        _ if u.contains("VAPOR") => "vapor_retarder",
        _ if u.contains("INSULATION") => "insulation",
        _ if u.contains("DECK") => "deck",
        _ if u.contains("CLADDING") || u.contains("VENEER") || u.contains("SIDING") => "cladding",
        _ if u.contains("AIR SPACE") || u.contains("CAVITY") => "air_space",
        _ if u.contains("FURRING") || u.contains("GIRT") => "furring",
        _ if u.contains("BARRIER") || u.contains("WRB") => "barrier",
        _ if u.contains("SHEATHING") => "sheathing",
        _ => return None,
    })
}

/// A thickness cell: `(2) 2"`, `1/2" MIN.`, `60 MIL`, `--`, `1 1/2"` → (inches, count, mils).
fn thickness_cell(s: &str) -> (f64, u32, Option<f64>) {
    let u = upper(s);
    let u = u.trim();
    if let Some(m) = u.strip_suffix("MIL").or_else(|| u.strip_suffix("MILS")) {
        return (0.0, 1, number(m.trim()));
    }
    if let Some(rest) = u.strip_prefix('(')
        && let Some((n, t)) = rest.split_once(')')
    {
        return (lead_inches(t.trim()).unwrap_or(0.0), n.trim().parse().unwrap_or(1).clamp(1, 20), None);
    }
    (lead_inches(u).unwrap_or(0.0), 1, None)
}

fn membrane_kind(desc: &str) -> (String, String) {
    let u = upper(desc);
    let kind = if u.contains("TPO") {
        "tpo"
    } else if u.contains("PVC") {
        "pvc"
    } else if u.contains("EPDM") {
        "epdm"
    } else if u.contains("SBS") || u.contains("BITUMEN") {
        "sbs"
    } else {
        ""
    };
    let attachment =
        u.rsplit(", ").next().filter(|a| a.contains("ADHERED") || a.contains("FASTENED") || a.contains("WELDED") || a.contains("APPLIED"));
    (kind.into(), attachment.map(|a| a.to_ascii_lowercase()).unwrap_or_default())
}

fn table_layers(t: &crate::arch_reader::ReadTable, notes: &mut Vec<String>) -> (Vec<EnvLayer>, Option<f64>) {
    let kc = t.col(&|h| h.contains("COMPONENT") || h.contains("LAYER"));
    let kd = t.col(&|h| h.contains("DESCRIPTION") || h.contains("MATERIAL"));
    let kt = t.col(&|h| h.contains("THICK") || h.contains("THK"));
    let mut out = Vec::new();
    let mut mil = None;
    for row in &t.rows {
        let words = cell(row, kc);
        let Some(comp) = component(&words) else {
            if !upper(&words).contains("BACKUP") {
                notes.push(format!("{}: layer `{words}` not understood", t.title));
            }
            continue;
        };
        let (thickness_in, count, m) = thickness_cell(&cell(row, kt));
        if comp == "membrane" {
            mil = m;
        }
        out.push(EnvLayer { component: comp.into(), description: cell(row, kd), thickness_in, count });
    }
    (out, mil)
}

/// The tag in a title after `ASSEMBLY ` / `TYPE ` (`ROOF TYPE R-1 (TOP DOWN)` → `R-1`).
fn title_tag(title: &str) -> String {
    let u = upper(title);
    let rest = u.split_once("ASSEMBLY ").or_else(|| u.split_once("TYPE ")).map_or("", |(_, r)| r);
    rest.split([':', ' ']).next().unwrap_or_default().trim().to_string()
}

fn datum_code(name: &str) -> Option<String> {
    let u = upper(name);
    if u.contains("PARAPET") || u.contains("ROOF EDGE") {
        Some("P".into())
    } else if u.contains("ROOF") {
        Some("R".into())
    } else {
        u.split_once("LEVEL ").and_then(|(_, r)| r.split_whitespace().next()).map(String::from)
    }
}

fn read_assemblies(s: &Shapes, m: &mut Model, res: &mut EnvelopeResult) {
    // Datums first: the wall zones' levels come from them.
    for t in &s.texts {
        let u = upper(&t.s);
        if let Some((name, v)) = u.split_once(" = ")
            && let (Some(code), Some(inch)) = (datum_code(name), length_in(v))
        {
            m.datums.push(Datum { code, elevation_ft: inch / 12.0 });
        } else if let Some(v) = u.strip_prefix("EL.").or_else(|| u.strip_prefix("EL ")) {
            let above = s
                .texts
                .iter()
                .filter(|o| o.at[1] > t.at[1] && o.at[1] < t.at[1] + 2.5 * t.h && (o.at[0] - t.at[0]).abs() < t.h)
                .min_by(|a, b| a.at[1].total_cmp(&b.at[1]));
            if let (Some(code), Some(inch)) = (above.and_then(|a| datum_code(&a.s)), length_in(v)) {
                m.datums.push(Datum { code, elevation_ft: inch / 12.0 });
            }
        }
        if m.datums.len() > MAX_ITEMS {
            break;
        }
    }
    m.datums.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    m.datums.dedup_by(|a, b| a.code == b.code);
    let floors: Vec<String> = m.datums.iter().filter(|d| d.code != "R" && d.code != "P").map(|d| d.code.clone()).collect();
    for t in tables(s, &["ROOF ASSEMBLY", "ROOF TYPE", "WALL ASSEMBLY"]) {
        let title = upper(&t.title);
        let (layers, mil) = table_layers(&t, &mut res.notes);
        if title.starts_with("ROOF") {
            let (membrane, attachment) = layers.iter().find(|l| l.component == "membrane").map(|l| membrane_kind(&l.description)).unwrap_or_default();
            m.roof = RoofAssembly { tag: title_tag(&title), membrane, mil, attachment, layers };
            continue;
        }
        let zone = title.split_once(':').map_or("", |(_, z)| z.trim());
        let (levels, to_top) = if zone.contains("ALL LEVELS") {
            (floors.clone(), true)
        } else {
            let from = zone.split_once("LEVEL ").and_then(|(_, r)| r.split_whitespace().next()).unwrap_or_default().to_string();
            let to_top = zone.contains(" TO ");
            let levels: Vec<String> =
                if to_top || zone.contains("ABOVE") { floors.iter().skip_while(|f| **f != from).cloned().collect() } else { vec![from] };
            (levels, to_top)
        };
        let clad = layers.iter().find(|l| l.component == "cladding").map(|l| upper(&l.description)).unwrap_or_default();
        let kind = if clad.contains("BRICK") || clad.contains("MASONRY") {
            "brick"
        } else if clad.contains("SIDING") {
            "siding"
        } else if clad.contains("EIFS") {
            "eifs"
        } else {
            "panel"
        };
        m.claddings.push(Cladding { tag: title_tag(&title), kind: kind.into(), levels, to_top, layers });
    }
    // The detail index: a numbered bubble and its title to the right.
    for (_, c, r) in s.circles.iter().filter(|c| c.0.contains("DETL")).take(MAX_ITEMS) {
        let Some(n) = s.texts.iter().filter(|t| dist(t.at, *c) < *r).find_map(|t| t.s.trim().parse::<u32>().ok()) else { continue };
        let title = s
            .texts
            .iter()
            .filter(|t| t.at[0] > c[0] + r && t.at[0] < c[0] + 4.0 * r && (t.at[1] - c[1]).abs() < 1.5 * r && !upper(&t.s).starts_with("SCALE"))
            .min_by(|a, b| dist(a.at, *c).total_cmp(&dist(b.at, *c)));
        let Some(title) = title else { continue };
        let u = upper(&title.s);
        let subject = if u.contains("COPING") || u.contains("EDGE") {
            "edge".to_string()
        } else if u.contains("DRAIN") {
            "drain".into()
        } else if let Some((_, tag)) = u.split_once("CURB - ") {
            tag.trim().to_string()
        } else if u.contains("HATCH") {
            "ROOF HATCH".into()
        } else if u.contains("OVERRUN") {
            "ELEV. OVERRUN".into()
        } else if u.contains("PIPE") {
            "pipe".into()
        } else if u.contains("CRICKET") {
            "cricket".into()
        } else if u.contains("STOREFRONT") {
            "storefront".into()
        } else if u.contains("WINDOW") {
            "window".into()
        } else {
            String::new()
        };
        m.details.push(Detail { number: n, title: title.s.clone(), subject });
    }
    m.details.sort_by_key(|d| d.number);
}

// ---- The spec ----------------------------------------------------------------------------------

fn r_after(u: &str) -> Option<f64> {
    let (_, rest) = u.split_once("R-")?;
    let v: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    number(v.trim_end_matches('.'))
}

fn number_before(u: &str, key: &str) -> Option<f64> {
    let (head, _) = u.split_once(key)?;
    number(head.split_whitespace().last()?)
}

/// What the Division 07 spec asks for.
pub fn read_spec(text: &str) -> Option<EnvSpec> {
    let mut s = EnvSpec::default();
    let mut any = false;
    let mut section = String::new();
    for line in text.lines().take(20_000) {
        let u = upper(line);
        if u.starts_with("SECTION ") {
            section = u.clone();
            continue;
        }
        if u.contains("PER INCH")
            && let Some(r) = r_after(&u)
        {
            s.r_per_in = r;
            any = true;
        } else if u.contains("AVERAGE")
            && let Some(r) = r_after(&u)
        {
            s.min_r = r;
            any = true;
        }
        if (section.contains("ROOFING") || section.contains("MEMBRANE ROOFING")) && u.contains(" MIL") {
            s.membrane_mil = number_before(&u, " MIL");
            any = true;
        }
        if u.contains("GAUGE")
            && (u.contains("COPING") || u.contains("EDGE"))
            && let Some(g) = number_before(&u, " GAUGE")
        {
            s.metal_ga = g as u32;
            any = true;
        }
    }
    any.then_some(s)
}

// ---- Putting it together -----------------------------------------------------------------------

/// Read the envelope: `arch`, `walls` and `structure` are the other readers' results; `spec` the
/// Division 07 text, if there is one.
pub fn read_envelope(
    dxf: &[(String, Vec<u8>)],
    map: &SheetMap,
    arch: &ArchResult,
    walls: &WallResult,
    structure: &StructuralModel,
    spec: Option<&str>,
) -> EnvelopeResult {
    let mut res = EnvelopeResult::default();
    let mut m = Model::default();
    if let Some(rf) = &arch.roof {
        m.roof_min = rf.min_ft;
        m.roof_max = rf.max_ft;
        m.parapet_in = rf.parapet_in;
        m.slope_in_ft = rf.slope_in_ft.unwrap_or(0.0);
        m.drains = rf.drains.clone();
        m.curbs = rf
            .curbs
            .iter()
            .map(|c| Curb { tag: c.tag.clone(), kind: c.kind.clone(), min_ft: c.min_ft, max_ft: c.max_ft, height_in: c.height_in.unwrap_or(0.0) })
            .collect();
        m.edge = if rf.parapet_in.is_some() { "coping".into() } else { "edge_metal".into() };
    }
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" {
            continue;
        }
        let title = upper(&sh.title);
        let drainage = title.contains("DRAINAGE") || title.contains("FLASHING PLAN");
        let assemblies = title.contains("ASSEMBLIES");
        if !drainage && !assemblies {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        if drainage {
            let Some(al) = &sh.alignment else { continue };
            let al: Alignment = al.clone();
            let s = shapes(&d, &|p| sheet_reader::building_from_model(&al, p), 1.0 / 12.0);
            read_drainage(&s, &mut m, &mut res);
        } else {
            let s = shapes(&d, &|p| p, 1.0);
            read_assemblies(&s, &mut m, &mut res);
        }
    }
    // The tapered layer's least thickness: the legend's, else the plan note's.
    if let Some(n) = res.tapered_note_in
        && let Some(l) = m.roof.layers.iter_mut().find(|l| l.component == "tapered")
        && l.thickness_in <= 0.0
    {
        l.thickness_in = n;
    }
    // The exterior walls' outer face: the outermost walls of the lowest level, whatever type
    // they were read as (an untagged exterior wall may have been guessed wrong), each out by
    // half its thickness across it.
    let lowest = walls.walls.iter().map(|w| w.level.as_str()).min().unwrap_or("1").to_string();
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    for w in walls.walls.iter().filter(|w| w.level == lowest).take(MAX_ITEMS) {
        let h = w.thickness_in / 24.0;
        let along_x = (w.a_ft[1] - w.b_ft[1]).abs() < (w.a_ft[0] - w.b_ft[0]).abs();
        let (hx, hy) = if along_x { (0.0, h) } else { (h, 0.0) };
        for p in [w.a_ft, w.b_ft] {
            lo = [lo[0].min(p[0] - hx), lo[1].min(p[1] - hy)];
            hi = [hi[0].max(p[0] + hx), hi[1].max(p[1] + hy)];
        }
    }
    m.outline = (lo[0] < hi[0] && lo[1] < hi[1]).then_some((lo, hi));
    let on_outline = |p: [f64; 2]| {
        m.outline.is_some_and(|(lo, hi)| {
            let d = (p[0] - lo[0]).abs().min((p[0] - hi[0]).abs()).min((p[1] - lo[1]).abs()).min((p[1] - hi[1]).abs());
            d < 1.5
        })
    };
    let exterior_doors: Vec<&crate::arch_reader::FoundDoor> =
        arch.doors.iter().filter(|d| (d.from_room.is_none() || d.swings_into.is_none()) && on_outline(d.at_ft)).collect();
    for w in &arch.windows {
        if let Some(wt) = w.mark.as_ref().and_then(|mk| arch.window_types.iter().find(|x| &x.mark == mk)) {
            let kind = if wt.storefront { "storefront" } else { "window" };
            m.openings.push(EnvOpening { kind: kind.into(), level: w.level.clone(), width_in: wt.width_in, height_in: wt.height_in });
        }
    }
    for d in exterior_doors {
        let (w, h) = d.row.as_ref().map_or((Some(d.width_in), None), |r| (r.width_in.or(Some(d.width_in)), r.height_in));
        m.openings.push(EnvOpening { kind: "door".into(), level: d.level.clone(), width_in: w.unwrap_or(36.0), height_in: h.unwrap_or(84.0) });
    }
    if let Some(bs) = structure.beams.get("R") {
        m.beams = bs.iter().map(|b| (b.from_ft, b.to_ft, b.grid.clone())).collect();
    }
    m.spec = spec.and_then(read_spec);
    res.spec_read = m.spec.is_some();
    res.takeoff = takeoff(&m);
    res.flags = flags(&m);
    res.model = m;
    res
}

/// Generate a set and read everything the envelope reader builds on, then the envelope.
pub fn read_fixture_envelope(seed: u64) -> Result<(Truth, EnvelopeResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map);
    let drawings: Vec<(String, cadaddy_doc::Drawing)> =
        dxf.iter().filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
    let refs: Vec<(String, &cadaddy_doc::Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    let walls = crate::wall_reader::read_walls(&dxf, &map, &ceilings, &structure);
    let arch = crate::arch_reader::read_arch(&dxf, &map, &ceilings);
    let spec = set.file("specs.txt").map(|b| String::from_utf8_lossy(b).into_owned());
    Ok((set.truth.clone(), read_envelope(&dxf, &map, &arch, &walls, &structure, spec.as_deref())))
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct EnvScore {
    pub items_total: usize,
    /// Within 1% (or 0.05) of the truth's net and the same unit.
    pub items_exact: usize,
    pub items_missing: Vec<String>,
    pub items_extra: Vec<String>,
    pub items_off: Vec<(String, f64, f64)>,
    pub flags_total: usize,
    pub flags_found: usize,
    pub flags_false: Vec<String>,
    pub roof_layers_ok: bool,
    pub claddings_ok: bool,
    pub ridges: (usize, usize),
    pub overflows: (usize, usize),
    pub pipes: (usize, usize),
    pub walkways: (usize, usize),
    pub callouts: (usize, usize),
    pub details: (usize, usize),
    pub datums: (usize, usize),
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn layers_match(a: &[EnvLayer], b: &[EnvLayer]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.component == y.component && x.count == y.count && near(x.thickness_in, y.thickness_in, 0.01))
}

pub fn score_envelope(t: &Truth, r: &EnvelopeResult) -> EnvScore {
    let mut s = EnvScore::default();
    let Some(e) = t.envelope.as_ref() else { return s };
    let m = &r.model;
    s.items_total = e.takeoff.len();
    for it in &e.takeoff {
        match r.takeoff.iter().find(|x| x.code == it.code) {
            Some(x) if x.unit == it.unit && near(x.net, it.net, (it.net.abs() * 0.01).max(0.05)) => s.items_exact += 1,
            Some(x) => s.items_off.push((it.code.clone(), it.net, x.net)),
            None => s.items_missing.push(it.code.clone()),
        }
    }
    s.items_extra = r.takeoff.iter().filter(|x| !e.takeoff.iter().any(|i| i.code == x.code)).map(|x| x.code.clone()).collect();
    s.flags_total = e.flags.len();
    s.flags_found = e.flags.iter().filter(|f| r.flags.iter().any(|g| g.kind == f.kind && g.subject == f.subject)).count();
    s.flags_false = r
        .flags
        .iter()
        .filter(|g| !e.flags.iter().any(|f| f.kind == g.kind && f.subject == g.subject))
        .map(|g| format!("{} {}", g.kind, g.subject))
        .collect();
    s.roof_layers_ok = m.roof.tag == e.roof.tag
        && m.roof.membrane == e.roof.membrane
        && m.roof.mil == e.roof.mil
        && m.roof.attachment == e.roof.attachment
        && layers_match(&m.roof.layers, &e.roof.layers);
    s.claddings_ok = m.claddings.len() == e.claddings.len()
        && m.claddings
            .iter()
            .zip(&e.claddings)
            .all(|(a, b)| a.tag == b.tag && a.kind == b.kind && a.levels == b.levels && a.to_top == b.to_top && layers_match(&a.layers, &b.layers));
    let found = |n: usize, f: &dyn Fn(usize) -> bool| (0..n).filter(|i| f(*i)).count();
    s.ridges = (
        e.ridges.len(),
        found(e.ridges.len(), &|i| {
            e.ridges.get(i).is_some_and(|x| {
                m.ridges.iter().any(|y| {
                    y.kind == x.kind && y.at == x.at && near(y.height_in, x.height_in, 0.01) && dist(centroid(&y.outline), centroid(&x.outline)) < 0.1
                })
            })
        }),
    );
    s.overflows = (
        e.overflows.len(),
        found(e.overflows.len(), &|i| {
            e.overflows.get(i).is_some_and(|x| m.overflows.iter().any(|y| y.kind == x.kind && dist(y.at_ft, x.at_ft) < 0.1))
        }),
    );
    s.pipes = (
        e.pipes.len(),
        found(e.pipes.len(), &|i| {
            e.pipes.get(i).is_some_and(|x| m.pipes.iter().any(|y| y.tag == x.tag && near(y.size_in, x.size_in, 0.01) && dist(y.at_ft, x.at_ft) < 0.1))
        }),
    );
    s.walkways = (
        e.walkways.len(),
        found(e.walkways.len(), &|i| {
            e.walkways.get(i).is_some_and(|x| m.walkways.iter().any(|y| dist(y.min_ft, x.min_ft) < 0.1 && dist(y.max_ft, x.max_ft) < 0.1))
        }),
    );
    s.callouts = (
        e.callouts.len(),
        found(e.callouts.len(), &|i| {
            e.callouts.get(i).is_some_and(|x| m.callouts.iter().any(|y| y.number == x.number && y.sheet == x.sheet && y.subject == x.subject))
        }),
    );
    s.details = (
        e.details.len(),
        found(e.details.len(), &|i| e.details.get(i).is_some_and(|x| m.details.iter().any(|y| y.number == x.number && y.subject == x.subject))),
    );
    s.datums = (
        e.datums.len(),
        found(e.datums.len(), &|i| {
            e.datums.get(i).is_some_and(|x| m.datums.iter().any(|y| y.code == x.code && near(y.elevation_ft, x.elevation_ft, 0.02)))
        }),
    );
    s
}

// ---- Building spec -----------------------------------------------------------------------------

fn assembly_layer(l: &EnvLayer, roof: bool) -> Value {
    let (side, category) = match l.component.as_str() {
        "membrane" => ("side1", "membrane"),
        "cover_board" => ("side1", "board"),
        "tapered" | "insulation" | "ci" => ("core", "insulation"),
        "vapor_retarder" | "barrier" => ("core", "membrane"),
        "deck" => ("side2", "deck"),
        "cladding" => ("side1", if roof { "membrane" } else { "cladding" }),
        "furring" | "air_space" => ("core", "furring"),
        "sheathing" => ("core", "sheathing"),
        _ => ("core", ""),
    };
    let material = if l.count > 1 { format!("({}) {}", l.count, l.description) } else { l.description.clone() };
    json!({"material": material, "thickness_in": l.thickness_in * l.count.max(1) as f64, "side": side, "category": category})
}

/// The building spec patch for what was read: the roof with its assembly (and parapet), its
/// crickets and saddles, and a material band per cladding zone on every facade, with the zone's
/// assembly. `to_plan` maps the roof level's building feet to its plan drawing's coordinates
/// (none: the roof isn't built, so it is left out).
pub fn spec_patch(r: &EnvelopeResult, to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let m = &r.model;
    let mut ext = json!({});
    let corners = [m.roof_min, [m.roof_max[0], m.roof_min[1]], m.roof_max, [m.roof_min[0], m.roof_max[1]]];
    let outline: Option<Vec<[f64; 2]>> = corners.iter().map(|p| to_plan("R", *p)).collect();
    if let Some(outline) = outline
        && m.roof_max[0] > m.roof_min[0]
    {
        let mut notes = Vec::new();
        if let Some(l) = m.roof.layers.iter().find(|l| l.component == "tapered") {
            notes.push(format!("tapered insulation {:.3} in/ft to the drains, {} in least", m.slope_in_ft, l.thickness_in));
        }
        if !m.roof.attachment.is_empty() {
            notes.push(format!("membrane {}", m.roof.attachment));
        }
        let mut roof = json!({
            "name": if m.roof.tag.is_empty() { "R-1".to_string() } else { m.roof.tag.clone() },
            "level": "R",
            "outline": outline,
            "assembly": {"tag": m.roof.tag, "description": "Roof assembly", "layers": m.roof.layers.iter().map(|l| assembly_layer(l, true)).collect::<Vec<_>>(), "notes": notes},
        });
        if let Some(p) = m.parapet_in {
            roof["parapet_ft"] = json!(p / 12.0);
        }
        ext["roofs"] = json!([roof]);
        let crickets: Vec<Value> = m
            .ridges
            .iter()
            .filter_map(|rd| {
                let pts: Option<Vec<[f64; 2]>> = rd.outline.iter().map(|p| to_plan("R", *p)).collect();
                pts.map(|pts| json!({"level": "R", "outline": pts, "height_ft": rd.height_in / 12.0}))
            })
            .collect();
        if !crickets.is_empty() {
            ext["crickets"] = json!(crickets);
        }
    }
    let base = m.datums.iter().find(|d| d.code != "P" && d.code != "R").map_or(0.0, |d| d.elevation_ft);
    let mut bands = Vec::new();
    for c in &m.claddings {
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        let mut order: Vec<&Datum> = m.datums.iter().filter(|d| d.code != "P").collect();
        order.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
        for l in &c.levels {
            if let Some(i) = order.iter().position(|d| &d.code == l)
                && let (Some(a), Some(b)) = (order.get(i), order.get(i + 1))
            {
                lo = lo.min(a.elevation_ft);
                hi = hi.max(b.elevation_ft);
            }
        }
        if c.to_top
            && let Some(p) = m.datums.iter().find(|d| d.code == "P")
        {
            hi = hi.max(p.elevation_ft);
        }
        if lo >= hi {
            continue;
        }
        let words = c.layers.iter().find(|l| l.component == "cladding").map_or_else(|| c.kind.clone(), |l| l.description.clone());
        bands.push(json!({
            "kind": "material",
            "from_ft": lo - base,
            "to_ft": hi - base,
            "material": words,
            "assembly": {"tag": c.tag, "description": format!("Exterior wall assembly {}", c.tag), "external": true, "layers": c.layers.iter().map(|l| assembly_layer(l, false)).collect::<Vec<_>>()},
        }));
    }
    if !bands.is_empty() {
        ext["facades"] = json!(["north", "south", "east", "west"].iter().map(|s| json!({"side": s, "bands": bands})).collect::<Vec<_>>());
    }
    json!({"exterior": ext})
}
