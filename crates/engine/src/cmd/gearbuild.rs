//! Electrical gear and feeders from the prints in the common "feeder types" style, for the
//! existing gear checks (`gear.check`/`gear.review`): a print set whose feeder schedule lists
//! feeder TYPES (tag → conduits and conductors, no from/to), whose one-line tags each piece of
//! gear's incoming feeder with a type, and whose gear sizes and lugs are only in the submittal.
//! (`cadaddy_mepfix::gear_reader` reads the other style: an equipment schedule with sizes, a lug
//! schedule, a feeder schedule with FROM and TO.)
//!
//! - [`feeder_types`]: a `FEEDER SCHEDULE` table read by its columns' names: tag, (quantity)
//!   conduit size, (quantity) conductor size, ground; aluminium where the header says `AL UNO`
//!   unless a row says `(CU)`, else copper.
//! - [`oneline_incoming`]: on a one-line, each gear label's incoming feeder is the feeder-type
//!   tag just above it.
//! - [`submittal_gear`]: per panel tag the schedules name, the gear submittal's box size
//!   (`50 X 20 X 5.75`: the smallest the depth, the largest the height), feed (top or bottom),
//!   main lug range (`(2)3/0-500KCMIL`) and branch lug range.
//! - [`place_along_walls`]: gear the plans don't locate is laid out in the room its schedule
//!   names, along the room's walls with its working space in front (NEC 110.26, the rule as data
//!   in `coderules.json`), each marked proposed.
//!
//! Every value records where it came from; nothing is known about one job.

use std::collections::BTreeMap;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

/// One feeder type of a feeder schedule.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FeederType {
    pub tag: String,
    pub conduits: u32,
    pub conduit_in: f64,
    /// Conductors per conduit (phases and neutral) and their size (`500 kcmil`, `2/0`, `#4`).
    pub conductors: u32,
    pub size: String,
    pub material: String,
    pub ground_size: String,
    pub source: String,
}

/// Inches from a trade size: `1-1/4"`, `2"`, `4"C`, `1 1/2`.
pub(crate) fn trade_in(v: &str) -> Option<f64> {
    let t = v.trim().trim_end_matches(['C', 'c']).trim_end_matches('"').trim();
    let (whole, frac) = match t.split_once(['-', ' ']) {
        Some((w, f)) if f.contains('/') => (w.trim(), Some(f.trim())),
        _ if t.contains('/') => ("0", Some(t)),
        _ => (t, None),
    };
    let w: f64 = whole.parse().ok()?;
    let f = match frac {
        Some(f) => {
            let (a, b) = f.split_once('/')?;
            a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok().filter(|b| *b > 0.0)?
        }
        None => 0.0,
    };
    Some(w + f).filter(|x| x.is_finite() && *x > 0.0 && *x <= 8.0)
}

/// `(4) #500kcmil (CU)` → (4, `500 kcmil`, Some(cu)); `#8G (CU)` → (1, `8`, Some(cu)).
fn conductors(v: &str) -> Option<(u32, String, Option<String>)> {
    let u = v.trim().to_ascii_uppercase();
    let (n, rest) = match u.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((n, r)) => (n.trim().parse::<u32>().ok()?, r.trim().to_string()),
        None => (1, u.clone()),
    };
    let material = if rest.contains("(CU)") || rest.contains(" CU") {
        Some("cu".to_string())
    } else if rest.contains("(AL)") || rest.contains(" AL") {
        Some("al".to_string())
    } else {
        None
    };
    let core = rest.split('(').next().unwrap_or_default().trim().trim_start_matches('#').trim_end_matches('G').trim().to_string();
    let size = if let Some(k) = core.strip_suffix("KCMIL").or_else(|| core.strip_suffix("MCM")) {
        format!("{} kcmil", k.trim())
    } else {
        core.to_ascii_lowercase()
    };
    (!size.is_empty() && size.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some((n, size, material))
}

