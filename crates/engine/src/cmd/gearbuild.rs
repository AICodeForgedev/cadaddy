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

use std::collections::{BTreeMap, BTreeSet};

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

/// A panel's room's outline on the plans (a stacked level's less the move that stacked it): the
/// room it names, else the one holding `at`, a place on the plans.
pub(crate) fn room_outline_on_plan(rooms: &[RoomBox], level: &str, named: Option<&str>, at: Vec2) -> Option<Vec<Vec2>> {
    let on_plan = |r: &RoomBox| -> Vec<Vec2> { r.outline.iter().map(|p| *p - r.offset).collect() };
    let on_level = || rooms.iter().filter(|r| r.level.eq_ignore_ascii_case(level) && r.outline.len() >= 3);
    named.and_then(|n| on_level().find(|r| r.number.eq_ignore_ascii_case(n))).map(on_plan).or_else(|| on_level().map(on_plan).find(|o| inside(o, at)))
}

/// Whether a point is inside a room's outline.
pub(crate) fn inside_outline(poly: &[Vec2], p: Vec2) -> bool {
    inside(poly, p)
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
    place_clear_of(outline, &[], sizes, space_w, space_d, gap)
}

/// [`place_along_walls`], keeping gear and its working space off `keep_clear` (door swings).
pub(crate) fn place_clear_of(
    outline: &[Vec2],
    keep_clear: &[([f64; 2], [f64; 2])],
    sizes: &[(f64, f64)],
    space_w: f64,
    space_d: f64,
    gap: f64,
) -> Vec<Option<Placed>> {
    let n = outline.len();
    let mut edges: Vec<(Vec2, Vec2)> = (0..n).map(|i| (outline[i], outline[(i + 1) % n])).filter(|(a, b)| a.dist(*b) > 1e-9).collect();
    edges.sort_by(|a, b| b.0.dist(b.1).total_cmp(&a.0.dist(a.1)));
    let mut taken: Vec<([f64; 2], [f64; 2])> = keep_clear.to_vec();
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
    /// The doors on its walls: centre and width, in the outline's coordinates.
    pub doors: Vec<(Vec2, f64)>,
}

/// A gear box's conduit entry areas, top and bottom: the submittal's where it draws them, else the
/// box less margins on the side it's fed from.
fn entry_plates(g: &SubmittedGear, rule: &super::coderules::CodeRule) -> ([f64; 2], [f64; 2]) {
    let [_, w, dp] = g.size_in;
    let margin = rule.plate_margin_in.unwrap_or([2.0, 1.5]);
    let plate = [(w - 2.0 * margin[0]).max(0.0), (dp - margin[1]).max(0.0)];
    match (g.top_plate_in, g.bottom_plate_in) {
        (None, None) => (if g.feed == "bottom" { [0.0, 0.0] } else { plate }, if g.feed == "bottom" { plate } else { [0.0, 0.0] }),
        (t, b) => (t.unwrap_or([0.0, 0.0]), b.unwrap_or([0.0, 0.0])),
    }
}

/// Gear the plans locate takes its box from its submittal: its centre stays, its long side is the
/// width. Whether anything changed.
pub(crate) fn take_submitted_size(e: &mut cadaddy_walk3d::MepEquipment, g: &SubmittedGear, rule: &super::coderules::CodeRule, upf: f64) -> bool {
    if !g.size_in.iter().all(|v| v.is_finite() && *v > 0.0) || !upf.is_finite() || upf <= 0.0 {
        return false;
    }
    let [h, w, dp] = g.size_in;
    let inch = upf / 12.0;
    let (dx, dy) = ((e.max[0] - e.min[0]).abs(), (e.max[1] - e.min[1]).abs());
    let (wx, wy) = if dx >= dy { (w * inch, dp * inch) } else { (dp * inch, w * inch) };
    let bottom = (rule.mount_top_in.unwrap_or(78.0) - h).max(0.0) / 12.0;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    if close(dx, wx) && close(dy, wy) && close(e.height_ft, h / 12.0) && close(e.bottom_ft, bottom) {
        return false;
    }
    let c = [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0];
    e.min = [c[0] - wx / 2.0, c[1] - wy / 2.0];
    e.max = [c[0] + wx / 2.0, c[1] + wy / 2.0];
    e.height_ft = h / 12.0;
    e.bottom_ft = bottom;
    e.properties.insert("BOX".into(), format!("{h} H x {w} W x {dp} D in"));
    // What the takeoff reads; the size it replaces stays beside it.
    if let Some(was) = e.properties.insert("SIZE".into(), format!("{w}\" W x {dp}\" D x {h}\" H")) {
        e.properties.insert("SIZE_WAS".into(), was);
    }
    e.sources.insert("size".into(), g.source.clone());
    if e.enclosure.is_none() {
        let (top_entry_in, bottom_entry_in) = entry_plates(g, rule);
        e.enclosure = Some(cadaddy_walk3d::Enclosure { top_entry_in, bottom_entry_in, gutter_in: [0.0, 0.0] });
    }
    true
}

/// Slides a box along its width out of a wall crossing it, to that wall's face: not more than
/// `most`, not off the wall it stands on, not into another. How far it moved.
fn slide_out_of_crossing_wall(
    e: &mut cadaddy_walk3d::MepEquipment,
    walls: &[cadaddy_walk3d::WallMark],
    types: &[cadaddy_walk3d::WallType],
    upf: f64,
    most: f64,
) -> Option<f64> {
    if !upf.is_finite() || upf <= 0.0 {
        return None;
    }
    let inch = upf / 12.0;
    let axis = usize::from(e.max[1] - e.min[1] > e.max[0] - e.min[0]);
    let other = 1 - axis;
    let here: Vec<&cadaddy_walk3d::WallMark> = walls.iter().filter(|w| w.level.eq_ignore_ascii_case(&e.level)).collect();
    let half = |w: &cadaddy_walk3d::WallMark| types.iter().find(|t| t.name == w.wall_type).map_or(6.0, |t| t.thickness_in) / 2.0 * inch;
    let lo_hi = |w: &cadaddy_walk3d::WallMark, i: usize| (w.a[i].min(w.b[i]), w.a[i].max(w.b[i]));
    let (band_lo, band_hi) = (e.min[other], e.max[other]);
    // The walls across its width that reach its depth, each as the stretch of its width they fill.
    let crossing: Vec<(f64, f64)> = here
        .iter()
        .filter(|w| (w.a[axis] - w.b[axis]).abs() < inch && lo_hi(w, other).0 < band_hi && lo_hi(w, other).1 > band_lo)
        .map(|w| ((w.a[axis] + w.b[axis]) / 2.0, half(w)))
        .map(|(c, h)| (c - h, c + h))
        .collect();
    // The wall it is on runs along its width, inside its depth, and must still hold it after.
    let on_host = |lo: f64, hi: f64| {
        here.iter().any(|w| {
            let line = (w.a[other] + w.b[other]) / 2.0;
            (w.a[other] - w.b[other]).abs() < inch && line >= band_lo && line <= band_hi && lo_hi(w, axis).0 <= lo && lo_hi(w, axis).1 >= hi
        })
    };
    let (start, end) = (e.min[axis], e.max[axis]);
    let mid = (start + end) / 2.0;
    let touching = |lo: f64, hi: f64, c: &(f64, f64)| c.0 < hi - 1e-6 && c.1 > lo + 1e-6;
    let &(wall_lo, wall_hi) = crossing.iter().find(|c| touching(start, end, c))?;
    let by = if (wall_lo + wall_hi) / 2.0 <= mid { wall_hi - start } else { wall_lo - end };
    let (lo, hi) = (start + by, end + by);
    if by.abs() > most || crossing.iter().any(|c| touching(lo, hi, c)) || (on_host(start, end) && !on_host(lo, hi)) {
        return None;
    }
    e.min[axis] = lo;
    e.max[axis] = hi;
    Some(by.abs())
}

/// A panel placed by its plan lettering, with an end standing in a wall that crosses its own: it
/// slides along its wall until that wall's face (no box stands in a wall), the lettering's place
/// kept beside it. Not more than its own width. Whether it moved.
pub(crate) fn clear_of_crossing_walls(
    e: &mut cadaddy_walk3d::MepEquipment,
    walls: &[cadaddy_walk3d::WallMark],
    types: &[cadaddy_walk3d::WallType],
    upf: f64,
) -> bool {
    if e.sources.get("position").map(String::as_str) != Some(super::printcond::ROOM_PANEL_POSITION) {
        return false;
    }
    let most = (e.max[0] - e.min[0]).max(e.max[1] - e.min[1]);
    let Some(by) = slide_out_of_crossing_wall(e, walls, types, upf, most) else { return false };
    e.properties.insert("DESIGN_VALUE".into(), "at its lettering on the plan, an end in a crossing wall".into());
    e.properties.insert("CORRECTED_VALUE".into(), format!("{:.1} in along its wall, clear of that wall", by / (upf / 12.0)));
    e.properties.insert("CORRECTION_REASON".into(), "a box can't stand in a wall; its place is lettering, not a located symbol".into());
    e.sources.insert("footprint".into(), "rule: on the wall nearest the lettering, slid clear of a crossing wall".into());
    true
}

/// A panel placed by lettering or a proposal, whose place leaves it no working space (or stands
/// it on no wall, or under a duct): on the wall
/// face near it that has it, in its own room, the place it was read kept beside it. What it moved
/// from and to go on the object. Whether it moved.
pub(crate) fn onto_clear_face(e: &mut cadaddy_walk3d::MepEquipment, f: &super::validate_gear::ClearFace, upf: f64) -> bool {
    if !upf.is_finite() || upf <= 0.0 || (f.min == e.min && f.max == e.max) {
        return false;
    }
    // Where it was read, as it was got: CADaddy's own proposal says so, not lettering.
    let was = e.properties.get("DESIGN_VALUE").cloned().unwrap_or_else(|| {
        match (e.sources.get("status").is_some_and(|s| s.eq_ignore_ascii_case("proposed")), e.sources.get("position")) {
            (true, Some(p)) if !p.trim().is_empty() => format!("where CADaddy proposed it ({})", p.trim()),
            (true, _) => "where CADaddy proposed it".to_string(),
            _ => "at its lettering on the plan".to_string(),
        }
    });
    e.min = f.min;
    e.max = f.max;
    let (design, reason) = match f.was_clear_in {
        Some(c) if f.was_ducted => (
            format!("{was}: {c:.0} in clear in front, a duct or pipe over it"),
            "a duct or pipe passed through its dedicated space where it was read (NEC 110.26(E))",
        ),
        Some(c) => (format!("{was}: {c:.0} in clear in front"), "no working space where it was read (NEC 110.26(A))"),
        None => (format!("{was}: on no wall"), "it stood on no wall where it was read (a panelboard hangs on one)"),
    };
    e.properties.insert("DESIGN_VALUE".into(), design);
    e.properties.insert(
        "CORRECTED_VALUE".into(),
        format!("{:.0} in from there on wall {}: {:.0} in clear in front", f.moved_in, f.wall.hex(), f.now_clear_in),
    );
    e.properties.insert("CORRECTION_REASON".into(), format!("{reason}; that place is lettering or a proposal, not a located symbol"));
    e.sources.insert("footprint".into(), "rule: the nearest wall face in its room with its working space".into());
    true
}

/// A wireway built over `gear`, out of a wall its length would cross: still over the whole can.
/// Whether it moved.
pub(crate) fn wireway_clear_of_walls(
    w: &mut cadaddy_walk3d::MepEquipment,
    gear: &cadaddy_walk3d::MepEquipment,
    walls: &[cadaddy_walk3d::WallMark],
    types: &[cadaddy_walk3d::WallType],
    upf: f64,
) -> bool {
    let long = (w.max[0] - w.min[0]).max(w.max[1] - w.min[1]);
    let wide = (gear.max[0] - gear.min[0]).max(gear.max[1] - gear.min[1]);
    let Some(by) = slide_out_of_crossing_wall(w, walls, types, upf, ((long - wide) / 2.0).max(0.0)) else { return false };
    w.sources.insert("position".into(), format!("proposed above {}, {:.1} in along it to clear a wall", gear.tag, by / (upf / 12.0)));
    true
}

/// Whether a point is within `reach` of an outline's walls.
pub(crate) fn on_walls(outline: &[Vec2], p: Vec2, reach: f64) -> bool {
    let n = outline.len();
    (0..n).any(|i| {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let ab = b - a;
        let len2 = ab.x * ab.x + ab.y * ab.y;
        let t = if len2 > 1e-12 { (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0) } else { 0.0 };
        p.dist(a + ab * t) <= reach
    })
}

/// Whether gear this big needs a second way out of its room (`E-WORKSPACE`'s large equipment).
fn needs_second_exit(bus_amps: Option<f64>, width_in: f64, doors: usize, rule: &super::coderules::CodeRule) -> bool {
    let (Some(amps), Some(wide)) = (rule.large_equipment_amps, rule.large_equipment_width_in) else { return false };
    bus_amps.is_some_and(|a| a >= amps) && width_in > wide && doors < 2
}

/// The gear and feeders the documents give, ready for the spec.
#[derive(Clone, Debug, Default)]
pub(crate) struct Planned {
    pub equipment: Vec<cadaddy_walk3d::MepEquipment>,
    pub feeders: Vec<cadaddy_walk3d::Feeder>,
    /// Gear not laid out, and why.
    pub unplaced: Vec<(String, String)>,
    /// The working spaces in front of what was laid out (NEC 110.26), drawn so clashes show.
    pub zones: Vec<Zone>,
    /// What the layout could not give a room's gear (a door swing, a second exit), with the rule.
    pub room_issues: Vec<String>,
}