/// The feeder types of the `FEEDER SCHEDULE` tables on a sheet's texts.
pub(crate) fn feeder_types(texts: &[TextItem], sheet: &str) -> Vec<FeederType> {
    let mut out = Vec::new();
    for title in texts.iter().filter(|t| t.value.trim().eq_ignore_ascii_case("FEEDER SCHEDULE")) {
        let h = title.height.max(1e-9);
        // The header: texts under the title naming the columns; its lowest line is the last.
        let near: Vec<&TextItem> =
            texts.iter().filter(|t| t.at.y < title.at.y && t.at.y > title.at.y - 12.0 * h && (t.at.x - title.at.x).abs() < 40.0 * h).collect();
        let col = |words: &[&str]| -> Option<&TextItem> {
            near.iter()
                .copied()
                .filter(|t| {
                    let u = t.value.to_ascii_uppercase();
                    words.iter().all(|w| u.contains(w)) && (words.contains(&"ISOLATED") || !u.contains("ISOLATED"))
                })
                .max_by(|a, b| a.at.y.total_cmp(&b.at.y))
        };
        // The column that names the size (a spanning `EACH CONDUIT` above several isn't one).
        let conduit = col(&["CONDUIT", "SIZE"]).or_else(|| col(&["CONDUIT"]).filter(|t| !t.value.to_ascii_uppercase().contains("EACH")));
        let (Some(tag), Some(conduit), Some(cond)) = (col(&["TAG"]), conduit, col(&["CONDUCTOR"])) else { continue };
        let ground = col(&["GROUND"]);
        let isolated = col(&["ISOLATED"]);
        let al_default = near.iter().any(|t| t.value.to_ascii_uppercase().replace(' ', "").contains("ALUNO"));
        // The header's lowest line: its column names' (not the first row's cells).
        let header_low = [Some(tag), Some(conduit), Some(cond), ground].into_iter().flatten().map(|t| t.at.y).fold(f64::INFINITY, f64::min);
        // Rows: lines below the header, each starting with a tag under the TAG column.
        let mut cols: Vec<(f64, u8)> = vec![(tag.at.x, 0), (conduit.at.x, 1), (cond.at.x, 2)];
        if let Some(g) = ground {
            cols.push((g.at.x, 3));
        }
        if let Some(g) = isolated {
            cols.push((g.at.x, 9));
        }
        cols.sort_by(|a, b| a.0.total_cmp(&b.0));
        let x0 = cols.first().map_or(0.0, |c| c.0) - 6.0 * h;
        let x1 = cols.last().map_or(0.0, |c| c.0) + 20.0 * h;
        let mut rows: Vec<&TextItem> =
            texts.iter().filter(|t| t.at.y < header_low - 0.3 * h && t.at.y > header_low - 60.0 * h && t.at.x > x0 && t.at.x < x1).collect();
        rows.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        let mut lines: Vec<Vec<&TextItem>> = Vec::new();
        for t in rows {
            match lines.last_mut() {
                Some(l) if l.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.5 * h) => l.push(t),
                _ => lines.push(vec![t]),
            }
        }
        // A cell is the column whose header is nearest (headers centred, cells left-aligned).
        let column = |x: f64| cols.iter().min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs())).map(|c| c.1);
        for l in &lines {
            let mut cells: BTreeMap<u8, String> = BTreeMap::new();
            for t in l {
                if let Some(c) = column(t.at.x) {
                    let e = cells.entry(c).or_default();
                    if !e.is_empty() {
                        e.push(' ');
                    }
                    e.push_str(t.value.trim());
                }
            }
            let tagv = cells.get(&0).map(|v| v.trim().to_string()).unwrap_or_default();
            if tagv.is_empty() || tagv.len() > 8 || !tagv.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                // A note or the table's end.
                if !out.is_empty() && !cells.contains_key(&1) {
                    break;
                }
                continue;
            }
            let Some((nc, csize)) = cells.get(&1).and_then(|v| {
                let u = v.trim();
                let (n, s) = match u.strip_prefix('(').and_then(|r| r.split_once(')')) {
                    Some((n, r)) => (n.trim().parse::<u32>().ok()?, r),
                    None => (1, u),
                };
                Some((n, trade_in(s)?))
            }) else {
                continue;
            };
            let Some((n, size, mat)) = cells.get(&2).and_then(|v| conductors(v)) else { continue };
            let ground_size = cells.get(&3).and_then(|v| conductors(v)).map(|g| g.1).unwrap_or_default();
            out.push(FeederType {
                tag: tagv,
                conduits: nc,
                conduit_in: csize,
                conductors: n,
                size,
                material: mat.unwrap_or_else(|| if al_default { "al".into() } else { "cu".into() }),
                ground_size,
                source: format!("FEEDER SCHEDULE {sheet}"),
            });
        }
    }
    out
}

/// On a one-line's texts: each gear label (`tags`) and the feeder-type tag just above it (its
/// incoming feeder). A label a gear tag names more than once (a typical panel drawn twice) gives
/// one answer when they agree.
pub(crate) fn oneline_incoming(texts: &[TextItem], tags: &[String], types: &[String]) -> BTreeMap<String, String> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for g in texts.iter().filter(|t| tags.iter().any(|x| x.eq_ignore_ascii_case(t.value.trim()))) {
        let h = g.height.max(1e-9);
        let above = texts
            .iter()
            .filter(|t| types.iter().any(|x| x.eq_ignore_ascii_case(t.value.trim())))
            .filter(|t| t.at.y > g.at.y && t.at.y - g.at.y < 10.0 * h && (t.at.x - g.at.x).abs() < 3.0 * h)
            .min_by(|a, b| (a.at.y - g.at.y).total_cmp(&(b.at.y - g.at.y)));
        if let Some(t) = above {
            seen.entry(g.value.trim().to_ascii_uppercase()).or_default().push(t.value.trim().to_string());
        }
    }
    seen.into_iter()
        .filter_map(|(k, v)| {
            let first = v.first()?.clone();
            v.iter().all(|x| *x == first).then_some((k, first))
        })
        .collect()
}

/// What a gear submittal says of one panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubmittedGear {
    /// Height, width, depth, inches.
    pub size_in: [f64; 3],
    /// `top` or `bottom`.
    pub feed: String,
    /// Lugs per phase and their range (`3/0-500 kcmil`).
    pub main_lugs: Option<(u32, String)>,
    pub branch_lug: Option<String>,
    /// Its conduit entry areas as the submittal draws them (width × depth, inches), when it does.
    pub top_plate_in: Option<[f64; 2]>,
    pub bottom_plate_in: Option<[f64; 2]>,
    /// `switchboard` when the submittal says so (else a panelboard).
    pub kind: String,
    pub source: String,
}

fn dims(t: &str) -> Option<[f64; 3]> {
    let u = t.to_ascii_uppercase();
    let mut nums: Vec<f64> = Vec::new();
    for part in u.split('X').take(3) {
        let p = part.trim();
        let tail: String = p.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
        let head: String = p.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let v = if nums.is_empty() { tail } else { head };
        nums.push(v.parse::<f64>().ok().filter(|x| *x > 1.0 && *x < 200.0)?);
    }
    let mut s = nums.clone();
    if s.len() != 3 {
        return None;
    }
    s.sort_by(f64::total_cmp);
    // The smallest the depth, the largest the height (a panel stands taller than it is wide).
    Some([s[2], s[1], s[0]])
}

/// A lug's wire range: `3/0-500KCMIL`, `#14-3/0`, `#6-350KCMIL` (each end a size).
fn lug_range(v: &str) -> bool {
    let size = |s: &str| {
        let s = s.trim().trim_start_matches('#');
        let s = s.strip_suffix("KCMIL").unwrap_or(s);
        !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '/')
    };
    v.split_once('-').is_some_and(|(a, b)| size(a) && size(b))
}