/// A piece of gear's working space: its plan box and its height.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Zone {
    pub tag: String,
    pub level: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub height_ft: f64,
}

/// The tag of the wireway built over a piece of gear.
pub(crate) fn wireway_tag(gear: &str) -> String {
    format!("{}-WIREWAY", gear.trim())
}

/// A wireway above `g` for raceways its plate can't take (`need_sq_in` of plate): the smallest
/// depth (square) with a length up to the rule's most, at least the gear's width, marked proposed
/// with the design value and rule beside it. None: no size in the rule holds it.
pub(crate) fn wireway_for(
    g: &cadaddy_walk3d::MepEquipment,
    need_sq_in: f64,
    usable_now: f64,
    upf: f64,
    rule: &super::coderules::CodeRule,
) -> Option<cadaddy_walk3d::MepEquipment> {
    if !need_sq_in.is_finite() || need_sq_in <= 0.0 || !upf.is_finite() || upf <= 0.0 {
        return None;
    }
    let step = rule.wireway_length_step_in.filter(|v| *v > 0.0).unwrap_or(12.0);
    let most = rule.wireway_max_length_in.filter(|v| *v > 0.0).unwrap_or(72.0);
    let usable = rule.wireway_usable.filter(|v| *v > 0.0 && *v <= 1.0).unwrap_or(0.85);
    let inch = upf / 12.0;
    let (dx, dy) = (g.max[0] - g.min[0], g.max[1] - g.min[1]);
    let wide = dx.max(dy) / inch;
    let (depth, length) = rule.wireway_depths_in.iter().filter(|d| **d > 0.0).find_map(|d| {
        let l = ((need_sq_in / (usable * d)).max(wide) / step).ceil() * step;
        (l <= most).then_some((*d, l))
    })?;
    // Centred over the gear, along its width.
    let c = [(g.min[0] + g.max[0]) / 2.0, (g.min[1] + g.max[1]) / 2.0];
    let half = if dx >= dy { [length * inch / 2.0, depth * inch / 2.0] } else { [depth * inch / 2.0, length * inch / 2.0] };
    let size = format!("{length:.0} x {depth:.0} x {depth:.0} in");
    let item = format!("wireway {size}");
    let mut properties = std::collections::BTreeMap::new();
    properties.insert("SIZE".to_string(), size);
    properties.insert("ABOVE".to_string(), g.tag.clone());
    properties.insert("DESIGN_VALUE".to_string(), format!("every raceway lands on {}'s top: {usable_now:.0} sq in usable", g.tag));
    properties.insert("CORRECTED_VALUE".to_string(), item.clone());
    properties.insert(
        "CORRECTION_REASON".to_string(),
        format!("about {need_sq_in:.0} sq in of plate needed against {usable_now:.0} sq in usable on {}", g.tag),
    );
    properties.insert("CORRECTION_RULE".to_string(), rule.wireway_cite.clone());
    let mut sources = std::collections::BTreeMap::new();
    sources.insert("status".to_string(), "proposed".to_string());
    sources.insert("position".to_string(), format!("proposed above {}", g.tag));
    Some(cadaddy_walk3d::MepEquipment {
        tag: wireway_tag(&g.tag),
        kind: "wireway".into(),
        level: g.level.clone(),
        min: [c[0] - half[0], c[1] - half[1]],
        max: [c[0] + half[0], c[1] + half[1]],
        bottom_ft: g.bottom_ft + g.height_ft,
        height_ft: depth / 12.0,
        title: format!("WIREWAY ABOVE {}", g.tag),
        properties,
        sources,
        confidence: 0.5,
        ..cadaddy_walk3d::MepEquipment::default()
    })
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
                    source: format!("{}; one-line tag {} above {child}; panel {} {SCHEDULE_CIRCUIT}{}", ft.source, ft.tag, p.panel, c.number),
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
        // Door swings stay clear: a door's width all round its centre, on either side of its wall.
        let swings: Vec<([f64; 2], [f64; 2])> = room
            .doors
            .iter()
            .map(|(c, w)| {
                let c = *c - room.offset;
                ([c.x - w, c.y - w], [c.x + w, c.y + w])
            })
            .collect();
        let (sw, sd, gap) = (rule.width_in.unwrap_or(30.0) * inch, space_d * inch, 6.0 * inch);
        let mut placed = place_clear_of(&outline, &swings, &sizes, sw, sd, gap);
        if placed.iter().any(Option::is_none) && !swings.is_empty() {
            let without = place_along_walls(&outline, &sizes, sw, sd, gap);
            if without.iter().filter(|p| p.is_some()).count() > placed.iter().filter(|p| p.is_some()).count() {
                placed = without;
                out.room_issues.push(format!("room {number}: its gear only fits inside a door's swing; move the door or the gear"));
            }
        }
        for (tag, g) in &list {
            let bus = header(tag, "bus amps").chars().take_while(char::is_ascii_digit).collect::<String>().parse::<f64>().ok();
            if needs_second_exit(bus, g.size_in[1], room.doors.len(), &rule) {
                out.room_issues.push(format!(
                    "room {number}: {tag} needs a second exit, {} door(s) found ({})",
                    room.doors.len(),
                    rule.large_equipment_cite.as_deref().unwrap_or_default()
                ));
            }
        }
        for ((tag, g), at) in list.iter().zip(placed) {
            let Some(at) = at else {
                out.unplaced.push((tag.clone(), format!("room {number} has no wall run long enough for it and its working space")));
                continue;
            };
            let [h, w, dp] = g.size_in;
            let (top_plate, bottom_plate) = entry_plates(g, &rule);
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
            // Where conduit may come in: each side's area (the submittal's drawn windows, else the
            // box less the rule's margins) and its source; the router lands raceways inside them.
            let drawn = g.top_plate_in.is_some() || g.bottom_plate_in.is_some();
            let margin = rule.plate_margin_in.unwrap_or([2.0, 1.5]);
            let area_src = if drawn {
                format!("drawn: {}", g.source)
            } else {
                format!("derived: the box less {} x {} in margins ({})", margin[0], margin[1], rule.cite)
            };
            for (side, a) in [("TOP", top_plate), ("BOTTOM", bottom_plate)] {
                let v = if a[0] > 0.0 && a[1] > 0.0 {
                    format!("{} W x {} D in, {:.0} sq in; {area_src}", a[0], a[1], a[0] * a[1])
                } else {
                    "none (no conduit this way)".to_string()
                };
                properties.insert(format!("{side} CONDUIT AREA"), v);
            }
            if !g.feed.is_empty() {
                properties.insert("INCOMING LOCATION".into(), format!("{} ({})", g.feed, g.source));
            }
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
            out.zones.push(Zone {
                tag: tag.clone(),
                level: level.clone(),
                min: at.space_min,
                max: at.space_max,
                height_ft: rule.height_in.unwrap_or(78.0) / 12.0,
            });
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
    #[test]
    fn a_panels_room_above_the_first_floor_is_read_on_the_plans() {
        // Level 2 built 64,000 along from its plan: the panel's place is on the plan.
        let off = Vec2::new(64_000.0, 35.0);
        let plan = [Vec2::new(0.0, 0.0), Vec2::new(144.0, 0.0), Vec2::new(144.0, 96.0), Vec2::new(0.0, 96.0)];
        let room = RoomBox {
            level: "Level 2".into(),
            number: "201".into(),
            outline: plan.iter().map(|p| *p + off).collect(),
            offset: off,
            ..RoomBox::default()
        };
        let rooms = [room];
        let at = Vec2::new(10.0, 10.0);
        // By the number it names, by where it is, and not on another level or outside it.
        assert_eq!(room_outline_on_plan(&rooms, "level 2", Some("201"), Vec2::new(-500.0, 0.0)).as_deref(), Some(&plan[..]));
        assert_eq!(room_outline_on_plan(&rooms, "Level 2", None, at).as_deref(), Some(&plan[..]));
        assert_eq!(room_outline_on_plan(&rooms, "Level 3", Some("201"), at), None);
        assert_eq!(room_outline_on_plan(&rooms, "Level 2", None, at + off), None);
    }

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
            doors: Vec::new(),
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
        assert!(
            e.properties.get("TOP CONDUIT AREA").is_some_and(|a| a.starts_with("16 W x 4.25 D in") && a.contains("derived")),
            "{:?}",
            e.properties
        );
        assert!(e.properties.get("BOTTOM CONDUIT AREA").is_some_and(|a| a.starts_with("none")));
        assert!(e.properties.get("INCOMING LOCATION").is_some_and(|a| a.starts_with("top")));
    }

    /// Raceways a can's plate can't take: a wireway over it, its basis on it; none too big to size.
    #[test]
    fn a_can_too_small_for_its_raceways_gets_a_wireway_built_over_it() {
        let rule = crate::cmd::coderules::rules().into_iter().find(|r| r.sets == "working_space").unwrap_or_default();
        let g = cadaddy_walk3d::MepEquipment {
            tag: "DP9".into(),
            level: "L1".into(),
            min: [100.0, 200.0],
            max: [105.75, 220.0],
            bottom_ft: 2.0,
            height_ft: 4.0,
            ..Default::default()
        };
        let w = wireway_for(&g, 231.0, 68.0, 12.0, &rule).expect("a wireway");
        assert_eq!(w.tag, "DP9-WIREWAY");
        assert_eq!(w.kind, "wireway");
        assert!((w.bottom_ft - 6.0).abs() < 1e-9, "on top of the can");
        // 6 in deep: 231 / (0.85 x 6) = 45.3 in, so 48 in, along the can's long side.
        assert_eq!(w.properties["SIZE"], "48 x 6 x 6 in");
        assert!((w.max[1] - w.min[1] - 48.0).abs() < 1e-9 && (w.max[0] - w.min[0] - 6.0).abs() < 1e-9, "{w:?}");
        assert!(w.properties["DESIGN_VALUE"].contains("68 sq in"));
        assert!(w.properties["CORRECTION_RULE"].contains("376"));
        assert_eq!(w.sources["status"], "proposed");
        assert!(wireway_for(&g, 1e6, 68.0, 12.0, &rule).is_none());
    }

    #[test]
    fn a_moved_panel_keeps_where_it_was_got_as_its_design_value() {
        use cadaddy_walk3d::MepEquipment;
        let f = crate::cmd::validate_gear::ClearFace {
            min: [0.0, 10.0],
            max: [6.0, 30.0],
            wall: cadaddy_doc::Handle(7),
            moved_in: 12.0,
            was_clear_in: Some(60.0),
            was_ducted: true,
            now_clear_in: 168.0,
        };
        let panel = |sources: &[(&str, &str)]| MepEquipment {
            tag: "E".into(),
            min: [0.0, 0.0],
            max: [6.0, 20.0],
            sources: sources.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        let mut proposed = panel(&[("status", "proposed"), ("position", "proposed in room 9 (STORAGE)")]);
        assert!(onto_clear_face(&mut proposed, &f, 12.0));
        let design = &proposed.properties["DESIGN_VALUE"];
        assert!(design.starts_with("where CADaddy proposed it (proposed in room 9") && !design.contains("lettering"), "{design}");
        assert!(design.contains("a duct or pipe over it") && proposed.properties["CORRECTION_REASON"].contains("110.26(E)"));
        let mut lettered = panel(&[("position", crate::cmd::printcond::ROOM_PANEL_POSITION)]);
        assert!(onto_clear_face(&mut lettered, &f, 12.0));
        assert!(lettered.properties["DESIGN_VALUE"].starts_with("at its lettering on the plan"));
    }

    #[test]
    fn a_lettered_panel_slides_out_of_the_wall_that_crosses_its_own() {
        use cadaddy_walk3d::{MepEquipment, WallMark, WallType};
        let wall = |a: [f64; 2], b: [f64; 2]| -> WallMark {
            serde_json::from_value(serde_json::json!({"a": a, "b": b, "level": "L2", "wall_type": "W"})).unwrap()
        };
        let types: [WallType; 1] = [serde_json::from_value(serde_json::json!({"name": "W", "thickness_in": 6.0})).unwrap()];
        // Its own wall runs north at x 102; a wall crosses at y 200, 6 in thick.
        let walls = [wall([102.0, 190.0], [102.0, 240.0]), wall([100.0, 200.0], [190.0, 200.0])];
        let panel = |y: f64, position: &str| MepEquipment {
            tag: "G-1".into(),
            level: "L2".into(),
            min: [100.0, y],
            max: [104.0, y + 14.0],
            sources: [("position".to_string(), position.to_string())].into(),
            ..Default::default()
        };
        let lettered = crate::cmd::printcond::ROOM_PANEL_POSITION;
        let mut e = panel(199.5, lettered);
        assert!(clear_of_crossing_walls(&mut e, &walls, &types, 12.0));
        assert!((e.min[1] - 203.0).abs() < 1e-9 && (e.max[1] - 217.0).abs() < 1e-9, "{e:?}");
        assert!((e.min[0] - 100.0).abs() < 1e-9, "stays on its wall");
        assert!(e.properties["DESIGN_VALUE"].contains("lettering"));
        assert!(e.properties["CORRECTED_VALUE"].contains("3.5 in"));
        assert!(!clear_of_crossing_walls(&mut e, &walls, &types, 12.0), "clear now");
        // A panel that is clear, or located by more than lettering, stays.
        let mut clear = panel(210.0, lettered);
        assert!(!clear_of_crossing_walls(&mut clear, &walls, &types, 12.0));
        let mut located = panel(199.5, "E-101 plan");
        assert!(!clear_of_crossing_walls(&mut located, &walls, &types, 12.0));
        // It doesn't leave its wall to get clear.
        let short = [wall([102.0, 195.0], [102.0, 214.0]), walls[1].clone()];
        let mut stays = panel(199.5, lettered);
        assert!(!clear_of_crossing_walls(&mut stays, &short, &types, 12.0));
        // Nor land in the next wall.
        let tight = [walls[0].clone(), walls[1].clone(), wall([100.0, 216.0], [190.0, 216.0])];
        let mut stuck = panel(199.5, lettered);
        assert!(!clear_of_crossing_walls(&mut stuck, &tight, &types, 12.0));
    }

    #[test]
    fn a_wireway_slides_out_of_a_wall_and_stays_over_its_can() {
        use cadaddy_walk3d::{MepEquipment, WallMark, WallType};
        let wall = |a: [f64; 2], b: [f64; 2]| -> WallMark {
            serde_json::from_value(serde_json::json!({"a": a, "b": b, "level": "L2", "wall_type": "W"})).unwrap()
        };
        let types: [WallType; 1] = [serde_json::from_value(serde_json::json!({"name": "W", "thickness_in": 6.0})).unwrap()];
        let can = MepEquipment { tag: "DP9".into(), level: "L2".into(), min: [100.0, 220.0], max: [106.0, 240.0], ..Default::default() };
        let way = || MepEquipment { tag: "DP9-WIREWAY".into(), level: "L2".into(), min: [100.0, 206.0], max: [106.0, 254.0], ..Default::default() };
        // A wall at y 210, 6 in thick, across the wireway's end: 7 in along to its face.
        let walls = [wall([90.0, 210.0], [120.0, 210.0])];
        let mut w = way();
        assert!(wireway_clear_of_walls(&mut w, &can, &walls, &types, 12.0));
        assert!((w.min[1] - 213.0).abs() < 1e-9 && (w.max[1] - 261.0).abs() < 1e-9, "{w:?}");
        assert!(w.min[1] <= can.min[1] && w.max[1] >= can.max[1], "still over the whole can");
        assert!(w.sources["position"].contains("7.0 in"));
        // Clearing it would leave the can's end uncovered: it stays, for review.
        let far = [wall([90.0, 220.0], [120.0, 220.0])];
        let mut w = way();
        assert!(!wireway_clear_of_walls(&mut w, &can, &far, &types, 12.0));
        assert!((w.min[1] - 206.0).abs() < 1e-9);
    }

    #[test]
    fn gear_the_plans_locate_takes_its_box_from_its_submittal() {
        let rule = crate::cmd::coderules::rules().into_iter().find(|r| r.id == "E-WORKSPACE").unwrap();
        let g = SubmittedGear { size_in: [74.0, 20.0, 5.75], source: "gear submittal p.3".into(), ..Default::default() };
        let mut e = cadaddy_walk3d::MepEquipment {
            tag: "A".into(),
            min: [100.0, 200.0],
            max: [100.0 + 5.0, 200.0 + 24.0],
            bottom_ft: 2.5,
            height_ft: 4.0,
            properties: [("SIZE".to_string(), "20\" W x 6\" D x 48\" H".to_string())].into(),
            ..Default::default()
        };
        assert!(take_submitted_size(&mut e, &g, &rule, 12.0));
        // Taller along y than x on the plan: the width runs that way; the centre stays.
        assert!((e.max[1] - e.min[1] - 20.0).abs() < 1e-9 && (e.max[0] - e.min[0] - 5.75).abs() < 1e-9, "{e:?}");
        assert!(((e.min[1] + e.max[1]) / 2.0 - 212.0).abs() < 1e-9);
        assert!((e.height_ft - 74.0 / 12.0).abs() < 1e-9);
        assert_eq!(e.properties.get("BOX").map(String::as_str), Some("74 H x 20 W x 5.75 D in"));
        assert_eq!(e.properties.get("SIZE").map(String::as_str), Some("20\" W x 5.75\" D x 74\" H"));
        assert_eq!(e.properties.get("SIZE_WAS").map(String::as_str), Some("20\" W x 6\" D x 48\" H"));
        assert_eq!(e.sources.get("size").map(String::as_str), Some("gear submittal p.3"));
        assert!(e.enclosure.is_some());
        assert!(!take_submitted_size(&mut e, &g, &rule, 12.0), "already its size");
        // No size in the submittal: left alone.
        let before = e.clone();
        assert!(!take_submitted_size(&mut e, &SubmittedGear::default(), &rule, 12.0));
        assert_eq!(e.height_ft, before.height_ft);
    }

    #[test]
    fn gear_keeps_off_a_door_swing_and_big_gear_wants_a_second_exit() {
        let room = [Vec2::new(0.0, 0.0), Vec2::new(120.0, 0.0), Vec2::new(120.0, 72.0), Vec2::new(0.0, 72.0)];
        let free = place_along_walls(&room, &[(20.0, 6.0)], 30.0, 36.0, 6.0);
        let free = free[0].clone().unwrap();
        // A 36" door on the bottom wall where the gear would have gone.
        let swing = ([free.min[0] - 20.0, -36.0], [free.min[0] + 52.0, 36.0]);
        let moved = place_clear_of(&room, &[swing], &[(20.0, 6.0)], 30.0, 36.0, 6.0);
        let moved = moved[0].clone().unwrap();
        assert!(moved.space_min[0] >= swing.1[0] - 1e-6 || moved.space_min[1] >= swing.1[1] - 1e-6, "{moved:?}");

        let rule = crate::cmd::coderules::rules().into_iter().find(|r| r.id == "E-WORKSPACE").unwrap();
        assert!(needs_second_exit(Some(2000.0), 90.0, 1, &rule), "2000 A, 7.5 ft wide, one door");
        assert!(!needs_second_exit(Some(2000.0), 90.0, 2, &rule), "two doors");
        assert!(!needs_second_exit(Some(2000.0), 57.0, 1, &rule), "not over 6 ft wide");
        assert!(!needs_second_exit(Some(600.0), 90.0, 1, &rule), "under 1200 A");
        assert!(!needs_second_exit(None, 90.0, 1, &rule), "rating unknown is not a finding");
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

/// The system feeders route on.
pub(crate) const FEEDER_SYSTEM: &str = "E-FEEDER";
/// A feeder's route above its level's ceiling, feet over the ceiling; under its floor, feet below
/// it (a bottom entry); a riser stands this far beside the gear it rises at, feet.
const FEEDER_OVER_CEILING_FT: f64 = 1.0;
const FEEDER_UNDER_FLOOR_FT: f64 = 1.5;
const RISER_BESIDE_FT: f64 = 1.5;

/// Copies of a typical panel stacked floor to floor stand within this of one another on plan, feet.
const STACK_REACH_FT: f64 = 6.0;
/// The most copies of a typical panel chained (a bound on hostile input).
const MAX_TYPICAL: usize = 5000;
/// The most levels a riser is cut at (a bound on hostile input).
const MAX_LEVELS: usize = 500;

type Gear = cadaddy_walk3d::MepEquipment;

/// The model's levels and units, for routing between pieces of gear.
struct RouteCtx<'a> {
    sp: &'a cadaddy_walk3d::BuildingSpec,
    upf: f64,
}

impl RouteCtx<'_> {
    fn level(&self, name: &str) -> Option<&cadaddy_walk3d::Level> {
        self.sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }
    fn corner(&self, name: &str) -> Vec2 {
        self.level(name).and_then(|l| l.region).map_or(Vec2::ZERO, |r| Vec2::new(r[0], r[1]))
    }
    fn elevation(&self, name: &str) -> f64 {
        self.level(name).map_or(0.0, |l| l.elevation_ft)
    }
    fn ceiling(&self, name: &str) -> f64 {
        self.level(name).map_or(9.0, |l| if l.ceiling_ft.is_finite() && l.ceiling_ft > 0.0 { l.ceiling_ft } else { 9.0 })
    }
    fn mid(e: &Gear) -> Vec2 {
        Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0)
    }
    /// Its place on its level's plan, from the plan's corner (each level's plan has its own place).
    fn plan(&self, e: &Gear) -> Vec2 {
        Self::mid(e) - self.corner(&e.level)
    }
    /// How far apart two pieces are, feet: across on plan and up.
    fn apart(&self, a: &Gear, b: &Gear) -> f64 {
        (self.plan(a) - self.plan(b)).len() / self.upf + (self.elevation(&a.level) - self.elevation(&b.level)).abs()
    }
    fn run(&self, level: &str, src: &str, pts: Vec<[f64; 2]>, size: f64, bottom: f64, top: Option<f64>) -> cadaddy_walk3d::MepRun {
        // Under the slab level's floor it is in the ground: the underground system (Q6148).
        let buried = bottom < 0.0 && super::ugplan::slab_level(self.sp).is_some_and(|l| l.eq_ignore_ascii_case(level));
        let mut r = super::printcond::run(if buried { super::ugplan::E_UG } else { FEEDER_SYSTEM }, level, src, pts, size, bottom);
        r.top_ft = top;
        r.size_source = "feeder schedule".into();
        r.height_source = "proposed".into();
        r
    }
    /// A vertical leg at `at` on `level`'s plan from `lo` to `hi` (feet over `level`'s floor),
    /// cut where the field's raceway changes and where it passes a floor (Q6347): a leg up out of
    /// the slab level's ground is the buried raceway to its stub-up's top (the underground rules'
    /// stub height over the floor, where it meets the raceway above in a transition), the
    /// above-grade raceway over it; a leg through another level's floor is that level's from its
    /// floor up (and the level below's under it), each piece on its own level's plan, so each
    /// level's riser is counted on that level and each floor it passes is a penetration.
    fn riser(&self, level: &str, src: &str, at: Vec2, lo: f64, hi: f64, size: f64, runs: &mut Vec<cadaddy_walk3d::MepRun>) {
        if !(lo.is_finite() && hi.is_finite()) || hi - lo <= 0.05 {
            return;
        }
        let base = self.elevation(level);
        let mut cuts = vec![lo, hi];
        let stub = super::ugplan::rules().stubup.conduit_top_ft;
        let on_slab = super::ugplan::slab_level(self.sp).is_some_and(|l| l.eq_ignore_ascii_case(level));
        if on_slab && lo < 0.0 && stub.is_finite() && stub >= 0.0 && hi > stub {
            cuts.push(stub);
        }
        // The floors it passes (a bound on hostile input: the model's levels); the slab level's
        // own is the ground's top, where the stub-up is the cut.
        let floors: Vec<(&str, f64)> =
            self.sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).take(MAX_LEVELS).map(|l| (l.name.as_str(), l.elevation_ft - base)).collect();
        cuts.extend(
            floors.iter().filter(|f| !(on_slab && f.0.eq_ignore_ascii_case(level))).map(|f| f.1).filter(|z| *z > lo + 1e-6 && *z < hi - 1e-6),
        );
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        for w in cuts.windows(2) {
            let [a, b] = w else { continue };
            // The level whose storey it stands in: the highest floor at or under its foot, else its own.
            let owner = floors.iter().filter(|f| f.1 <= a + 1e-6).max_by(|x, y| x.1.total_cmp(&y.1)).map_or((level, 0.0), |f| (f.0, f.1));
            let p = at - self.corner(level) + self.corner(owner.0);
            runs.push(self.run(owner.0, src, vec![[p.x, p.y]], size, a - owner.1, Some(b - owner.1)));
        }
    }

    /// One conduit of a feeder from gear `a` to gear `b`, out of `a` at `a0` and into `b` at `b0`
    /// (each in its conduit area, [`entry_slots`]): out of `a` (top or bottom) to the route over
    /// its level's ceiling (or under its floor), across in two legs to `b` or to a riser beside
    /// it, floor to floor, and into `b` (top or bottom).
    #[allow(clippy::too_many_arguments)]
    fn gear_to_gear(
        &self,
        a: &Gear,
        b: &Gear,
        (a_bottom, b_bottom): (bool, bool),
        (a0, b0): (Vec2, Vec2),
        size: f64,
        src: &str,
        runs: &mut Vec<cadaddy_walk3d::MepRun>,
    ) -> Result<(), String> {
        let (Some(la), Some(lb)) = (self.level(&a.level), self.level(&b.level)) else {
            return Err("a level not in the model".into());
        };
        let pb_here = b0 - self.corner(&lb.name) + self.corner(&la.name);
        // A service from site gear (the utility's transformer) runs at its least cover in the ground.
        let under = super::utilityxfmr::service_cover_ft(a, b, size).map_or(FEEDER_UNDER_FLOOR_FT, |v| v.max(FEEDER_UNDER_FLOOR_FT));
        let h_a = if a_bottom { -under } else { self.ceiling(&la.name) + FEEDER_OVER_CEILING_FT };
        let za_out = if a_bottom { a.bottom_ft } else { a.bottom_ft + a.height_ft };
        let zb_in = if b_bottom { b.bottom_ft } else { b.bottom_ft + b.height_ft };
        let same = la.name == lb.name;
        self.riser(&la.name, src, a0, za_out.min(h_a), za_out.max(h_a), size, runs);
        let end = if same { pb_here } else { pb_here + Vec2::new(RISER_BESIDE_FT * self.upf, 0.0) };
        let mut pts = vec![[a0.x, a0.y], [end.x, a0.y], [end.x, end.y]];
        pts.dedup_by(|p, q| (p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6);
        if pts.len() >= 2 {
            runs.push(self.run(&la.name, src, pts, size, h_a, None));
        }
        if same {
            self.riser(&la.name, src, end, zb_in.min(h_a), zb_in.max(h_a), size, runs);
            return Ok(());
        }
        let rise = lb.elevation_ft - la.elevation_ft;
        let h_b = if b_bottom { -under } else { self.ceiling(&lb.name) + FEEDER_OVER_CEILING_FT };
        let (lo, hi) = if rise >= 0.0 { (h_a, rise + h_b) } else { (rise + h_b, h_a) };
        self.riser(&la.name, src, end, lo, hi, size, runs);
        let beside = b0 + Vec2::new(RISER_BESIDE_FT * self.upf, 0.0);
        runs.push(self.run(&lb.name, src, vec![[beside.x, beside.y], [b0.x, b0.y]], size, h_b, None));
        self.riser(&lb.name, src, b0, zb_in.min(h_b), zb_in.max(h_b), size, runs);
        Ok(())
    }

    /// The feed on through `a`'s feed-through lugs to `b`, the next of its chain: straight through
    /// the floor when `b` stands over or under it (out of `a`'s top into `b`'s bottom, or the
    /// other way); a little off, through the floor into the lower level's ceiling space, square
    /// across over its ceiling and plumb into `b`; over the ceiling when on one level.
    fn feed_through(&self, a: &Gear, b: &Gear, size: f64, src: &str, runs: &mut Vec<cadaddy_walk3d::MepRun>) -> Result<(), String> {
        if a.level.eq_ignore_ascii_case(&b.level) {
            return self.gear_to_gear(a, b, (false, false), (Self::mid(a), Self::mid(b)), size, src, runs);
        }
        let rise = self.elevation(&b.level) - self.elevation(&a.level);
        let up = rise > 0.0;
        let za = if up { a.bottom_ft + a.height_ft } else { a.bottom_ft };
        let zb = if up { b.bottom_ft } else { b.bottom_ft + b.height_ft };
        let pa = Self::mid(a);
        let pa_there = pa - self.corner(&a.level) + self.corner(&b.level);
        let pb = Self::mid(b);
        if (pa_there - pb).len() <= 0.5 * self.upf {
            self.riser(&a.level, src, pa, za.min(rise + zb), za.max(rise + zb), size, runs);
            return Ok(());
        }
        // Off the stack: through the floor into the ceiling space between them (the lower
        // level's), square across over its ceiling, and plumb into `b`; never a leg across the
        // room at the gear's height.
        let square = |p: Vec2, q: Vec2| {
            let mut pts = vec![[p.x, p.y], [q.x, p.y], [q.x, q.y]];
            pts.dedup_by(|u, v| (u[0] - v[0]).abs() < 1e-6 && (u[1] - v[1]).abs() < 1e-6);
            pts
        };
        if up {
            let h = self.ceiling(&a.level) + FEEDER_OVER_CEILING_FT;
            let pb_here = pb - self.corner(&b.level) + self.corner(&a.level);
            self.riser(&a.level, src, pa, za.min(h), za.max(h), size, runs);
            runs.push(self.run(&a.level, src, square(pa, pb_here), size, h, None));
            self.riser(&a.level, src, pb_here, h.min(rise + zb), h.max(rise + zb), size, runs);
        } else {
            let h = self.ceiling(&b.level) + FEEDER_OVER_CEILING_FT;
            self.riser(&a.level, src, pa, (rise + h).min(za), (rise + h).max(za), size, runs);
            runs.push(self.run(&b.level, src, square(pa_there, pb), size, h, None));
            self.riser(&b.level, src, pb, zb.min(h), zb.max(h), size, runs);
        }
        Ok(())
    }
}

/// One conduit of a feeder: from gear `a` to gear `b`.
struct Leg<'a> {
    feeder: usize,
    a: &'a Gear,
    b: &'a Gear,
    ends: (bool, bool),
    size: f64,
    src: String,
    what: String,
}

/// A conduit's outside diameter by its trade size (EMT), inches.
fn conduit_od_in(trade: f64) -> f64 {
    const OD: [(f64, f64); 10] =
        [(0.5, 0.706), (0.75, 0.922), (1.0, 1.163), (1.25, 1.51), (1.5, 1.74), (2.0, 2.197), (2.5, 2.875), (3.0, 3.5), (3.5, 4.0), (4.0, 4.5)];
    OD.iter().min_by(|a, b| (a.0 - trade).abs().total_cmp(&(b.0 - trade).abs())).map_or(trade + 0.5, |o| if trade > 4.25 { trade + 0.5 } else { o.1 })
}

/// Clear space between two conduits entering side by side (locknuts and bushings), inches.
const CONDUIT_CLEAR_IN: f64 = 0.5;
/// The margin from a box's edges where its conduit area isn't given, inches.
const PLATE_MARGIN_IN: f64 = 2.0;

/// A gear side's conduit area, inches (width along the box's long side, depth): its
/// `TOP|BOTTOM CONDUIT AREA` property (`16 W x 4.25 D in…`), else the box less margins.
fn conduit_area(ctx: &RouteCtx<'_>, g: &Gear, bottom: bool) -> [f64; 2] {
    let key = if bottom { "BOTTOM CONDUIT AREA" } else { "TOP CONDUIT AREA" };
    let given = g.properties.get(key).and_then(|v| {
        let nums: Vec<f64> = v
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter_map(|w| w.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n > 0.0)
            .take(2)
            .collect();
        (nums.len() == 2 && v.contains(" W x ")).then(|| [nums[0], nums[1]])
    });
    given.unwrap_or_else(|| {
        let inch = ctx.upf / 12.0;
        let (w, d) = ((g.max[0] - g.min[0]).abs() / inch, (g.max[1] - g.min[1]).abs() / inch);
        let (long, short) = if w >= d { (w, d) } else { (d, w) };
        [(long - 2.0 * PLATE_MARGIN_IN).max(1.0), (short - PLATE_MARGIN_IN).max(1.0)]
    })
}