/// A gear elevation drawing read by position (a drawing's text has no reading order): its height
/// (the inch ruler's top mark, `90.00"`, three marks at least), its width (the number beside each
/// `SECTION#` label, 12" or more, added), its depth and conduit areas (each `... CONDUIT` plan's
/// dimension labels, those nearer a `RESTRICTED` label left out: the largest the section's depth,
/// the one in line with it the area's depth, the largest left the area's width; `TOP`/`BOTTOM`
/// beside the label which way conduit comes in).
pub(crate) fn elevation_at(texts: &[TextItem]) -> Option<SubmittedGear> {
    // A plan's dimension labels lie this close to its label (text heights): the elevation's own
    // numbers (unit spaces) are farther.
    const PLAN_REACH: f64 = 25.0;
    let num = |t: &TextItem| {
        let v = t.value.trim();
        (v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.trim_end_matches('"').chars().all(|c| c.is_ascii_digit() || c == '.'))
            .then(|| v.trim_end_matches('"').parse::<f64>().ok())
            .flatten()
            .filter(|x| x.is_finite())
    };
    let ruler: Vec<f64> = texts.iter().filter(|t| t.value.trim().ends_with('"')).filter_map(num).collect();
    if ruler.len() < 3 {
        return None;
    }
    let height = ruler.iter().copied().fold(0.0, f64::max);
    // The drawing's text height (the median: a stray tiny or huge text doesn't set the scale).
    let mut hs: Vec<f64> = texts.iter().map(|t| t.height).filter(|v| v.is_finite() && *v > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    let h = hs.get(hs.len() / 2).copied().unwrap_or(1.0);
    let labels: Vec<&TextItem> = texts.iter().filter(|t| t.value.to_ascii_uppercase().starts_with("SECTION#")).collect();
    let mut widths: Vec<(f64, Vec2)> = Vec::new();
    for l in &labels {
        let w = texts
            .iter()
            .filter(|t| !t.value.trim().ends_with('"'))
            .filter_map(|t| num(t).filter(|v| (12.0..=120.0).contains(v)).map(|v| (v, t.at)))
            .filter(|(_, at)| at.dist(l.at) < 8.0 * h)
            .min_by(|a, b| a.1.dist(l.at).total_cmp(&b.1.dist(l.at)));
        if let Some(w) = w.filter(|w| !widths.iter().any(|x| x.1.dist(w.1) < 1e-6)) {
            widths.push(w);
        }
    }
    if widths.is_empty() {
        return None;
    }
    let conduit: Vec<&TextItem> = texts.iter().filter(|t| t.value.to_ascii_uppercase().contains("CONDUIT")).collect();
    let restricted: Vec<&TextItem> = texts.iter().filter(|t| t.value.to_ascii_uppercase().contains("RESTRICTED")).collect();
    let dims: Vec<(f64, Vec2)> = texts
        .iter()
        .filter(|t| !t.value.trim().ends_with('"'))
        .filter_map(|t| num(t).filter(|v| *v >= 6.0 && *v <= 120.0).map(|v| (v, t.at)))
        .filter(|(v, at)| !widths.iter().any(|w| w.1.dist(*at) < 1e-6 && (w.0 - v).abs() < 1e-9))
        .collect();
    let (mut top, mut bottom, mut depth) = ([0.0f64, 0.0f64], [0.0f64, 0.0f64], 0.0f64);
    for (ci, c) in conduit.iter().enumerate() {
        // This plan's numbers: those nearest this label of all the conduit and restricted labels.
        let mine: Vec<(f64, Vec2)> = dims
            .iter()
            .copied()
            .filter(|(_, at)| {
                let d = at.dist(c.at);
                d < PLAN_REACH * h
                    && conduit.iter().enumerate().all(|(j, o)| j == ci || at.dist(o.at) >= d)
                    // A restricted area's own sizes sit right beside its label.
                    && restricted.iter().all(|r| at.dist(r.at) >= 6.0 * h)
            })
            .collect();
        let Some(&(sd, sd_at)) = mine.iter().max_by(|a, b| a.0.total_cmp(&b.0)) else { continue };
        let in_line = |a: Vec2, b: Vec2| (a.x - b.x).abs() < 0.6 * h || (a.y - b.y).abs() < 0.6 * h;
        let ad = mine.iter().filter(|(v, at)| *v < sd && in_line(*at, sd_at)).map(|x| x.0).fold(0.0, f64::max);
        let aw = mine.iter().filter(|(v, at)| *v < sd && !(in_line(*at, sd_at) && (*v - ad).abs() < 1e-9)).map(|x| x.0).fold(0.0, f64::max);
        if ad <= 0.0 || aw <= 0.0 {
            continue;
        }
        depth = depth.max(sd);
        // The way conduit comes in, in the label or beside it (`TOP & BOTTOM`, `BOTTOM ONLY`).
        let near = |w: &str| {
            texts
                .iter()
                .any(|t| t.at.dist(c.at) < 4.0 * h && t.value.to_ascii_uppercase().split(|ch: char| !ch.is_ascii_alphabetic()).any(|x| x == w))
        };
        if near("TOP") {
            top = [top[0] + aw, top[1].max(ad)];
        }
        if near("BOTTOM") {
            bottom = [bottom[0] + aw, bottom[1].max(ad)];
        }
    }
    (depth > 0.0).then(|| SubmittedGear {
        size_in: [height, widths.iter().map(|w| w.0).sum(), depth],
        top_plate_in: (top[0] > 0.0).then_some(top),
        bottom_plate_in: (bottom[0] > 0.0).then_some(bottom),
        kind: "switchboard".into(),
        ..SubmittedGear::default()
    })
}

/// Per panel tag (`tags`), what the gear submittal's text says: the text after each designation
/// (`<tag> - 1`, `<tag>--PANELBOARD`), up to the next designation.
pub(crate) fn submittal_gear(text: &str, tags: &[String], source: &str) -> BTreeMap<String, SubmittedGear> {
    // One line of words; a range broken at its dash (`(1)#6- 350KCMIL`) joined again.
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ").replace("- ", "-").replace(" -", "-").replace("-1 ", " - 1 ");
    let up = flat.to_ascii_uppercase();
    let mut out: BTreeMap<String, SubmittedGear> = BTreeMap::new();
    for tag in tags {
        let t = tag.to_ascii_uppercase();
        let marks = [format!(" {t} - 1 "), format!(" {t}--"), format!(" {t}-PANELBOARD")];
        let mut g = SubmittedGear { source: source.to_string(), ..SubmittedGear::default() };
        for m in &marks {
            let mut from = 0;
            while let Some(i) = up.get(from..).and_then(|s| s.find(m.as_str())).map(|i| i + from) {
                from = i + m.len();
                let end = (i + 700).min(up.len());
                let Some(win) = up.get(i + m.len()..end) else { break };
                // Up to the next designation (another tag's).
                let stop = tags
                    .iter()
                    .filter(|o| !o.eq_ignore_ascii_case(tag))
                    .flat_map(|o| [format!(" {} - 1 ", o.to_ascii_uppercase()), format!(" {}--PANELBOARD", o.to_ascii_uppercase())])
                    .filter_map(|s| win.find(&s))
                    .min()
                    .unwrap_or(win.len());
                let win = win.get(..stop).unwrap_or(win);
                if g.size_in == [0.0; 3] {
                    let words: Vec<&str> = win.split(' ').collect();
                    for k in 0..words.len().saturating_sub(3) {
                        if words.get(k + 1) == Some(&"X") && words.get(k + 3) == Some(&"X") {
                            // The third size next, else (a table that wraps) the next decimal or
                            // small number a few words on.
                            let third = words
                                .iter()
                                .skip(k + 4)
                                .take(12)
                                .copied()
                                .find(|w| w.parse::<f64>().is_ok_and(|v| v > 1.0 && (w.contains('.') || v <= 12.0)))
                                .unwrap_or_default();
                            let s = format!("{} X {} X {third}", words[k], words[k + 2]);
                            if let Some(d) = dims(&s) {
                                g.size_in = d;
                                break;
                            }
                        }
                    }
                }
                if g.feed.is_empty() {
                    // Said outright first (`Incoming Location: Bottom`, `Top Feed`), then a bare word.
                    g.feed = if win.contains("INCOMING LOCATION:BOTTOM") || win.contains("INCOMING LOCATION: BOTTOM") || win.contains("BOTTOM FEED") {
                        "bottom".into()
                    } else if win.contains("INCOMING LOCATION:TOP")
                        || win.contains("INCOMING LOCATION: TOP")
                        || win.contains("TOP FEED")
                        || win.contains(" TOP ")
                    {
                        "top".into()
                    } else if win.contains(" BOTTOM ") {
                        "bottom".into()
                    } else {
                        String::new()
                    };
                }
                if g.kind.is_empty() && win.contains("SWITCHBOARD") {
                    g.kind = "switchboard".into();
                }
                // A switchboard's incoming lugs: `INCOMING LUGS: (6) 600 KCMIL`.
                if g.main_lugs.is_none()
                    && let Some(i) = win.find("INCOMING LUGS:")
                {
                    let rest: Vec<&str> = win.get(i + 14..).unwrap_or_default().split(' ').filter(|w| !w.is_empty()).take(3).collect();
                    if let (Some(n), Some(size)) = (rest.first().and_then(|w| w.trim_matches(['(', ')']).parse::<u32>().ok()), rest.get(1)) {
                        let unit = rest.get(2).filter(|u| u.starts_with("KCMIL") || u.starts_with("MCM")).map_or("", |_| " kcmil");
                        g.main_lugs = Some((n, format!("{}{unit}", size.to_ascii_lowercase())));
                    }
                }
                if g.main_lugs.is_none()
                    && let Some(p) = win.find(")").filter(|_| win.contains("MAIN LUG") || win.contains(" MLO ")).and_then(|_| {
                        // `(2)3/0-500KCMIL` (possibly split: `(2)3/0- 500KCMIL`).
                        win.split(' ')
                            .filter_map(|x| x.find('(').and_then(|p| x.get(p..)))
                            .find(|x| x.contains(')') && x.contains('-') && x.contains("KCMIL") && lug_range(x.split(')').nth(1).unwrap_or_default()))
                            .map(str::to_string)
                    })
                {
                    let n = p.trim_start_matches('(').split(')').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(1);
                    let range = p.split(')').nth(1).unwrap_or_default().replace("KCMIL", " kcmil").to_ascii_lowercase();
                    g.main_lugs = Some((n, range));
                }
                // A branch breaker's lug: `(1)#14-3/0 ... LUG`.
                if g.branch_lug.is_none()
                    && let Some(x) = win.split(' ').find(|x| x.starts_with("(1)#") && !x.contains("KCMIL") && lug_range(x.trim_start_matches("(1)")))
                {
                    g.branch_lug = Some(x.trim_start_matches("(1)").to_ascii_lowercase());
                }
            }
        }
        if g.size_in != [0.0; 3] || g.main_lugs.is_some() || !g.feed.is_empty() {
            out.insert(tag.clone(), g);
        }
    }
    out
}

/// One piece of gear laid along a room's wall: its plan box (corners), the wall it backs on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placed {
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// The working space in front: plan box.
    pub space_min: [f64; 2],
    pub space_max: [f64; 2],
}

fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

/// Gear of `widths` × `depths` (drawing units) along an axis-aligned room outline's walls, in
/// order, each with a working space of `space_w` (at least the gear's width) × `space_d` in front,
/// kept inside the room and clear of the gear already placed. The longest walls first; `gap`
/// between pieces. Gear that doesn't fit is `None`.
pub(crate) fn place_along_walls(outline: &[Vec2], sizes: &[(f64, f64)], space_w: f64, space_d: f64, gap: f64) -> Vec<Option<Placed>> {
    let n = outline.len();
    let mut edges: Vec<(Vec2, Vec2)> = (0..n).map(|i| (outline[i], outline[(i + 1) % n])).filter(|(a, b)| a.dist(*b) > 1e-9).collect();
    edges.sort_by(|a, b| b.0.dist(b.1).total_cmp(&a.0.dist(a.1)));
    let mut taken: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let clear = |lo: [f64; 2], hi: [f64; 2], taken: &[([f64; 2], [f64; 2])]| {
        taken.iter().all(|(a, b)| hi[0] <= a[0] + 1e-9 || lo[0] >= b[0] - 1e-9 || hi[1] <= a[1] + 1e-9 || lo[1] >= b[1] - 1e-9)
    };
    let mut out = Vec::new();
    for &(w, d) in sizes {
        let mut got = None;
        'edges: for (a, b) in &edges {
            let len = a.dist(*b);
            let dir = Vec2::new((b.x - a.x) / len, (b.y - a.y) / len);
            // Square walls only (axis-aligned boxes).
            if dir.x.abs() > 1e-6 && dir.y.abs() > 1e-6 {
                continue;
            }
            // Inward: the side of the wall the room is on.
            let mid = Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            let left = Vec2::new(-dir.y, dir.x);
            let inward = if inside(outline, mid + left * (gap.max(1e-3))) { left } else { left * -1.0 };
            let sw = space_w.max(w);
            let mut s = gap;
            while s + sw <= len + 1e-9 {
                let p0 = *a + dir * s;
                let corners = |off0: f64, off1: f64, along: f64| {
                    let q0 = p0 + inward * off0;
                    let q1 = p0 + dir * along + inward * off1;
                    ([q0.x.min(q1.x), q0.y.min(q1.y)], [q0.x.max(q1.x), q0.y.max(q1.y)])
                };
                let (gmin, gmax) = corners(0.0, d, w);
                let (smin, smax) = corners(d, d + space_d, sw);
                let ok = [
                    Vec2::new(smin[0] + 1e-6, smin[1] + 1e-6),
                    Vec2::new(smax[0] - 1e-6, smax[1] - 1e-6),
                    Vec2::new(smin[0] + 1e-6, smax[1] - 1e-6),
                    Vec2::new(smax[0] - 1e-6, smin[1] + 1e-6),
                ]
                .iter()
                .all(|c| inside(outline, *c));
                if ok && clear(gmin, gmax, &taken) && clear(smin, smax, &taken) {
                    taken.push((gmin, gmax));
                    taken.push((smin, smax));
                    got = Some(Placed { min: gmin, max: gmax, space_min: smin, space_max: smax });
                    break 'edges;
                }
                s += gap.max(1e-3);
            }
        }
        out.push(got);
    }
    out
}

/// A room as built: its level, number and name, and its outline on its level's plan (sheet units:
/// the built outline less the level's stacking move).
#[derive(Clone, Debug, Default)]
pub(crate) struct RoomBox {
    pub level: String,
    pub number: String,
    pub name: String,
    pub outline: Vec<Vec2>,
    /// How far the build moved the level to stack it (to put new elements where it put the rest).
    pub offset: Vec2,
}

/// The gear and feeders the documents give, ready for the spec.
#[derive(Clone, Debug, Default)]
pub(crate) struct Planned {
    pub equipment: Vec<cadaddy_walk3d::MepEquipment>,
    pub feeders: Vec<cadaddy_walk3d::Feeder>,
    /// Gear not laid out, and why.
    pub unplaced: Vec<(String, String)>,
}

/// Volts to ground of a system as a schedule prints it: `208Y/120V` → 120, `480Y/277` → 277,
/// `120/240V, 1PH, 3W` → 120, `480V` → 480.
fn volts_to_ground(v: &str) -> Option<f64> {
    let u = v.to_ascii_uppercase();
    let first = u.split(',').next().unwrap_or_default();
    let nums: Vec<f64> = first.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<f64>().ok()).filter(|n| *n >= 100.0).collect();
    match nums.as_slice() {
        [_, g, ..] => Some(*g),
        [only] => Some(*only),
        _ => None,
    }
}

/// The panel tags a schedule row's description names (`PANELBOARD G (4 UNITS)` → G).
fn names_panel(description: &str, tag: &str) -> bool {
    let u = description.to_ascii_uppercase();
    let t = tag.to_ascii_uppercase();
    let words: Vec<&str> = u.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    words.contains(&t.as_str()) && (u.trim() == t || u.contains("PANEL") || u.contains("PNL") || words.first() == Some(&t.as_str()))
}

/// The room a schedule's location names (`MECH ROOM 160`: its number; else its name).
fn room_named<'a>(rooms: &'a [RoomBox], location: &str) -> Option<&'a RoomBox> {
    let u = location.to_ascii_uppercase();
    let number = u.split_whitespace().rev().find(|w| w.chars().any(|c| c.is_ascii_digit()));
    number
        .and_then(|n| rooms.iter().find(|r| r.number.eq_ignore_ascii_case(n)))
        .or_else(|| rooms.iter().find(|r| !r.name.is_empty() && u.contains(&r.name.to_ascii_uppercase())))
}