/// Where each leg's conduit leaves its source and enters its target: each gear side's conduits
/// laid in rows across its conduit area, a trade size and a half inch apart, in the order of where
/// they go (so they leave without crossing), rows from the back. Past the area's depth the rows go
/// on (the gear check says the can is too small).
fn entry_slots(ctx: &RouteCtx<'_>, legs: &[Leg<'_>]) -> Vec<(Vec2, Vec2)> {
    let mut out: Vec<(Vec2, Vec2)> = legs.iter().map(|l| (RouteCtx::mid(l.a), RouteCtx::mid(l.b))).collect();
    // Each gear side's conduits: (leg, its source end?, size, order along the side).
    let mut sides: BTreeMap<(String, String, bool), (&Gear, Vec<(usize, bool, f64, f64)>)> = BTreeMap::new();
    for (j, l) in legs.iter().enumerate() {
        for (g, other, bottom, is_a) in [(l.a, l.b, l.ends.0, true), (l.b, l.a, l.ends.1, false)] {
            let wide = (g.max[0] - g.min[0]).abs() >= (g.max[1] - g.min[1]).abs();
            let toward = ctx.plan(other) - ctx.plan(g);
            let along = if wide { toward.x } else { toward.y };
            sides.entry((g.tag.to_ascii_uppercase(), g.level.clone(), bottom)).or_insert_with(|| (g, Vec::new())).1.push((j, is_a, l.size, along));
        }
    }
    let inch = ctx.upf / 12.0;
    for ((_, _, bottom), (g, mut list)) in sides {
        list.sort_by(|a, b| a.3.total_cmp(&b.3).then(a.0.cmp(&b.0)));
        let [w, d] = conduit_area(ctx, g, bottom);
        let wide = (g.max[0] - g.min[0]).abs() >= (g.max[1] - g.min[1]).abs();
        let mid = RouteCtx::mid(g);
        let (mut x, mut row_y, mut row_h) = (-w / 2.0, -d / 2.0, 0.0_f64);
        for (j, is_a, size, _) in list {
            let od = conduit_od_in(size);
            if x > -w / 2.0 && x + od > w / 2.0 {
                x = -w / 2.0;
                row_y += row_h + CONDUIT_CLEAR_IN;
                row_h = 0.0;
            }
            let (u, v) = (x + od / 2.0, row_y + od / 2.0);
            x += od + CONDUIT_CLEAR_IN;
            row_h = row_h.max(od);
            let at = if wide { mid + Vec2::new(u * inch, v * inch) } else { mid + Vec2::new(v * inch, u * inch) };
            if let Some(o) = out.get_mut(j) {
                if is_a {
                    o.0 = at;
                } else {
                    o.1 = at;
                }
            }
        }
    }
    out
}

/// The units a feeder serves from its source's schedule (`PANELBOARD G (4 UNITS)` → 4): the
/// source panel's circuit naming the feeder's other end.
fn units_per_feeder(sp: &cadaddy_walk3d::BuildingSpec, f: &cadaddy_walk3d::Feeder) -> Option<usize> {
    let to = f.to.to_ascii_uppercase();
    let words = |v: &str| {
        v.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>()
    };
    sp.mep
        .panel_schedules
        .iter()
        .filter(|p| p.panel.eq_ignore_ascii_case(&f.from))
        .flat_map(|p| &p.circuits)
        .map(|c| words(&c.description))
        .filter(|w| w.contains(&to))
        .find_map(|w| w.windows(2).find_map(|p| (p[1] == "UNITS" || p[1] == "UNIT").then(|| p[0].parse::<usize>().ok()).flatten()))
        .filter(|n| (1..=64).contains(n))
}

/// A typical panel's placed copies (indices into the spec's equipment) in stacks: copies stacked
/// floor to floor (one place on each level's plan), each stack bottom to top; a copy stacked
/// with none is a stack of its own.
fn typical_stacks(ctx: &RouteCtx<'_>, family: &[usize]) -> Vec<Vec<usize>> {
    let eq = &ctx.sp.mep.equipment;
    let reach = STACK_REACH_FT * ctx.upf;
    let mut stacks: Vec<(Vec2, Vec<usize>)> = Vec::new();
    for &i in family.iter().take(MAX_TYPICAL) {
        let Some(e) = eq.get(i) else { continue };
        let p = ctx.plan(e);
        let same_level = |o: &usize| eq.get(*o).is_some_and(|o| o.level.eq_ignore_ascii_case(&e.level));
        match stacks.iter_mut().find(|s| (s.0 - p).len() <= reach && !s.1.iter().any(same_level)) {
            Some(s) => s.1.push(i),
            None => stacks.push((p, vec![i])),
        }
    }
    let elevation = |i: &usize| eq.get(*i).map_or(0.0, |e| ctx.elevation(&e.level));
    stacks
        .into_iter()
        .map(|(_, mut s)| {
            s.sort_by(|a, b| elevation(a).total_cmp(&elevation(b)));
            s
        })
        .collect()
}

/// The property a typical panel's copy carries the plan's label of the breaker that feeds its
/// stack (`SB2-5`, lettered in its room), set by [`label_typical_copies`].
pub(crate) const FEEDER_LABEL: &str = "FEEDER LABEL";
/// The property each copy of a typical panel carries its feed: its source panel's breaker and
/// feeder, or the copy whose feed-through lugs feed it ([`record_typical_feeds`]).
pub(crate) const FED_BY: &str = "FED BY";
/// How a feeder read off a schedule row names that row (`… panel SB2 schedule circuit 5`).
const SCHEDULE_CIRCUIT: &str = "schedule circuit ";

/// A circuit number's first number (`3`, `1,3,5`, `13/15`: 1 and 13).
fn first_number(s: &str) -> Option<u32> {
    s.split(|c: char| !c.is_ascii_digit()).find(|w| !w.is_empty())?.parse().ok()
}

/// A breaker label as a plan letters it: a source panel and its (first) circuit, `SB2-5`,
/// `SB-2-11`, `SB2-5,7,9`. One word, nothing else.
pub(crate) fn breaker_label(text: &str) -> Option<(String, u32)> {
    let t = text.trim();
    if t.is_empty() || t.len() > 24 || t.chars().any(char::is_whitespace) {
        return None;
    }
    let first = t.split(',').next()?.trim();
    let (panel, number) = first.rsplit_once('-')?;
    let panel = panel.trim().trim_end_matches('-');
    let ok = !panel.is_empty()
        && panel.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && panel.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit());
    if !ok {
        return None;
    }
    Some((panel.to_ascii_uppercase(), number.parse().ok()?))
}

/// The source breaker of a feeder read off a schedule row: its panel and circuit.
fn feeder_breaker(f: &cadaddy_walk3d::Feeder) -> Option<(String, u32)> {
    let (_, rest) = f.source.rsplit_once(SCHEDULE_CIRCUIT)?;
    Some((f.from.trim().to_ascii_uppercase(), first_number(rest)?))
}

/// The breakers that feed a typical panel (one schedule for many rooms): each schedule row of
/// another panel naming it (`PANELBOARD U (4 UNITS)`), by its panel and first circuit number,
/// with the typical panel's tag. A panel is typical when its schedule is typical of several
/// rooms or the row feeding it serves several units.
pub(crate) fn typical_breakers(schedules: &[cadaddy_walk3d::PanelSchedule]) -> BTreeMap<(String, u32), String> {
    let mut out = BTreeMap::new();
    for p in schedules.iter().filter(|p| p.typical_of.is_none()) {
        for c in &p.circuits {
            let units = c.description.to_ascii_uppercase().contains("UNITS");
            let Some(t) = schedules
                .iter()
                .filter(|t| !t.panel.eq_ignore_ascii_case(&p.panel) && (t.typical_of.is_some() || units))
                .find(|t| names_panel(&c.description, &t.panel))
            else {
                continue;
            };
            if let Some(n) = first_number(&c.number) {
                out.insert((p.panel.trim().to_ascii_uppercase(), n), t.panel.clone());
            }
        }
    }
    out
}

/// A label lettered on a plan, placed onto the model: its text, level, place, sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PlanLabel {
    pub text: String,
    pub level: String,
    pub at: [f64; 2],
    pub sheet: String,
}