/// The gear (panels the schedules list, sized by the submittal, in the rooms their schedules
/// name) and the feeders between them (a schedule row naming a panel, its type the one-line's
/// tag above that panel, the type's conduits and conductors).
pub(crate) fn plan(
    sp: &cadaddy_walk3d::BuildingSpec,
    types: &[FeederType],
    incoming: &BTreeMap<String, String>,
    gear: &BTreeMap<String, SubmittedGear>,
    rooms: &[RoomBox],
) -> Planned {
    use cadaddy_walk3d::{Enclosure, Feeder, MepEquipment, Termination};
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let rule = super::coderules::rules().into_iter().find(|r| r.id == "E-WORKSPACE").unwrap_or_default();
    let mut out = Planned::default();
    let schedules = &sp.mep.panel_schedules;
    let tags: Vec<String> = schedules.iter().map(|p| p.panel.clone()).collect();
    let header =
        |tag: &str, k: &str| schedules.iter().find(|p| p.panel.eq_ignore_ascii_case(tag)).and_then(|p| p.header.get(k).cloned()).unwrap_or_default();
    // Feeders: each schedule row naming another panel.
    for p in schedules {
        for c in &p.circuits {
            for child in tags.iter().filter(|t| !t.eq_ignore_ascii_case(&p.panel) && names_panel(&c.description, t)) {
                let Some(ft) = incoming.get(&child.to_ascii_uppercase()).and_then(|k| types.iter().find(|t| t.tag.eq_ignore_ascii_case(k))) else {
                    continue;
                };
                let single = header(child, "volts/phase").to_ascii_uppercase().contains("1PH");
                let phases = if single { 2 } else { 3 }.min(ft.conductors);
                out.feeders.push(Feeder {
                    tag: ft.tag.clone(),
                    from: p.panel.clone(),
                    to: child.clone(),
                    sets: ft.conduits.max(1),
                    phases,
                    size: ft.size.clone(),
                    material: ft.material.clone(),
                    neutrals: ft.conductors.saturating_sub(phases),
                    grounds: u32::from(!ft.ground_size.is_empty()),
                    ground_size: ft.ground_size.clone(),
                    conduits: ft.conduits.max(1),
                    conduit_in: ft.conduit_in,
                    entry: gear.get(child).map_or_else(|| "top".to_string(), |g| if g.feed.is_empty() { "top".into() } else { g.feed.clone() }),
                    from_entry: gear.get(&p.panel).map(|g| g.feed.clone()).unwrap_or_default(),
                    source: format!("{}; one-line tag {} above {child}; panel {} schedule circuit {}", ft.source, ft.tag, p.panel, c.number),
                    ..Feeder::default()
                });
            }
        }
    }
    // Gear: the panels the plans don't place, in their schedules' rooms.
    let mut by_room: BTreeMap<(String, String), Vec<(String, SubmittedGear)>> = BTreeMap::new();
    for p in schedules.iter().filter(|p| p.typical_of.is_none()) {
        if sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(&p.panel)) {
            continue;
        }
        let Some(g) = gear.get(&p.panel).filter(|g| g.size_in.iter().all(|v| *v > 0.0)) else {
            out.unplaced.push((p.panel.clone(), "no box size in a gear submittal".into()));
            continue;
        };
        let loc = p.header.get("location").cloned().unwrap_or_default();
        match room_named(rooms, &loc) {
            Some(r) => by_room.entry((r.level.clone(), r.number.clone())).or_default().push((p.panel.clone(), g.clone())),
            None => out
                .unplaced
                .push((p.panel.clone(), format!("its schedule's location ({}) is no room of the model", if loc.is_empty() { "none" } else { &loc }))),
        }
    }
    let inch = upf / 12.0;
    for ((level, number), list) in by_room {
        let Some(room) = rooms.iter().find(|r| r.level == level && r.number == number) else { continue };
        let outline: Vec<Vec2> = room.outline.iter().map(|p| *p - room.offset).collect();
        // Working space: the deepest of its gear's voltages (condition 1).
        let depth_for = |tag: &str| {
            let v = volts_to_ground(&header(tag, "volts/phase")).unwrap_or(150.0);
            rule.depth_in.iter().find(|r| v <= r[0]).map_or(36.0, |r| r[1])
        };
        let space_d = list.iter().map(|(t, _)| depth_for(t)).fold(0.0, f64::max);
        let sizes: Vec<(f64, f64)> = list.iter().map(|(_, g)| (g.size_in[1] * inch, g.size_in[2] * inch)).collect();
        let placed = place_along_walls(&outline, &sizes, rule.width_in.unwrap_or(30.0) * inch, space_d * inch, 6.0 * inch);
        for ((tag, g), at) in list.iter().zip(placed) {
            let Some(at) = at else {
                out.unplaced.push((tag.clone(), format!("room {number} has no wall run long enough for it and its working space")));
                continue;
            };
            let [h, w, dp] = g.size_in;
            let margin = rule.plate_margin_in.unwrap_or([2.0, 1.5]);
            let plate = [(w - 2.0 * margin[0]).max(0.0), (dp - margin[1]).max(0.0)];
            // The submittal's drawn conduit areas, where it draws them; else the box less margins.
            let (top_plate, bottom_plate) = match (g.top_plate_in, g.bottom_plate_in) {
                (None, None) => (if g.feed == "bottom" { [0.0, 0.0] } else { plate }, if g.feed == "bottom" { plate } else { [0.0, 0.0] }),
                (t, b) => (t.unwrap_or([0.0, 0.0]), b.unwrap_or([0.0, 0.0])),
            };
            let top = rule.mount_top_in.unwrap_or(78.0);
            let mut properties: BTreeMap<String, String> = schedules
                .iter()
                .find(|p| p.panel.eq_ignore_ascii_case(tag))
                .map(|p| p.header.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.clone())).collect())
                .unwrap_or_default();
            properties.insert("BOX".into(), format!("{h} H x {w} W x {dp} D in"));
            properties.insert(
                "WORKING SPACE".into(),
                format!(
                    "{:.0} W x {:.0} D x {:.0} H in in front ({}, condition 1)",
                    (rule.width_in.unwrap_or(30.0)).max(w),
                    depth_for(tag),
                    rule.height_in.unwrap_or(78.0),
                    rule.cite
                ),
            );
            properties
                .insert("PLACEMENT".into(), format!("proposed: along a wall of room {number}, its schedule's location; the plans don't locate it"));
            let mut sources = BTreeMap::new();
            sources.insert("size".into(), g.source.clone());
            sources.insert("position".into(), format!("proposed in room {number} ({})", header(tag, "location")));
            sources.insert("status".into(), "proposed".into());
            let mut terminations = Vec::new();
            if let Some((n, range)) = &g.main_lugs {
                terminations.push(Termination {
                    point: "main".into(),
                    lugs: *n,
                    wires_per_lug: 1,
                    range: range.clone(),
                    lug_type: "mechanical".into(),
                    supplied: true,
                    source: g.source.clone(),
                    ..Termination::default()
                });
            }
            if let Some(range) = &g.branch_lug {
                terminations.push(Termination {
                    point: "breaker".into(),
                    lugs: 1,
                    wires_per_lug: 1,
                    range: range.clone(),
                    lug_type: "mechanical".into(),
                    supplied: true,
                    source: g.source.clone(),
                    ..Termination::default()
                });
            }
            out.equipment.push(MepEquipment {
                tag: tag.clone(),
                kind: if g.kind.is_empty() { "panel".into() } else { g.kind.clone() },
                level: level.clone(),
                min: at.min,
                max: at.max,
                bottom_ft: (top - h).max(0.0) / 12.0,
                height_ft: h / 12.0,
                title: format!("{} {tag}", if g.kind == "switchboard" { "SWITCHBOARD" } else { "PANELBOARD" }),
                properties,
                sources,
                confidence: 0.5,
                terminations,
                enclosure: Some(Enclosure { top_entry_in: top_plate, bottom_entry_in: bottom_plate, gutter_in: [0.0, 0.0] }),
                ..MepEquipment::default()
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f64, y: f64, v: &str) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    #[test]
    fn a_feeder_type_schedule_reads_by_its_columns() {
        let mut v = vec![
            t(2.0, 10.0, "FEEDER SCHEDULE"),
            t(0.2, 9.4, "FEEDER"),
            t(0.2, 9.3, "TAG"),
            t(1.0, 9.3, "(QUANTITY) CONDUIT SIZE"),
            t(2.2, 9.6, "(QUANTITY)"),
            t(2.2, 9.5, "CONDUCTOR"),
            t(2.2, 9.4, "SIZE (AL UNO)"),
            t(3.6, 9.5, "GROUND SIZE (AL"),
            t(3.6, 9.4, "UNO)"),
            t(4.6, 9.5, "ISOLATED GROUND SIZE"),
        ];
        for (y, tag, c, k, g) in [
            (9.0, "74", "(1) 1-1/4\"C", "(4) #4 (CU)", "#8G (CU)"),
            (8.8, "123", "(1) 2\"C", "(3) #2/0", "#4G"),
            (8.6, "604", "(2) 4\"C", "(4) #500kcmil", "#2/0G"),
        ] {
            v.extend([t(0.25, y, tag), t(1.0, y, c), t(2.2, y, k), t(3.6, y, g), t(4.7, y, "-")]);
        }
        v.push(t(0.2, 8.2, "NOTE 1:"));
        let f = feeder_types(&v, "E-1");
        assert_eq!(f.len(), 3, "{f:#?}");
        assert_eq!(
            (f[0].tag.as_str(), f[0].conduit_in, f[0].material.as_str(), f[0].size.as_str(), f[0].ground_size.as_str()),
            ("74", 1.25, "cu", "4", "8")
        );
        assert_eq!((f[1].conduits, f[1].conduit_in, f[1].conductors, f[1].size.as_str(), f[1].material.as_str()), (1, 2.0, 3, "2/0", "al"));
        assert_eq!((f[2].conduits, f[2].conduit_in, f[2].size.as_str(), f[2].ground_size.as_str()), (2, 4.0, "500 kcmil", "2/0"));
    }

    #[test]
    fn a_one_lines_gear_label_takes_the_feeder_tag_above_it() {
        let v = vec![
            t(5.0, 5.0, "DP1"),
            t(5.05, 5.6, "604"),
            t(4.0, 3.0, "G"),
            t(4.02, 3.5, "123"),
            t(6.0, 3.0, "G"),
            t(6.0, 3.5, "123"),
            t(8.0, 5.0, "DP2"),
        ];
        let m = oneline_incoming(&v, &["DP1".into(), "DP2".into(), "G".into()], &["604".into(), "123".into()]);
        assert_eq!(m.get("DP1").map(String::as_str), Some("604"));
        assert_eq!(m.get("G").map(String::as_str), Some("123"));
        assert!(!m.contains_key("DP2"));
    }

    #[test]
    fn a_gear_submittal_gives_each_panels_box_feed_and_lugs() {
        let text = "Designation Type Box Size 23000 1 DP1 - 1 P2 208Y/120 MLO 600 (2)3/0- 500KCMIL Aluminum Top Surface 50 X 20 X 5.75 B50 65 K \
                    24000 1 A - 1 P2 MLO 400 (1)4/0- 600KCMIL Aluminum Top Surface 74 X 20 X 5.75 B74 \
                    DP1--PANELBOARD P2 INTERIOR W/ 600A MAIN LUG - (2)3/0-500Kcmil 18 125A /2P 3VA Mechanical (1)#14-3/0 AL / CU Lug";
        let g = submittal_gear(text, &["DP1".into(), "A".into(), "D".into()], "gear submittal");
        let dp1 = g.get("DP1").expect("DP1");
        assert_eq!(dp1.size_in, [50.0, 20.0, 5.75]);
        assert_eq!(dp1.feed, "top");
        assert_eq!(dp1.main_lugs, Some((2, "3/0-500 kcmil".into())));
        assert_eq!(dp1.branch_lug.as_deref(), Some("#14-3/0"));
        assert_eq!(g.get("A").map(|a| a.size_in), Some([74.0, 20.0, 5.75]));
        assert!(!g.contains_key("D"));
    }

    #[test]
    fn a_switchboards_bill_of_material_gives_its_feed_kind_and_incoming_lugs() {
        let text = "--- page 9 of 85 --- 20000 1 MDP--SB2 SWITCHBOARD - 2 SECTIONS SB2, Incoming Location: Bottom, 2,000 Amp Top Thru Bus 1 - INCOMING LUGS: (6) 600 KCMIL AL/CU MECH";
        let g = submittal_gear(text, &["MDP".into(), "DP1".into()], "switchgear");
        let m = g.get("MDP").expect("MDP");
        // Its size is on its elevation drawing (read by position: elevation_at).
        assert_eq!(m.size_in, [0.0; 3]);
        assert_eq!(m.feed, "bottom");
        assert_eq!(m.kind, "switchboard");
        assert_eq!(m.main_lugs, Some((6, "600 kcmil".into())));
    }

    #[test]
    fn a_switchboards_elevation_reads_by_position() {
        let tt = |x: f64, y: f64, v: &str| TextItem { value: v.to_string(), at: Vec2::new(x, y), height: 4.5, rotation: 0.0 };
        let mut v = vec![
            tt(121.2, 334.7, "32.00"),
            tt(111.9, 326.5, "SECTION#"),
            tt(117.1, 331.1, "WEIGHT:900.0"),
            tt(103.1, 318.2, "7.5"),
            tt(71.3, 334.7, "25.00"),
            tt(60.8, 326.5, "SECTION#"),
        ];
        for (i, y) in [306.9, 288.9, 270.9, 252.9, 162.9].iter().enumerate() {
            v.push(tt(19.5, *y, &format!("{}.00\"", [10, 20, 30, 40, 90][i])));
        }
        // Section 2's plan (a restricted area beside it), section 1's.
        v.extend([
            tt(114.8, 92.3, "RESTRICTED"),
            tt(121.0, 83.3, "17.00"),
            tt(103.1, 90.7, "12.00"),
            tt(121.2, 41.3, "26.00"),
            tt(113.7, 103.0, "TOP"),
            tt(127.6, 103.0, "BOTTOM"),
            tt(118.2, 107.5, "CONDUIT"),
            tt(176.5, 80.2, "28.00"),
            tt(164.6, 57.0, "2.50"),
            tt(163.2, 80.2, "23.00"),
        ]);
        v.extend([
            tt(70.1, 41.3, "19.00"),
            tt(62.7, 103.0, "BOTTOM"),
            tt(67.2, 107.5, "CONDUIT"),
            tt(20.6, 80.2, "28.00"),
            tt(35.6, 57.0, "2.50"),
            tt(34.2, 80.2, "23.00"),
        ]);
        let g = elevation_at(&v).expect("read");
        assert_eq!(g.size_in, [90.0, 57.0, 28.0]);
        assert_eq!(g.top_plate_in, Some([26.0, 23.0]));
        assert_eq!(g.bottom_plate_in, Some([45.0, 23.0]));
    }

    #[test]
    fn the_plan_feeds_each_panel_its_schedules_rows_and_lays_out_its_gear() {
        use cadaddy_walk3d::{BuildingSpec, PanelCircuit, PanelSchedule};
        let row =
            |n: &str, d: &str| PanelCircuit { number: n.into(), description: d.into(), breaker: "125".into(), poles: 2, ..PanelCircuit::default() };
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        let hdr = |loc: &str, v: &str| [("location".to_string(), loc.to_string()), ("volts/phase".to_string(), v.to_string())].into_iter().collect();
        sp.mep.panel_schedules = vec![
            PanelSchedule {
                panel: "P1".into(),
                circuits: vec![row("1", "PANELBOARD Q (2 UNITS)"), row("3", "PANELBOARD Q (2 UNITS)"), row("5", "SPARE")],
                header: hdr("ELEC 101", "208Y/120V, 3PH, 4W"),
                ..PanelSchedule::default()
            },
            PanelSchedule { panel: "Q".into(), typical_of: Some(10), header: hdr("UNIT", "208Y/120V, 1PH, 3W"), ..PanelSchedule::default() },
        ];
        let types = vec![FeederType {
            tag: "9".into(),
            conduits: 1,
            conduit_in: 2.0,
            conductors: 3,
            size: "2/0".into(),
            material: "al".into(),
            ground_size: "4".into(),
            source: "FEEDER SCHEDULE".into(),
        }];
        let incoming: BTreeMap<String, String> = [("Q".to_string(), "9".to_string())].into_iter().collect();
        let gear: BTreeMap<String, SubmittedGear> = [(
            "P1".to_string(),
            SubmittedGear {
                size_in: [50.0, 20.0, 5.75],
                feed: "top".into(),
                main_lugs: Some((2, "3/0-500 kcmil".into())),
                branch_lug: Some("#14-3/0".into()),
                source: "sub".into(),
                ..SubmittedGear::default()
            },
        )]
        .into_iter()
        .collect();
        let room = RoomBox {
            level: "L1".into(),
            number: "101".into(),
            name: "ELEC".into(),
            outline: vec![Vec2::new(0.0, 0.0), Vec2::new(144.0, 0.0), Vec2::new(144.0, 96.0), Vec2::new(0.0, 96.0)],
            offset: Vec2::new(0.0, 0.0),
        };
        let p = plan(&sp, &types, &incoming, &gear, &[room]);
        assert_eq!(p.feeders.len(), 2, "{:#?}", p.feeders);
        let f = &p.feeders[0];
        assert_eq!(
            (f.from.as_str(), f.to.as_str(), f.phases, f.neutrals, f.conduit_in, f.material.as_str(), f.from_entry.as_str()),
            ("P1", "Q", 2, 1, 2.0, "al", "top")
        );
        assert_eq!(p.equipment.len(), 1, "{:?}", p.unplaced);
        let e = &p.equipment[0];
        assert_eq!((e.tag.as_str(), e.level.as_str()), ("P1", "L1"));
        assert!(((e.max[0] - e.min[0]).max(e.max[1] - e.min[1]) - 20.0).abs() < 1e-6, "{e:?}");
        assert_eq!(e.enclosure.as_ref().map(|x| x.top_entry_in), Some([16.0, 4.25]));
        assert!((e.bottom_ft - 28.0 / 12.0).abs() < 1e-9);
        assert_eq!(e.sources.get("status").map(String::as_str), Some("proposed"));
        assert!(e.properties.get("WORKING SPACE").is_some_and(|w| w.contains("36 D")), "{:?}", e.properties);
    }

    #[test]
    fn gear_goes_along_the_walls_with_its_working_space_inside_the_room() {
        // A 10 x 6 ft room (inches): two 20" wide, 6" deep panels, 30" x 36" in front of each.
        let room = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 72.0), Vec2::new(0.0, 72.0)];
        let p = place_along_walls(&room, &[(20.0, 6.0), (20.0, 6.0)], 30.0, 36.0, 6.0);
        let (a, b) = (p[0].clone().unwrap(), p[1].clone().unwrap());
        // On the long (bottom) wall, side by side, spaces clear of each other.
        assert!(a.min[1].abs() < 1e-6 && b.min[1].abs() < 1e-6, "{a:?} {b:?}");
        assert!(a.space_max[0] <= b.space_min[0] + 1e-6 || b.space_max[0] <= a.space_min[0] + 1e-6);
        assert!((a.space_max[1] - 42.0).abs() < 1e-6);
        // A room too shallow for the working space takes none.
        let closet = [Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), Vec2::new(40.0, 30.0), Vec2::new(0.0, 30.0)];
        assert_eq!(place_along_walls(&closet, &[(20.0, 6.0)], 30.0, 36.0, 6.0), vec![None]);
    }
}