/// Whether two names are one level (`2`, `Level 2`, `LEVEL 2`).
fn same_level(a: &str, b: &str) -> bool {
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    a.trim().eq_ignore_ascii_case(b.trim())
        || super::printmep::level_name(a).eq_ignore_ascii_case(&super::printmep::level_name(b))
        || (!digits(a).is_empty() && digits(a) == digits(b))
}

/// What [`label_typical_copies`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LabelReport {
    /// Copies that took a label.
    pub labelled: usize,
    /// Labels in no copy's room.
    pub unmatched: usize,
    /// Rooms with two different labels in them (none taken).
    pub conflicts: Vec<String>,
}

/// Each typical panel's copy in a room (its `ROOM` property; `rooms`: by room, its level and
/// outline) takes the breaker label lettered in that room on its level ([`breaker_label`]):
/// the plan's word for which breaker feeds the copy's stack. A room with two different labels
/// takes neither (said which). An earlier label goes first (a rerun).
pub(crate) fn label_typical_copies(
    copies: &mut [cadaddy_walk3d::MepEquipment],
    labels: &[PlanLabel],
    rooms: &BTreeMap<String, (String, Vec<Vec2>, bool)>,
) -> LabelReport {
    let mut out = LabelReport::default();
    let mut used = vec![false; labels.len()];
    for e in copies.iter_mut() {
        e.properties.remove(FEEDER_LABEL);
        e.sources.remove("feeder label");
        let Some((level, outline, _)) = e.properties.get("ROOM").and_then(|r| rooms.get(r)) else { continue };
        if outline.len() < 3 {
            continue;
        }
        let here: Vec<usize> = labels
            .iter()
            .enumerate()
            .filter(|(_, l)| breaker_label(&l.text).is_some() && same_level(&l.level, level))
            .filter(|(_, l)| cadaddy_geom::point_in_polygon(outline, Vec2::new(l.at[0], l.at[1])))
            .map(|(i, _)| i)
            .collect();
        for &i in &here {
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
        }
        let keys: BTreeSet<(String, u32)> = here.iter().filter_map(|&i| labels.get(i).and_then(|l| breaker_label(&l.text))).collect();
        let Some(first) = here.first().and_then(|&i| labels.get(i)) else { continue };
        if keys.len() > 1 {
            let texts: BTreeSet<&str> = here.iter().filter_map(|&i| labels.get(i)).map(|l| l.text.as_str()).collect();
            out.conflicts.push(format!("{}: {} labels in its room ({})", e.tag, keys.len(), texts.into_iter().collect::<Vec<_>>().join(", ")));
            continue;
        }
        e.properties.insert(FEEDER_LABEL.into(), first.text.trim().to_ascii_uppercase());
        e.sources.insert("feeder label".into(), format!("plan label {} lettered in its room ({})", first.text.trim(), first.sheet));
        out.labelled += 1;
    }
    out.unmatched = used.iter().filter(|u| !**u).count();
    out
}

/// One feeder to a typical panel and the copies it feeds: the feeder's index, its copies
/// (indices into the equipment) in feed order from its source (through their feed-through
/// lugs), and the plan label that gave them (none: the nearest stack, proposed).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TypicalFeed {
    pub feeder: usize,
    pub chain: Vec<usize>,
    pub label: Option<String>,
}

/// The feeders to typical panels (one schedule for many rooms: `PANELBOARD U (4 UNITS)`), each
/// with the copies it feeds through their feed-through lugs. The documents first: a stack whose
/// copies carry a breaker label ([`FEEDER_LABEL`]) is fed by the feeder from that breaker (each
/// copy the nearest labelled copy's in its stack; no more copies than the units the breaker's
/// row serves, those nearest the label). The stacks no label names are then chained in the
/// units each feeder serves and handed to the feeders no label names, nearest first
/// (proposed). With notes on labels that name no feeder or two stacks.
pub(crate) fn typical_feeds(sp: &cadaddy_walk3d::BuildingSpec) -> (Vec<TypicalFeed>, Vec<String>) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ctx = RouteCtx { sp, upf };
    let eq = &sp.mep.equipment;
    let tagged = |tag: &str| eq.iter().enumerate().filter(|(_, e)| e.tag.eq_ignore_ascii_case(tag)).map(|(i, _)| i).collect::<Vec<_>>();
    let one = |tag: &str| {
        let m = tagged(tag);
        (m.len() == 1).then(|| m[0])
    };
    // A typical panel's copies: many tagged as it, or tagged `<it>-…`.
    let family = |tag: &str| {
        let m = tagged(tag);
        if m.len() > 1 {
            return m;
        }
        let pre = format!("{}-", tag.to_ascii_uppercase());
        eq.iter().enumerate().filter(|(_, e)| e.kind == "panel" && e.tag.to_ascii_uppercase().starts_with(&pre)).map(|(i, _)| i).collect()
    };
    let mut typical: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in sp.mep.feeders.iter().enumerate() {
        if one(&f.from).is_some() && one(&f.to).is_none() {
            typical.entry(f.to.to_ascii_uppercase()).or_default().push(i);
        }
    }
    let mut out: Vec<TypicalFeed> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for (tag, feeders) in &typical {
        let fam = family(tag);
        if fam.is_empty() {
            continue;
        }
        let n = feeders.iter().find_map(|&i| sp.mep.feeders.get(i).and_then(|f| units_per_feeder(sp, f)));
        let breaker = |i: usize| sp.mep.feeders.get(i).and_then(feeder_breaker);
        // The feeder a copy's label names.
        let named = |e: usize| -> Option<(usize, String)> {
            let text = eq.get(e)?.properties.get(FEEDER_LABEL)?;
            let key = breaker_label(text)?;
            feeders.iter().copied().find(|&i| breaker(i).as_ref() == Some(&key)).map(|i| (i, text.clone()))
        };
        let mut pool: Vec<Vec<usize>> = Vec::new();
        let mut by_label: BTreeMap<usize, (Vec<usize>, String)> = BTreeMap::new();
        for stack in typical_stacks(&ctx, &fam) {
            for &e in &stack {
                if let Some(text) = eq.get(e).and_then(|g| g.properties.get(FEEDER_LABEL))
                    && named(e).is_none()
                {
                    notes.push(format!("{}: its plan label {text} names no feeder to {tag}", eq.get(e).map_or("?", |g| g.tag.as_str())));
                }
            }
            let marks: Vec<(usize, usize, String)> = stack.iter().enumerate().filter_map(|(k, &e)| named(e).map(|(i, t)| (k, i, t))).collect();
            if marks.is_empty() {
                pool.push(stack);
                continue;
            }
            // Each copy the nearest labelled copy's feeder (a tie: the lower one's).
            let owner: Vec<usize> = (0..stack.len()).map(|k| marks.iter().min_by_key(|m| (m.0.abs_diff(k), m.0)).map_or(0, |m| m.1)).collect();
            let mut k = 0;
            while k < stack.len() {
                let Some(&i) = owner.get(k) else { break };
                let mut end = k;
                while owner.get(end) == Some(&i) {
                    end += 1;
                }
                let piece: Vec<usize> = stack.get(k..end).map(<[usize]>::to_vec).unwrap_or_default();
                // No more than the units its row serves: those nearest its label.
                let mark = marks.iter().filter(|m| m.1 == i && m.0 >= k && m.0 < end).map(|m| m.0 - k).next().unwrap_or(0);
                let (kept, rest): (Vec<usize>, Vec<Vec<usize>>) = match n {
                    Some(n) if piece.len() > n => {
                        let from = mark.min(piece.len() - n);
                        let kept = piece.get(from..from + n).map(<[usize]>::to_vec).unwrap_or_default();
                        let below = piece.get(..from).map(<[usize]>::to_vec).unwrap_or_default();
                        let above = piece.get(from + n..).map(<[usize]>::to_vec).unwrap_or_default();
                        (kept, [below, above].into_iter().filter(|p| !p.is_empty()).collect())
                    }
                    _ => (piece, Vec::new()),
                };
                pool.extend(rest);
                let text = marks.iter().find(|m| m.1 == i).map(|m| m.2.clone()).unwrap_or_default();
                match by_label.entry(i) {
                    std::collections::btree_map::Entry::Occupied(_) => {
                        let first = kept.first().and_then(|&e| eq.get(e)).map_or("?", |g| g.tag.as_str());
                        notes.push(format!("{first}: its plan label {text} names a breaker another stack's label names; fed as if unlabelled"));
                        pool.push(kept);
                    }
                    std::collections::btree_map::Entry::Vacant(v) => {
                        v.insert((kept, text));
                    }
                }
                k = end;
            }
        }
        // The rest: chained in the units each feeder serves, nearest first to the feeders left.
        let len = n.unwrap_or(usize::MAX).max(1);
        let chains: Vec<Vec<usize>> = pool.iter().flat_map(|s| s.chunks(len.min(s.len().max(1))).map(<[usize]>::to_vec)).collect();
        let mut pairs = Vec::new();
        for &i in feeders.iter().filter(|i| !by_label.contains_key(i)) {
            let Some(src) = sp.mep.feeders.get(i).and_then(|f| one(&f.from)).and_then(|s| eq.get(s)) else { continue };
            for (c, chain) in chains.iter().enumerate() {
                let d = chain.iter().filter_map(|&e| eq.get(e)).map(|e| ctx.apart(src, e)).fold(f64::INFINITY, f64::min);
                pairs.push((d, i, c));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken = vec![false; chains.len()];
        let mut proposed: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (_, i, c) in pairs {
            if proposed.contains_key(&i) || taken.get(c).copied().unwrap_or(true) {
                continue;
            }
            if let (Some(t), Some(chain)) = (taken.get_mut(c), chains.get(c)) {
                *t = true;
                proposed.insert(i, chain.clone());
            }
        }
        let feeds = by_label.into_iter().map(|(i, (c, t))| (i, c, Some(t))).chain(proposed.into_iter().map(|(i, c)| (i, c, None)));
        for (i, mut chain, label) in feeds {
            // From the chain's end nearer the source.
            let src = sp.mep.feeders.get(i).and_then(|f| one(&f.from)).and_then(|s| eq.get(s));
            if let (Some(a), Some(first), Some(last)) = (src, chain.first().and_then(|&e| eq.get(e)), chain.last().and_then(|&e| eq.get(e)))
                && ctx.apart(a, last) < ctx.apart(a, first)
            {
                chain.reverse();
            }
            out.push(TypicalFeed { feeder: i, chain, label });
        }
    }
    out.sort_by_key(|f| f.feeder);
    (out, notes)
}

/// What [`record_typical_feeds`] recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FedReport {
    /// Copies whose feed the plan's labels give, and those proposed by the nearest stack.
    pub by_label: usize,
    pub proposed: usize,
    /// Copies of a typical panel no feeder reaches.
    pub unfed: usize,
    pub notes: Vec<String>,
}

/// Each typical panel's copy takes its feed onto itself ([`FED_BY`], [`typical_feeds`]): the
/// first of a chain its source breaker and feeder, the rest the copy whose feed-through lugs
/// feed it; where it came from (the plan's breaker label, or proposed by the nearest stack). A
/// rerun's earlier feed goes first.
pub(crate) fn record_typical_feeds(sp: &mut cadaddy_walk3d::BuildingSpec) -> FedReport {
    let (feeds, notes) = typical_feeds(sp);
    let mut out = FedReport { notes, ..FedReport::default() };
    for e in sp.mep.equipment.iter_mut() {
        if e.sources.remove("fed by").is_some() {
            e.properties.remove(FED_BY);
        }
    }
    let mut fed: BTreeSet<usize> = BTreeSet::new();
    for f in &feeds {
        let Some(feeder) = sp.mep.feeders.get(f.feeder).cloned() else { continue };
        let breaker = feeder_breaker(&feeder).map_or_else(|| feeder.from.clone(), |(p, n)| format!("{p} circuit {n}"));
        let what = if feeder.tag.is_empty() { format!("from {breaker}") } else { format!("feeder {} from {breaker}", feeder.tag) };
        let basis = match &f.label {
            Some(t) => format!("the plan's breaker label {t} in its stack"),
            None => format!("proposed: the stack nearest {} that no plan label names", feeder.from),
        };
        let mut prev: Option<String> = None;
        for &e in &f.chain {
            let Some(g) = sp.mep.equipment.get_mut(e) else { continue };
            let value = match &prev {
                None => what.clone(),
                Some(p) => format!("{p}'s feed-through lugs ({what})"),
            };
            g.properties.insert(FED_BY.into(), value);
            g.sources.insert("fed by".into(), basis.clone());
            prev = Some(g.tag.clone());
            fed.insert(e);
            if f.label.is_some() {
                out.by_label += 1;
            } else {
                out.proposed += 1;
            }
        }
    }
    let typical: BTreeSet<String> = sp.mep.panel_schedules.iter().filter(|p| p.typical_of.is_some()).map(|p| p.panel.to_ascii_uppercase()).collect();
    out.unfed = sp
        .mep
        .equipment
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            !fed.contains(i)
                && e.kind == "panel"
                && typical.iter().any(|t| e.tag.eq_ignore_ascii_case(t) || e.tag.to_ascii_uppercase().starts_with(&format!("{t}-")))
        })
        .count();
    out
}

/// The feeder into each typical panel's copy, by the copy's index: [`typical_feeds`]' feeder.
pub(crate) fn typical_feeder_in(sp: &cadaddy_walk3d::BuildingSpec) -> BTreeMap<usize, usize> {
    typical_feeds(sp).0.into_iter().flat_map(|f| f.chain.into_iter().map(move |e| (e, f.feeder))).collect()
}

/// The feeders' conduit, proposed (Q6152): each conduit of each feeder from its gear's exit (top
/// or bottom, as the feeder's entries say) up over the level's ceiling (or down under its floor),
/// across in two legs to its other gear, rising floor to floor beside it when that gear is on
/// another level, and into it (top or bottom). Parallel conduits a trade size and an inch apart.
/// A feeder to a typical panel (one schedule for many rooms: `PANELBOARD U (4 UNITS)`) feeds a
/// chain of its copies through their feed-through lugs ([`typical_feeds`]: the stack the plan's
/// breaker label names, else the nearest stack left), routed to the chain's end nearer it and on
/// through the chain. A feeder with no placed gear at an end is left out, said why.
pub(crate) fn feeder_runs(sp: &cadaddy_walk3d::BuildingSpec) -> (Vec<cadaddy_walk3d::MepRun>, Vec<String>) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ctx = RouteCtx { sp, upf };
    let mut runs = Vec::new();
    let mut left = Vec::new();
    let tagged = |tag: &str| sp.mep.equipment.iter().filter(|e| e.tag.eq_ignore_ascii_case(tag)).collect::<Vec<_>>();
    let one = |tag: &str| {
        let m = tagged(tag);
        (m.len() == 1).then(|| m[0])
    };
    // A typical panel's copies: many tagged as it, or tagged `<it>-…`.
    let family = |tag: &str| {
        let m = tagged(tag);
        if m.len() > 1 {
            return m;
        }
        let pre = format!("{}-", tag.to_ascii_uppercase());
        sp.mep.equipment.iter().filter(|e| e.kind == "panel" && e.tag.to_ascii_uppercase().starts_with(&pre)).collect()
    };
    let typical: BTreeSet<String> =
        sp.mep.feeders.iter().filter(|f| one(&f.from).is_some() && one(&f.to).is_none()).map(|f| f.to.to_ascii_uppercase()).collect();
    let chain_of: BTreeMap<usize, Vec<&Gear>> =
        typical_feeds(sp).0.into_iter().map(|f| (f.feeder, f.chain.iter().filter_map(|&e| sp.mep.equipment.get(e)).collect())).collect();
    // Each conduit's leg, gear to gear; a chain's feed-throughs after its first leg.
    let mut legs: Vec<Leg> = Vec::new();
    let mut chained: Vec<(usize, Vec<&Gear>, f64, String, String)> = Vec::new();
    for (i, f) in sp.mep.feeders.iter().enumerate() {
        let n = f.conduits.clamp(1, 64);
        let size = if f.conduit_in.is_finite() && f.conduit_in > 0.0 { f.conduit_in } else { 1.0 };
        let ends = (f.from_entry.eq_ignore_ascii_case("bottom"), f.entry.eq_ignore_ascii_case("bottom"));
        let what = format!("{} {}→{}", f.tag, f.from, f.to);
        let Some(a) = one(&f.from) else {
            left.push(format!("{what}: not one placed piece of gear at its source"));
            continue;
        };
        let (b, src) = if let Some(b) = one(&f.to) {
            (b, format!("feeder {what}"))
        } else {
            let Some(chain) = chain_of.get(&i).cloned() else {
                let why = if typical.contains(&f.to.to_ascii_uppercase()) && !family(&f.to).is_empty() {
                    "no placed copy of it left to feed"
                } else {
                    "not one placed piece of gear at its end"
                };
                left.push(format!("{what}: {why}"));
                continue;
            };
            let names = chain.iter().map(|e| e.tag.as_str()).collect::<Vec<_>>().join("→");
            let src = format!("feeder {} {}→{}", f.tag, f.from, names);
            let Some(&b) = chain.first() else { continue };
            chained.push((i, chain, size, src.clone(), what.clone()));
            (b, src)
        };
        for _ in 0..n {
            legs.push(Leg { feeder: i, a, b, ends, size, src: src.clone(), what: what.clone() });
        }
    }
    // Where each conduit comes out of and goes into its gear: its place in the side's conduit area.
    let slots = entry_slots(&ctx, &legs);
    let mut failed: BTreeSet<usize> = BTreeSet::new();
    for (leg, at) in legs.iter().zip(slots) {
        if failed.contains(&leg.feeder) {
            continue;
        }
        if let Err(e) = ctx.gear_to_gear(leg.a, leg.b, leg.ends, at, leg.size, &leg.src, &mut runs) {
            left.push(format!("{}: {e}", leg.what));
            failed.insert(leg.feeder);
        }
    }
    for (i, chain, size, src, what) in chained {
        if failed.contains(&i) {
            continue;
        }
        for w in chain.windows(2) {
            if let [x, y] = w
                && let Err(e) = ctx.feed_through(x, y, size, &src, &mut runs)
            {
                left.push(format!("{what}: {e}"));
                break;
            }
        }
    }
    (runs, left)
}

/// Each typical panel schedule (one for many rooms) with how many of its copies are placed:
/// (tag, placed, typical of).
pub(crate) fn typical_placed(sp: &cadaddy_walk3d::BuildingSpec) -> Vec<(String, usize, u32)> {
    sp.mep
        .panel_schedules
        .iter()
        .filter_map(|p| {
            let of = p.typical_of?;
            let pre = format!("{}-", p.panel.to_ascii_uppercase());
            let placed = sp
                .mep
                .equipment
                .iter()
                .filter(|e| e.kind == "panel" && (e.tag.eq_ignore_ascii_case(&p.panel) || e.tag.to_ascii_uppercase().starts_with(&pre)))
                .count();
            Some((p.panel.clone(), placed, of))
        })
        .collect()
}

/// Like reasons said once, with how many (`123 DP2→G: … ×3`).
pub(crate) fn tally(items: &[String]) -> Vec<String> {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for v in items {
        match seen.iter_mut().find(|s| &s.0 == v) {
            Some(s) => s.1 += 1,
            None => seen.push((v.clone(), 1)),
        }
    }
    seen.into_iter().map(|(v, n)| if n > 1 { format!("{v} ×{n}") } else { v }).collect()
}

#[cfg(test)]
mod feeder_route_tests {
    use super::*;

    fn gear(tag: &str, level: &str, at: [f64; 2], bottom: f64, height: f64) -> cadaddy_walk3d::MepEquipment {
        cadaddy_walk3d::MepEquipment {
            tag: tag.into(),
            kind: "panel".into(),
            level: level.into(),
            min: [at[0] - 6.0, at[1] - 3.0],
            max: [at[0] + 6.0, at[1] + 3.0],
            bottom_ft: bottom,
            height_ft: height,
            ..Default::default()
        }
    }

    #[test]
    fn a_service_from_the_utilitys_transformer_runs_in_the_ground_at_its_cover() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        sp.levels = vec![
            serde_json::from_value(serde_json::json!({"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": [0.0, 0.0, 1200.0, 1200.0]}))
                .expect("level"),
        ];
        let mut x = gear("UTILITY-XFMR", "Level 1", [1400.0, 100.0], 0.0, 5.0);
        x.kind = "transformer".into();
        x.sources.insert(super::super::utilityxfmr::SITE_SOURCE.into(), "outside the building".into());
        x.properties.insert(super::super::utilityxfmr::COVER_PROPERTY.into(), "30 in (note)".into());
        sp.mep.equipment = vec![x, gear("MSB", "Level 1", [1100.0, 400.0], 0.0, 7.5)];
        sp.mep.feeders = vec![cadaddy_walk3d::Feeder {
            tag: "SV1".into(),
            from: "UTILITY-XFMR".into(),
            to: "MSB".into(),
            conduits: 2,
            sets: 2,
            conduit_in: 4.0,
            entry: "bottom".into(),
            from_entry: "bottom".into(),
            ..cadaddy_walk3d::Feeder::default()
        }];
        let (runs, left) = feeder_runs(&sp);
        assert!(left.is_empty(), "{left:?}");
        let across: Vec<&cadaddy_walk3d::MepRun> = runs.iter().filter(|r| r.points.len() >= 2).collect();
        assert_eq!(across.len(), 2, "one per conduit");
        for r in &across {
            assert_eq!(r.system, super::super::ugplan::E_UG);
            assert!(r.source_sheet.starts_with("feeder SV1 UTILITY-XFMR→MSB"), "{}", r.source_sheet);
            // 30 in cover and the 4 in conduit.
            assert!(r.bottoms_ft.iter().all(|b| (b + 34.0 / 12.0).abs() < 1e-9), "{:?}", r.bottoms_ft);
        }
        // A feeder between two pieces of gear in the building stays at the usual depth.
        sp.mep.equipment.retain(|e| e.tag != "UTILITY-XFMR");
        sp.mep.equipment.push(gear("UTILITY-XFMR", "Level 1", [900.0, 100.0], 0.0, 5.0));
        let (runs, _) = feeder_runs(&sp);
        assert!(runs.iter().filter(|r| r.points.len() >= 2).all(|r| r.bottoms_ft.iter().all(|b| (b + FEEDER_UNDER_FLOOR_FT).abs() < 1e-9)));
    }

    #[test]
    fn a_feeder_leaves_its_gear_crosses_and_rises_floor_to_floor_into_the_other() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (n, e, r) in [("Level 1", 0.0, [0.0, 0.0, 1200.0, 1200.0]), ("Level 3", 20.0, [5000.0, 0.0, 6200.0, 1200.0])] {
            sp.levels.push(
                serde_json::from_value(serde_json::json!({"name": n, "elevation_ft": e, "height_ft": 10.0, "ceiling_ft": 9.0, "region": r}))
                    .expect("level"),
            );
        }
        sp.mep.equipment = vec![
            gear("MDP", "Level 1", [100.0, 100.0], 0.0, 7.5),
            gear("DP1", "Level 1", [100.0, 400.0], 2.0, 4.0),
            gear("DP2", "Level 3", [5300.0, 100.0], 2.0, 4.0),
            gear("G", "Level 3", [5400.0, 100.0], 2.0, 3.0),
            gear("G", "Level 3", [5500.0, 100.0], 2.0, 3.0),
        ];
        let feeder = |to: &str, conduits: u32| cadaddy_walk3d::Feeder {
            tag: "604".into(),
            from: "MDP".into(),
            to: to.into(),
            conduits,
            conduit_in: 4.0,
            from_entry: "bottom".into(),
            entry: "top".into(),
            ..Default::default()
        };
        sp.mep.feeders = vec![feeder("DP1", 2), feeder("DP2", 1), feeder("G", 1)];
        let (runs, left) = feeder_runs(&sp);
        assert!(left.is_empty(), "G, two copies, has its nearer one fed: {left:?}");
        // DP1, same level, bottom out and top in: down, across under the floor, up into its top; twice.
        // The leg up into it is the buried raceway to the stub-up's top, the raceway above over it (Q6347).
        let dp1: Vec<_> = runs.iter().filter(|r| r.source_sheet.contains("DP1")).collect();
        assert_eq!(dp1.len(), 8, "{dp1:#?}");
        let stub = super::super::ugplan::rules().stubup.conduit_top_ft;
        let into: Vec<_> =
            dp1.iter().filter(|r| r.top_ft.is_some() && r.points[0][1] > 300.0).map(|r| (r.system.as_str(), r.bottoms_ft[0], r.top_ft)).collect();
        assert_eq!(into[..2], [(super::super::ugplan::E_UG, -1.5, Some(stub)), (FEEDER_SYSTEM, stub, Some(6.0))]);
        // DP2, two levels up: the riser beside it reaches its level's route (20 + 9 + 1 ft), cut
        // at the stub-up's top and at Level 3's floor (Q6347).
        let up = runs
            .iter()
            .filter(|r| {
                r.source_sheet.contains("DP2") && r.level == "Level 3" && r.points.len() == 1 && r.bottoms_ft == [0.0] && r.top_ft == Some(10.0)
            })
            .count();
        assert_eq!(up, 1, "{runs:#?}");
        assert!(runs.iter().any(|r| r.source_sheet.contains("DP2") && r.level == "Level 3" && r.top_ft == Some(10.0)), "down into DP2's top");
    }

    /// Q6347: a feeder from the ground floor's gear to a panel two floors up is built in its three
    /// parts: the buried leg with its stub-up (to the stub's top over the slab), the riser in the
    /// above-grade raceway on each level it passes (each floor a cut), and the run on the panel's
    /// level; none of the riser is the buried system or counted on the ground floor alone.
    #[test]
    fn a_feeder_to_an_upper_floor_rises_level_by_level_in_the_raceway_above_the_slab() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        // Unequal storeys, each level's plan in its own place on the sheet.
        sp.levels.clear();
        for (n, e, r) in [
            ("First", 0.0, [0.0, 0.0, 1200.0, 1200.0]),
            ("Second", 11.0, [3000.0, 0.0, 4200.0, 1200.0]),
            ("Third", 20.5, [6000.0, 500.0, 7200.0, 1700.0]),
        ] {
            sp.levels.push(
                serde_json::from_value(serde_json::json!({"name": n, "elevation_ft": e, "height_ft": 10.0, "ceiling_ft": 8.0, "region": r}))
                    .expect("level"),
            );
        }
        sp.mep.equipment = vec![gear("SWBD", "First", [100.0, 100.0], 0.0, 7.5), gear("P3", "Third", [6700.0, 1100.0], 2.0, 4.0)];
        sp.mep.feeders = vec![cadaddy_walk3d::Feeder {
            tag: "F1".into(),
            from: "SWBD".into(),
            to: "P3".into(),
            conduits: 1,
            conduit_in: 3.0,
            from_entry: "bottom".into(),
            entry: "top".into(),
            ..Default::default()
        }];
        let (runs, left) = feeder_runs(&sp);
        assert!(left.is_empty(), "{left:?}");
        let stub = super::super::ugplan::rules().stubup.conduit_top_ft;
        let vertical = |r: &&cadaddy_walk3d::MepRun| r.points.len() == 1 && r.top_ft.is_some();
        // The buried part: on the ground floor, below the stub's top.
        for r in runs.iter().filter(|r| r.system == super::super::ugplan::E_UG) {
            assert_eq!(r.level, "First", "{r:#?}");
            assert!(r.top_ft.unwrap_or(r.bottoms_ft[0]) <= stub + 1e-9, "the buried raceway stops at its stub-up: {r:#?}");
        }
        assert!(runs.iter().any(|r| r.system == super::super::ugplan::E_UG && vertical(&r) && r.top_ft == Some(stub)), "the stub-up: {runs:#?}");
        // The riser: in the above-grade raceway on every level it passes, floor to floor.
        let risers: Vec<_> = runs.iter().filter(|r| r.system == FEEDER_SYSTEM).filter(vertical).collect();
        let on = |l: &str| risers.iter().filter(|r| r.level == l).map(|r| (r.bottoms_ft[0], r.top_ft.unwrap_or(0.0))).collect::<Vec<_>>();
        assert_eq!(on("First"), vec![(stub, 11.0)], "{risers:#?}");
        assert_eq!(on("Second"), vec![(0.0, 9.5)], "{risers:#?}");
        // On the panel's level: up from its floor to the route over its ceiling, and down into the panel's top.
        assert_eq!(on("Third"), vec![(0.0, 9.0), (6.0, 9.0)], "{risers:#?}");
        // Each piece stands on its own level's plan, one stack: the same spot from each level's corner.
        let spot = |r: &cadaddy_walk3d::MepRun| {
            let c = sp.levels.iter().find(|l| l.name == r.level).and_then(|l| l.region).unwrap_or_default();
            ((r.points[0][0] - c[0]).round(), (r.points[0][1] - c[1]).round())
        };
        let stack: Vec<_> = risers.iter().filter(|r| r.level != "Third" || r.bottoms_ft[0] == 0.0).map(|r| spot(r)).collect();
        assert!(stack.windows(2).all(|w| w[0] == w[1]), "{stack:?}");
        // Nothing lost: the vertical feet are the floor-to-floor rise plus the ends.
        let up: f64 = runs.iter().filter(vertical).map(|r| r.top_ft.unwrap_or(0.0) - r.bottoms_ft[0]).sum();
        // Out of the gear's bottom (1.5), up to the third level's route (20.5 + 9 + 1.5), down into the top (3).
        assert!((up - (1.5 + 20.5 + 9.0 + 1.5 + 3.0)).abs() < 1e-9, "{up}");
    }

    #[test]
    fn feeders_to_a_typical_panel_each_feed_a_stack_of_its_copies_through_their_lugs() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (n, e, r) in [("Level 1", 0.0, [0.0, 0.0, 1200.0, 1200.0]), ("Level 2", 10.0, [5000.0, 0.0, 6200.0, 1200.0])] {
            sp.levels.push(
                serde_json::from_value(serde_json::json!({"name": n, "elevation_ft": e, "height_ft": 10.0, "ceiling_ft": 9.0, "region": r}))
                    .expect("level"),
            );
        }
        sp.mep.equipment = vec![
            gear("DP1", "Level 1", [100.0, 100.0], 2.0, 4.0),
            gear("G-101", "Level 1", [300.0, 600.0], 4.0, 3.0),
            gear("G-201", "Level 2", [5300.0, 602.0], 4.0, 3.0),
            gear("G-102", "Level 1", [900.0, 600.0], 4.0, 3.0),
            gear("G-202", "Level 2", [5900.0, 600.0], 4.0, 3.0),
        ];
        sp.mep.panel_schedules = vec![cadaddy_walk3d::PanelSchedule {
            panel: "DP1".into(),
            circuits: vec![cadaddy_walk3d::PanelCircuit { number: "1".into(), description: "PANELBOARD G (2 UNITS)".into(), ..Default::default() }],
            ..Default::default()
        }];
        let feeder = || cadaddy_walk3d::Feeder {
            tag: "123".into(),
            from: "DP1".into(),
            to: "G".into(),
            conduits: 1,
            conduit_in: 2.0,
            entry: "top".into(),
            ..Default::default()
        };
        sp.mep.feeders = vec![feeder(), feeder(), feeder()];
        let (runs, left) = feeder_runs(&sp);
        // Two stacks of two: one to each of two feeders, the nearer stack to the first; none left for the third.
        assert_eq!(left, vec!["123 DP1→G: no placed copy of it left to feed".to_string()]);
        assert!(runs.iter().any(|r| r.source_sheet == "feeder 123 DP1→G-101→G-201"), "{runs:#?}");
        assert!(runs.iter().any(|r| r.source_sheet == "feeder 123 DP1→G-102→G-202"));
        // Through G-101's lugs: out of its top (7 ft), through the floor into G-201's bottom (10 + 4 ft).
        // Cut at the floor (Q6347): Level 1's to its ceiling side of the floor, Level 2's from its floor.
        let through = runs.iter().filter(|r| r.source_sheet.contains("G-101") && r.level == "Level 1" && r.top_ft == Some(10.0)).collect::<Vec<_>>();
        assert_eq!(through.len(), 1, "{runs:#?}");
        assert_eq!(through[0].bottoms_ft[0], 7.0);
        assert!(runs.iter().any(|r| r.source_sheet.contains("G-101") && r.level == "Level 2" && r.bottoms_ft == [0.0] && r.top_ft == Some(4.0)));
        // G-201 sits 2 inches off G-101 on plan: no leg for that.
        assert!(!runs.iter().any(|r| r.source_sheet.contains("G-101") && r.level == "Level 2" && r.points.len() >= 2));
        // Every route is its feeder's (Q6201: its conductors ride it), a typical copy's too.
        let mut with_runs = sp.clone();
        with_runs.mep.runs = runs.clone();
        assert!(runs.iter().all(|r| super::super::mepatoms::build::feeder_of_run(&with_runs, r) == Some(0)), "{runs:#?}");
        assert_eq!(tally(&[left.clone(), left.clone(), left.clone()].concat()), vec!["123 DP1→G: no placed copy of it left to feed ×3".to_string()]);
        sp.mep.panel_schedules.push(cadaddy_walk3d::PanelSchedule { panel: "G".into(), typical_of: Some(124), ..Default::default() });
        assert_eq!(typical_placed(&sp), vec![("G".to_string(), 4, 124)]);
    }

    #[test]
    fn a_copy_off_its_stack_is_fed_square_over_the_ceiling_never_across_the_room_at_the_gears_height() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (n, e, r) in [("Level 1", 0.0, [0.0, 0.0, 1200.0, 1200.0]), ("Level 2", 10.0, [5000.0, 0.0, 6200.0, 1200.0])] {
            sp.levels.push(
                serde_json::from_value(serde_json::json!({"name": n, "elevation_ft": e, "height_ft": 10.0, "ceiling_ft": 9.0, "region": r}))
                    .expect("level"),
            );
        }
        // G-201 stands 2' east and 2'-6" north of G-101 on plan: off its stack in both directions.
        sp.mep.equipment = vec![
            gear("DP1", "Level 1", [100.0, 100.0], 2.0, 4.0),
            gear("G-101", "Level 1", [300.0, 600.0], 4.0, 3.0),
            gear("G-201", "Level 2", [5324.0, 630.0], 4.0, 3.0),
        ];
        sp.mep.panel_schedules = vec![cadaddy_walk3d::PanelSchedule {
            panel: "DP1".into(),
            circuits: vec![cadaddy_walk3d::PanelCircuit { number: "1".into(), description: "PANELBOARD G (2 UNITS)".into(), ..Default::default() }],
            ..Default::default()
        }];
        sp.mep.feeders = vec![cadaddy_walk3d::Feeder {
            tag: "123".into(),
            from: "DP1".into(),
            to: "G".into(),
            conduits: 1,
            conduit_in: 2.0,
            entry: "top".into(),
            ..Default::default()
        }];
        let (runs, _) = feeder_runs(&sp);
        let through: Vec<_> = runs.iter().filter(|r| r.source_sheet.contains("G-101→G-201")).collect();
        assert!(!through.is_empty(), "{runs:#?}");
        // Every level piece square on plan.
        for r in &runs {
            for w in r.points.windows(2) {
                let (dx, dy) = ((w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs());
                assert!(dx < 1e-6 || dy < 1e-6, "a diagonal: {r:#?}");
            }
        }
        // Out of G-101's top (7') up into Level 1's ceiling space (its feeders' route height, as
        // DP1's leg runs), square across to under G-201, and up through the floor into its
        // bottom (10' + 4'); no leg on Level 2 at the gear's height.
        let h = runs
            .iter()
            .find(|r| r.top_ft.is_none() && r.points.first() != Some(&[300.0, 600.0]))
            .and_then(|r| r.bottoms_ft.first().copied())
            .expect("route height");
        assert!(h > 7.0 && h < 10.0, "{h}");
        let level: Vec<_> = through.iter().filter(|r| r.top_ft.is_none() && r.points.first() == Some(&[300.0, 600.0])).collect();
        assert_eq!(level.len(), 1, "{level:#?}");
        assert_eq!((level[0].level.as_str(), level[0].points.len()), ("Level 1", 3), "one L: {level:#?}");
        assert!(level[0].bottoms_ft.iter().all(|b| (b - h).abs() < 1e-9), "{level:#?}");
        assert!(through.iter().any(|r| r.top_ft == Some(h) && r.bottoms_ft == vec![7.0]), "{through:#?}");
        // Cut at Level 2's floor (Q6347): Level 1's piece to the floor, Level 2's on its own plan into G-201's bottom.
        assert!(
            through.iter().any(|r| r.level == "Level 1" && r.top_ft == Some(10.0) && r.bottoms_ft == vec![h] && r.points == vec![[324.0, 630.0]]),
            "{through:#?}"
        );
        assert!(
            through.iter().any(|r| r.level == "Level 2" && r.top_ft == Some(4.0) && r.bottoms_ft == vec![0.0] && r.points == vec![[5324.0, 630.0]]),
            "{through:#?}"
        );
    }

    #[test]
    fn conduits_enter_side_by_side_in_the_conduit_area_ordered_by_where_they_go() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        sp.levels.push(
            serde_json::from_value(
                serde_json::json!({"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [0, 0, 1200, 1200]}),
            )
            .expect("level"),
        );
        // DP1 a 20" x 6" box (along x); its top area 16" x 4.25".
        let mut dp1 = gear("DP1", "Level 1", [500.0, 500.0], 2.0, 4.0);
        (dp1.min, dp1.max) = ([490.0, 497.0], [510.0, 503.0]);
        dp1.properties.insert("TOP CONDUIT AREA".into(), "16 W x 4.25 D in, 68 sq in; derived".into());
        sp.mep.equipment = vec![
            dp1,
            gear("W", "Level 1", [100.0, 500.0], 2.0, 4.0),
            gear("E", "Level 1", [900.0, 500.0], 2.0, 4.0),
            gear("MDP", "Level 1", [500.0, 900.0], 0.0, 7.5),
        ];
        let feeder = |tag: &str, from: &str, to: &str, conduits: u32| cadaddy_walk3d::Feeder {
            tag: tag.into(),
            from: from.into(),
            to: to.into(),
            conduits,
            conduit_in: 2.0,
            entry: "top".into(),
            from_entry: "top".into(),
            ..Default::default()
        };
        sp.mep.feeders = vec![feeder("1", "DP1", "E", 1), feeder("2", "DP1", "W", 1), feeder("3", "MDP", "DP1", 5)];
        let (runs, left) = feeder_runs(&sp);
        assert!(left.is_empty(), "{left:?}");
        // The risers out of and into DP1's top: seven, each its own place in the area.
        let at_dp1: Vec<[f64; 2]> = runs
            .iter()
            .filter(|r| r.top_ft.is_some() && r.points.first().is_some_and(|p| (p[0] - 500.0).abs() < 10.0 && (p[1] - 500.0).abs() < 5.0))
            .filter_map(|r| r.points.first().copied())
            .collect();
        assert_eq!(at_dp1.len(), 7, "{runs:#?}");
        for (k, p) in at_dp1.iter().enumerate() {
            assert!(p[0] >= 492.0 && p[0] <= 508.0 && p[1] >= 497.0 && p[1] <= 503.0, "in the 16 x 4.25 area: {p:?}");
            for q in at_dp1.iter().skip(k + 1) {
                assert!(((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() >= 2.197 - 1e-6, "a 2\" conduit's width apart: {p:?} {q:?}");
            }
        }
        // In the order of where they go: the one going west first (the front row's west end),
        // the one going east last (the back row, after the five from MDP straight north).
        let out_of =
            |src: &str| runs.iter().find(|r| r.source_sheet == src && r.top_ft.is_some()).and_then(|r| r.points.first().copied()).expect(src);
        let (w, e) = (out_of("feeder 2 DP1→W"), out_of("feeder 1 DP1→E"));
        assert!(at_dp1.iter().all(|p| w[0] <= p[0] + 1e-9 && w[1] <= p[1] + 1e-9), "{w:?} {at_dp1:?}");
        assert!(at_dp1.iter().all(|p| e[1] >= p[1] - 1e-9), "{e:?} {at_dp1:?}");
        // Seven 2" conduits don't fit one 16" row (six do): a second row behind the first.
        let ys: BTreeSet<i64> = at_dp1.iter().map(|p| (p[1] * 100.0).round() as i64).collect();
        assert_eq!(ys.len(), 2, "{at_dp1:?}");
    }

    /// Levels `Level 1`… one over another, 10 ft apart, all their plans at one place.
    fn levels(n: usize) -> Vec<cadaddy_walk3d::Level> {
        (1..=n)
            .map(|k| {
                serde_json::from_value(
                    serde_json::json!({"name": format!("Level {k}"), "elevation_ft": 10.0 * (k as f64 - 1.0), "height_ft": 10.0, "ceiling_ft": 9.0}),
                )
                .expect("level")
            })
            .collect()
    }

    /// A copy of typical panel `T` in room `<level><stack>` (`T-203`), as the typical-room plans
    /// stamp it, with the room's outline around it.
    fn copy(t: &str, level: usize, stack: usize, x: f64, rooms: &mut BTreeMap<String, (String, Vec<Vec2>, bool)>) -> cadaddy_walk3d::MepEquipment {
        let room = format!("{level}{stack:02}");
        let mut g = gear(&format!("{t}-{room}"), &format!("Level {level}"), [x, 600.0], 4.0, 3.0);
        g.properties.insert("ROOM".into(), room.clone());
        let o = vec![Vec2::new(x - 120.0, 480.0), Vec2::new(x + 120.0, 480.0), Vec2::new(x + 120.0, 900.0), Vec2::new(x - 120.0, 900.0)];
        rooms.insert(room, (format!("Level {level}"), o, true));
        g
    }

    /// Feeders read as the job reads them: a schedule row of each source naming `G`, the one-line's type.
    fn schedule_feeders(sp: &cadaddy_walk3d::BuildingSpec) -> Vec<cadaddy_walk3d::Feeder> {
        let types = vec![FeederType {
            tag: "604".into(),
            conduits: 1,
            conduit_in: 1.25,
            conductors: 3,
            size: "#2".into(),
            material: "cu".into(),
            ground_size: "#8".into(),
            source: "FEEDER SCHEDULE".into(),
        }];
        let incoming: BTreeMap<String, String> = [("G".to_string(), "604".to_string())].into_iter().collect();
        plan(sp, &types, &incoming, &BTreeMap::new(), &[]).feeders
    }

    fn row(n: &str, d: &str) -> cadaddy_walk3d::PanelCircuit {
        cadaddy_walk3d::PanelCircuit { number: n.into(), description: d.into(), ..Default::default() }
    }

    #[test]
    fn breaker_labels_read_a_source_panel_and_its_first_circuit() {
        assert_eq!(breaker_label("DP1-3"), Some(("DP1".to_string(), 3)));
        assert_eq!(breaker_label(" dp-2-11 "), Some(("DP-2".to_string(), 11)));
        assert_eq!(breaker_label("DP1-3,5,7"), Some(("DP1".to_string(), 3)));
        for not in ["G-7a", "ROOM 101", "DP1", "-3", "3-4", "DP1-", "PANEL DP1-3"] {
            assert_eq!(breaker_label(not), None, "{not}");
        }
        // The breakers feeding a typical panel: rows naming it, by their first circuit number.
        let schedules = vec![
            cadaddy_walk3d::PanelSchedule {
                panel: "DP1".into(),
                circuits: vec![row("1,3,5", "PANELBOARD G (4 UNITS)"), row("2", "PANEL H"), row("7", "RECEPTACLES")],
                ..Default::default()
            },
            cadaddy_walk3d::PanelSchedule { panel: "G".into(), typical_of: Some(16), ..Default::default() },
            cadaddy_walk3d::PanelSchedule { panel: "H".into(), ..Default::default() },
        ];
        let b = typical_breakers(&schedules);
        assert_eq!(b.into_iter().collect::<Vec<_>>(), vec![(("DP1".to_string(), 1), "G".to_string())]);
    }

    /// Q6436: two distribution panels on two floors, four stacks of four copies of typical panel
    /// G; each stack's breaker lettered in one of its rooms names a source that is NOT the nearest.
    /// Each stack is fed from its labelled breaker, and every copy records its feed.
    #[test]
    fn typical_stacks_are_fed_by_the_breakers_their_plan_labels_name_not_the_nearest_source() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, levels: levels(4), ..Default::default() };
        let mut rooms = BTreeMap::new();
        // DP1 on the first floor beside stacks 3 and 4; DP2 on the second beside stacks 1 and 2.
        sp.mep.equipment = vec![gear("DP1", "Level 1", [4300.0, 100.0], 2.0, 6.0), gear("DP2", "Level 2", [700.0, 100.0], 2.0, 6.0)];
        for level in 1..=4 {
            for stack in 1..=4 {
                sp.mep.equipment.push(copy("G", level, stack, 1000.0 * stack as f64, &mut rooms));
            }
        }
        sp.mep.panel_schedules = vec![
            cadaddy_walk3d::PanelSchedule {
                panel: "DP1".into(),
                circuits: vec![row("1", "PANELBOARD G (4 UNITS)"), row("2", "PANELBOARD G (4 UNITS)")],
                ..Default::default()
            },
            cadaddy_walk3d::PanelSchedule {
                panel: "DP2".into(),
                circuits: vec![row("1", "PANELBOARD G (4 UNITS)"), row("2", "PANELBOARD G (4 UNITS)")],
                ..Default::default()
            },
            cadaddy_walk3d::PanelSchedule { panel: "G".into(), typical_of: Some(16), ..Default::default() },
        ];
        sp.mep.feeders = schedule_feeders(&sp);
        assert_eq!(sp.mep.feeders.len(), 4, "{:#?}", sp.mep.feeders);
        // The labels, one per stack, lettered in its second-floor room (and one in a corridor).
        let label = |text: &str, stack: usize| PlanLabel {
            text: text.into(),
            level: "2".into(),
            at: [1000.0 * stack as f64 + 30.0, 700.0],
            sheet: "E-2".into(),
        };
        let labels = vec![
            label("DP1-1", 1),
            label("DP1-2", 2),
            label("DP2-1", 3),
            label("DP2-2", 4),
            PlanLabel { text: "DP2-2".into(), level: "2".into(), at: [2500.0, 300.0], sheet: "E-2".into() },
        ];
        let rep = label_typical_copies(&mut sp.mep.equipment, &labels, &rooms);
        assert_eq!(rep, LabelReport { labelled: 4, unmatched: 1, conflicts: Vec::new() });
        assert_eq!(sp.mep.equipment.iter().find(|e| e.tag == "G-201").and_then(|e| e.properties.get(FEEDER_LABEL)), Some(&"DP1-1".to_string()));

        let (runs, left) = feeder_runs(&sp);
        assert!(left.is_empty(), "{left:?}");
        let fed = |src: &str, stack: usize| {
            let copies: Vec<String> = (1..=4).map(|l| format!("G-{l}{stack:02}")).collect();
            runs.iter().any(|r| {
                let Some(rest) = r.source_sheet.strip_prefix(&format!("feeder 604 {src}→")) else { return false };
                let names: BTreeSet<&str> = rest.split('→').collect();
                names.len() == 4 && copies.iter().all(|c| names.contains(c.as_str()))
            })
        };
        // By distance DP1 would feed stacks 3 and 4: the labels say 1 and 2.
        assert!(
            fed("DP1", 1) && fed("DP1", 2) && fed("DP2", 3) && fed("DP2", 4),
            "{:#?}",
            runs.iter().map(|r| &r.source_sheet).collect::<BTreeSet<_>>()
        );

        // Every copy is its own gear end with its feed recorded on it.
        let rep = record_typical_feeds(&mut sp);
        assert_eq!((rep.by_label, rep.proposed, rep.unfed), (16, 0, 0), "{rep:?}");
        let fed_by = |t: &str| sp.mep.equipment.iter().find(|e| e.tag == t).and_then(|e| e.properties.get(FED_BY)).cloned().unwrap_or_default();
        assert_eq!(fed_by("G-101"), "feeder 604 from DP1 circuit 1");
        assert_eq!(fed_by("G-201"), "G-101's feed-through lugs (feeder 604 from DP1 circuit 1)");
        assert_eq!(fed_by("G-404"), "G-304's feed-through lugs (feeder 604 from DP2 circuit 2)");
        let src = sp.mep.equipment.iter().find(|e| e.tag == "G-303").and_then(|e| e.sources.get("fed by")).cloned().unwrap_or_default();
        assert_eq!(src, "the plan's breaker label DP2-1 in its stack");
        let into = typical_feeder_in(&sp);
        assert_eq!(into.len(), 16);
        // A rerun records the same, once.
        let again = record_typical_feeds(&mut sp);
        assert_eq!((again.by_label, again.proposed), (16, 0));

        // Without the labels: the nearest stacks, said proposed (the earlier behaviour).
        for e in sp.mep.equipment.iter_mut() {
            e.properties.remove(FEEDER_LABEL);
        }
        let (runs, _) = feeder_runs(&sp);
        let near = |src: &str, stack: usize| {
            runs.iter().any(|r| r.source_sheet.starts_with(&format!("feeder 604 {src}→")) && r.source_sheet.contains(&format!("G-1{stack:02}")))
        };
        assert!(near("DP1", 4) && near("DP2", 1), "{:#?}", runs.iter().map(|r| &r.source_sheet).collect::<BTreeSet<_>>());
        let rep = record_typical_feeds(&mut sp);
        assert_eq!((rep.by_label, rep.proposed, rep.unfed), (0, 16, 0));
        let src = sp.mep.equipment.iter().find(|e| e.tag == "G-303").and_then(|e| e.sources.get("fed by")).cloned().unwrap_or_default();
        assert!(src.starts_with("proposed: the stack nearest"), "{src}");
    }

    /// A stack taller than the units a breaker serves: the label's breaker takes the copies
    /// nearest its label, the rest go to the breaker no label names.
    #[test]
    fn a_label_takes_no_more_copies_than_its_breaker_serves_and_the_rest_are_proposed() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, levels: levels(8), ..Default::default() };
        let mut rooms = BTreeMap::new();
        sp.mep.equipment = vec![gear("DP1", "Level 1", [100.0, 100.0], 2.0, 6.0)];
        for level in 1..=8 {
            sp.mep.equipment.push(copy("G", level, 1, 1000.0, &mut rooms));
        }
        sp.mep.panel_schedules = vec![
            cadaddy_walk3d::PanelSchedule {
                panel: "DP1".into(),
                circuits: vec![row("1", "PANELBOARD G (4 UNITS)"), row("3", "PANELBOARD G (4 UNITS)")],
                ..Default::default()
            },
            cadaddy_walk3d::PanelSchedule { panel: "G".into(), typical_of: Some(8), ..Default::default() },
        ];
        sp.mep.feeders = schedule_feeders(&sp);
        let labels = vec![PlanLabel { text: "DP1-3".into(), level: "Level 7".into(), at: [1000.0, 700.0], sheet: "E-7".into() }];
        assert_eq!(label_typical_copies(&mut sp.mep.equipment, &labels, &rooms).labelled, 1);
        let (feeds, notes) = typical_feeds(&sp);
        assert!(notes.is_empty(), "{notes:?}");
        let tags = |f: &TypicalFeed| f.chain.iter().map(|&e| sp.mep.equipment[e].tag.clone()).collect::<Vec<_>>();
        let by_label = feeds.iter().find(|f| f.label.is_some()).expect("labelled");
        assert_eq!(sp.mep.feeders[by_label.feeder].source.rsplit(' ').next(), Some("3"));
        assert_eq!(tags(by_label), ["G-501", "G-601", "G-701", "G-801"]);
        let proposed = feeds.iter().find(|f| f.label.is_none()).expect("proposed");
        assert_eq!(tags(proposed), ["G-101", "G-201", "G-301", "G-401"]);
    }

    /// A room with two different breaker labels takes neither; a label naming a breaker that feeds
    /// no typical panel is said, never followed.
    #[test]
    fn two_labels_in_a_room_and_a_label_of_no_feeder_are_said_not_guessed() {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, levels: levels(2), ..Default::default() };
        let mut rooms = BTreeMap::new();
        sp.mep.equipment = vec![gear("DP1", "Level 1", [100.0, 100.0], 2.0, 6.0)];
        for stack in 1..=2 {
            for level in 1..=2 {
                sp.mep.equipment.push(copy("G", level, stack, 1000.0 * stack as f64, &mut rooms));
            }
        }
        sp.mep.panel_schedules = vec![
            cadaddy_walk3d::PanelSchedule {
                panel: "DP1".into(),
                circuits: vec![row("1", "PANELBOARD G (2 UNITS)"), row("2", "PANELBOARD G (2 UNITS)")],
                ..Default::default()
            },
            cadaddy_walk3d::PanelSchedule { panel: "G".into(), typical_of: Some(4), ..Default::default() },
        ];
        sp.mep.feeders = schedule_feeders(&sp);
        let at = |x: f64| [x, 700.0];
        let labels = vec![
            PlanLabel { text: "DP1-1".into(), level: "1".into(), at: at(1000.0), sheet: "E-1".into() },
            PlanLabel { text: "DP1-2".into(), level: "1".into(), at: at(1040.0), sheet: "E-1".into() },
            PlanLabel { text: "DP1-9".into(), level: "1".into(), at: at(2000.0), sheet: "E-1".into() },
        ];
        let rep = label_typical_copies(&mut sp.mep.equipment, &labels, &rooms);
        assert_eq!(rep.labelled, 1);
        assert_eq!(rep.conflicts, vec!["G-101: 2 labels in its room (DP1-1, DP1-2)".to_string()]);
        let (feeds, notes) = typical_feeds(&sp);
        assert_eq!(notes, vec!["G-102: its plan label DP1-9 names no feeder to G".to_string()]);
        // Both stacks still fed (proposed): no label taken, none guessed.
        assert_eq!(feeds.len(), 2);
        assert!(feeds.iter().all(|f| f.label.is_none() && f.chain.len() == 2), "{feeds:?}");
    }
}
