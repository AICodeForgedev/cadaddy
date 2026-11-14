//! Equipment connections against their submittals (Q6176): each load the one-line feeds (a
//! feeder-type tag with its protection above it and a label below that isn't a panel: `ELEV-1`,
//! `DOAS-1`), its approved submittal's electrical data (read from its text; a scanned submittal is
//! read by an agent, asked through the job), and the checks (`coderules.json` E-EQUIP-ELEC): its
//! protection no larger than the maker's maximum, its conductors' ampacity at least 125% of its
//! nameplate or full-load amps, its controller's short-circuit rating at least the available fault
//! current at its source. A difference is an RFI with both values and the fix.
//!
//! Q6646: each load the panel schedules feed by tag (two letters are a tag too: `EF-9`) with a
//! submittal: its file named for the tag, or for its kind (`EXHAUST FAN`, rule data) when a page
//! of it names the tag (alone or in a list: `EF-5, 6, 7`). Beyond the above, its protection no
//! smaller than its minimum circuit ampacity calls for (NEC 440.22, 430.52), its conductors of
//! the material and least size its notes name (copper only, `4 AWG Wire Min.`; NEC 110.3(B)), a
//! disconnect within sight of each placed unit (its own, unless its submittal says it brings one;
//! NEC 440.14, 430.102(B)), and the service receptacle and data connection its notes ask for.
//! Every difference is an RFI citing both documents, a corrected one too; what is corrected is
//! built (the breaker, the conductors, the missing connection beside the unit) with the
//! drawings' value kept beside it.

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::EquipmentElectrical;

/// A load on a one-line: its label, the feeder type tag above it, the protection above that.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OnelineLoad {
    pub tag: String,
    pub feeder: String,
    pub ocpd: String,
}

/// `70A/3P` → (70, 3).
pub(crate) fn ocpd_of(v: &str) -> Option<(f64, u32)> {
    let u = v.trim().to_ascii_uppercase();
    let (a, p) = u.split_once('/')?;
    let amps: f64 = a.trim().trim_end_matches('A').trim().parse().ok()?;
    let poles: u32 = p.trim().trim_end_matches('P').trim().parse().ok()?;
    (amps > 0.0 && (1..=3).contains(&poles)).then_some((amps, poles))
}

/// The loads on a one-line's texts: below each feeder-type tag (`types`), the nearest label that
/// isn't a panel (`panels`), and above it the nearest protection (`70A/3P`).
pub(crate) fn oneline_loads(texts: &[TextItem], panels: &[String], types: &[String]) -> Vec<OnelineLoad> {
    let mut out: Vec<OnelineLoad> = Vec::new();
    for t in texts.iter().filter(|t| types.iter().any(|x| x.eq_ignore_ascii_case(t.value.trim()))) {
        let h = t.height.max(1e-9);
        let below = texts
            .iter()
            .filter(|l| l.at.y < t.at.y && t.at.y - l.at.y < 14.0 * h && (l.at.x - t.at.x).abs() < 4.0 * h)
            .filter(|l| {
                let v = l.value.trim();
                v.len() <= 16
                    && v.chars().any(|c| c.is_ascii_alphabetic())
                    && !v.contains(' ')
                    && !panels.iter().any(|p| p.eq_ignore_ascii_case(v))
                    && !types.iter().any(|x| x.eq_ignore_ascii_case(v))
            })
            .min_by(|a, b| (t.at.y - a.at.y).total_cmp(&(t.at.y - b.at.y)));
        let Some(label) = below else { continue };
        // A label right under the tag that names a panel was filtered; the next text down a gear box's.
        let ocpd = texts
            .iter()
            .filter(|o| o.at.y > t.at.y && o.at.y - t.at.y < 10.0 * h && (o.at.x - t.at.x).abs() < 4.0 * h && ocpd_of(&o.value).is_some())
            .min_by(|a, b| (a.at.y - t.at.y).total_cmp(&(b.at.y - t.at.y)))
            .map(|o| o.value.trim().to_string())
            .unwrap_or_default();
        let tag = label.value.trim().to_string();
        if !out.iter().any(|x| x.tag.eq_ignore_ascii_case(&tag)) {
            out.push(OnelineLoad { tag, feeder: t.value.trim().to_string(), ocpd });
        }
    }
    out
}

/// A number written next to its key, after it (`SCCR: 10 kAmp`, `NAMEPLATE AMPS: 42`) or before it
/// (`74.6 Amps MCA`, `80 Amps MOP`); kA counts a thousand amps.
fn number_at(up: &str, keys: &[&str]) -> Option<f64> {
    let words: Vec<&str> = up.split(' ').filter(|w| !w.is_empty()).collect();
    let bare = |w: &str| w.trim_matches([':', '=', '(', ')', ',']).to_string();
    // A word's leading number (`10000RMS` → 10000, `74.6` → 74.6).
    let num = |w: &str| {
        let t = bare(w);
        let n: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
        n.replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
    };
    let kilo = |w: &str| {
        let u = bare(w);
        u.starts_with("KA") || u.starts_with("KAMP") || u.starts_with("KAIC")
    };
    for k in keys {
        let kw: Vec<String> = k.split(' ').map(bare).collect();
        for i in 0..words.len() {
            if !kw.iter().enumerate().all(|(j, x)| words.get(i + j).is_some_and(|w| bare(w) == *x)) {
                continue;
            }
            // Before, when a unit sits between: `74.6 Amps MCA`, `80 Amps MOP`.
            let back = |n: usize| i.checked_sub(n).and_then(|j| words.get(j)).copied();
            if let Some(u) = back(1).filter(|w| matches!(bare(w).as_str(), "AMPS" | "AMP" | "A" | "KA" | "KAMP" | "KAMPS"))
                && let Some(v) = back(2).and_then(num)
            {
                return Some(if bare(u).starts_with('K') { v * 1000.0 } else { v });
            }
            // Right after (a colon allowed): `SCCR: 10 kAmp`, `NAMEPLATE AMPS: 42`; or after its
            // unit: `MOPA  A.  20`, `MOP (A) 15`.
            let mut after = i + kw.len();
            // Its abbreviation in brackets first (`Max. Overcurrent Protection (MOP) 15`).
            if words.get(after).is_some_and(|w| {
                let inner = w.trim_start_matches('(').trim_end_matches(')');
                w.starts_with('(') && w.ends_with(')') && inner.len() >= 2 && inner.chars().all(|c| c.is_ascii_alphabetic())
            }) {
                after += 1;
            }
            let mut thousand = false;
            if words.get(after).is_some_and(|w| matches!(bare(w).trim_end_matches('.'), "A" | "AMPS" | "AMP")) {
                after += 1;
            } else if words.get(after).is_some_and(|w| kilo(w)) {
                // `SCCR   kA   5K`: thousands.
                thousand = true;
                after += 1;
            }
            if let Some(v) = words.get(after).and_then(|w| num(w)) {
                let k = thousand || bare(words.get(after).copied().unwrap_or("")).ends_with('K') || words.get(after + 1).is_some_and(|w| kilo(w));
                return Some(if k { v * 1000.0 } else { v });
            }
            if thousand {
                continue;
            }
            // Right before, no unit: `46 MCA`.
            if let Some(v) = back(1).and_then(num) {
                return Some(v);
            }
        }
    }
    None
}

/// The electrical data a submittal's text gives (nameplate / full-load / minimum circuit amps,
/// maximum protection and fuse class, volts and phases, acceleration amps, SCCR). A motor's own
/// FLA inside a unit isn't the unit's: full-load amps count only when said in words.
pub(crate) fn text_electrical(text: &str) -> EquipmentElectrical {
    let up: String = text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    let mut e = EquipmentElectrical {
        nameplate_a: number_at(&up, &["NAMEPLATE AMPS", "NAMEPLATE CURRENT", "RATED CURRENT"]),
        fla: number_at(&up, &["FULL LOAD AMPS", "FULL LOAD CURRENT"]),
        mca: number_at(&up, &["MINIMUM CIRCUIT AMPACITY", "MIN. CIRCUIT AMPACITY", "MCA"]),
        max_ocpd_a: number_at(
            &up,
            &[
                "MAXIMUM OVERCURRENT PROTECTION",
                "MAX. OVERCURRENT PROTECTION",
                "MAX FUSE SIZE",
                "MAX FUSE",
                "MOCP",
                "MOPA",
                "MOP",
                "MAX OCPD",
                "FUSE (AMP)",
            ],
        ),
        accel_a: number_at(&up, &["MAX ACCEL AMPS", "ACCELERATION AMPS", "ACCELERATING CURRENT"]),
        sccr_a: number_at(&up, &["RATING (SCCR)", "SCCR"]),
        ..EquipmentElectrical::default()
    };
    for class in ["RK1", "RK5", "CLASS J", "CLASS T", "CLASS CC"] {
        if up.contains(class) {
            e.fuse_class = class.trim_start_matches("CLASS ").to_string();
            break;
        }
    }
    // Volts: a supply voltage said outright, else the first `208 V` / `480V` (100-600).
    e.volts = number_at(&up, &["SUPPLIED VOLTAGE"]).or_else(|| {
        let words: Vec<&str> = up.split(' ').collect();
        words.windows(2).find_map(|w| {
            let v = w[0].trim_end_matches('V').parse::<f64>().ok().filter(|v| (100.0..=600.0).contains(v))?;
            (w[0].ends_with('V') || w[1].starts_with('V')).then_some(v)
        })
    });
    e.phase = if ["3 PH", "3PH", "3-PHASE", "3 PHASE", "3 PHS", "3Ø"].iter().any(|p| up.contains(p)) {
        Some(3)
    } else if ["1 PH", "1PH", "SINGLE PHASE", "1 PHS"].iter().any(|p| up.contains(p)) {
        Some(1)
    } else {
        None
    };
    e
}

/// Whether a submittal's data says anything electrical.
pub(crate) fn has_data(e: &EquipmentElectrical) -> bool {
    e.nameplate_a.is_some() || e.fla.is_some() || e.mca.is_some() || e.max_ocpd_a.is_some() || e.sccr_a.is_some()
}

/// A failed check: what, both values, the fix, and each document's word (the drawings' and the
/// submittal's) for the RFI.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Conflict {
    pub check: &'static str,
    pub message: String,
    pub fix: String,
    pub drawings: String,
    pub submittal: String,
}

/// The checks a correction of the connection's protection or conductors settles ([`correct`]).
pub(crate) const CORRECTED_CHECKS: &[&str] = &["protection", "protection_min", "conductors", "conductor_material", "conductor_min"];
/// The checks of what is missing around a unit on its plans ([`lacking`]): built beside it.
pub(crate) const LACK_CHECKS: &[&str] = &["disconnect", "receptacle", "data"];

/// A conductor size as the ampacity table keys it (`#4` → `4`, `250 kcmil` → `250`).
fn size_key(size: &str) -> &str {
    size.trim().trim_start_matches('#').trim_end_matches(" kcmil").trim_end_matches(" KCMIL").trim()
}

/// The smallest standard protection (the rule's sizes) at or above `amps`.
fn standard_at_least(amps: f64, rule: &super::coderules::CodeRule) -> Option<f64> {
    rule.standard_ocpd_a.iter().copied().filter(|a| a.is_finite() && *a + 1e-9 >= amps).min_by(f64::total_cmp)
}

/// The protection a connection whose drawn breaker is under its minimum circuit ampacity takes:
/// the smallest standard size at or above it, never above the most its submittal allows.
fn least_protection(mca: f64, max: Option<f64>, rule: &super::coderules::CodeRule) -> Option<f64> {
    let at = standard_at_least(mca, rule)?;
    match max {
        Some(m) if at > m + 1e-9 => None,
        _ => Some(at),
    }
}

/// The checks of one connection: `feeder` the drawings' conductors (size, material), `afc` the
/// available fault current at its source panel (its schedule's AFC), the rule's data.
pub(crate) fn check(e: &EquipmentElectrical, feeder: Option<(&str, &str)>, afc: Option<f64>, rule: &super::coderules::CodeRule) -> Vec<Conflict> {
    let mut out = Vec::new();
    let drawn = format!("{} at {}", e.drawing_ocpd, if e.source_panel.is_empty() { "its panel" } else { e.source_panel.as_str() });
    let schedule = if e.source_panel.is_empty() { "the drawings".to_string() } else { format!("panel {} schedule", e.source_panel) };
    if let (Some((amps, _)), Some(max)) = (ocpd_of(&e.drawing_ocpd), e.max_ocpd_a)
        && amps > max + 1e-9
    {
        let class = if e.fuse_class.is_empty() { String::new() } else { format!(" Class {}", e.fuse_class) };
        out.push(Conflict {
            check: "protection",
            message: format!(
                "{}: the drawings protect it with {} at {}; its submittal allows at most {max:.0} A{class} ({})",
                e.tag, e.drawing_ocpd, e.source_panel, e.source
            ),
            fix: format!("{max:.0} A{class} fuses in its disconnect (a fused switch), or a breaker of {max:.0} A at {}", e.source_panel),
            drawings: format!("{schedule}: {drawn}"),
            submittal: format!("{}: maximum overcurrent protection {max:.0} A{class}", e.source),
        });
    }
    // Its protection no smaller than its minimum circuit ampacity calls for (NEC 440.22, 430.52).
    if let (Some((amps, _)), Some(mca)) = (ocpd_of(&e.drawing_ocpd), e.mca)
        && amps + 1e-9 < mca
    {
        let fix = match least_protection(mca, e.max_ocpd_a, rule) {
            Some(a) => format!("a {a:.0} A breaker at {} (the smallest standard size at or above its {mca} A MCA)", e.source_panel),
            None => format!("protection at or above its {mca} A MCA, within what its maker allows: ask the maker"),
        };
        out.push(Conflict {
            check: "protection_min",
            message: format!(
                "{}: the drawings protect it with {} at {}, under its submittal's minimum circuit ampacity {mca} A ({}) [{}]",
                e.tag, e.drawing_ocpd, e.source_panel, e.source, rule.min_ocpd_cite
            ),
            fix,
            drawings: format!("{schedule}: {drawn}"),
            submittal: format!("{}: minimum circuit ampacity {mca} A", e.source),
        });
    }
    let base = e.mca.or_else(|| e.fla.or(e.nameplate_a).map(|a| a * rule.conductor_factor.unwrap_or(1.25)));
    if let (Some(need), Some((size, material))) = (base, feeder) {
        let have = rule.ampacity_75c.get(material).and_then(|t| t.get(size_key(size)));
        if let Some(&have) = have
            && have + 1e-9 < need
        {
            out.push(Conflict {
                check: "conductors",
                message: format!(
                    "{}: its feeder's #{size} {material} carries {have:.0} A (75 C); its submittal needs {need:.0} A ({})",
                    e.tag, e.source
                ),
                fix: "Larger conductors (and conduit to suit)".into(),
                drawings: format!("{schedule}: #{} {material}", size_key(size)),
                submittal: format!("{}: {need:.0} A minimum circuit ampacity", e.source),
            });
        }
    }
    // The conductor material and least size its maker names (NEC 110.3(B)).
    if let Some((size, material)) = feeder {
        let mat = material.trim().to_ascii_lowercase();
        if e.copper_only && mat.starts_with("al") {
            out.push(Conflict {
                check: "conductor_material",
                message: format!(
                    "{}: the drawings feed it with #{} aluminium; its submittal ({}) calls for copper wire only [{}]",
                    e.tag,
                    size_key(size),
                    e.source,
                    rule.conductor_cite
                ),
                fix: "Copper conductors of the ampacity it needs (and conduit to suit)".into(),
                drawings: format!("{schedule}: #{} {material}", size_key(size)),
                submittal: format!("{}: copper conductors only", e.source),
            });
        }
        let have = rule.ampacity_75c.get(mat.as_str()).and_then(|t| t.get(size_key(size))).copied();
        let least = rule
            .ampacity_75c
            .get(if e.copper_only || mat.is_empty() { "cu" } else { mat.as_str() })
            .and_then(|t| t.get(size_key(&e.min_wire)))
            .copied();
        if !e.min_wire.is_empty()
            && let (Some(have), Some(least)) = (have, least)
            && have + 1e-9 < least
        {
            out.push(Conflict {
                check: "conductor_min",
                message: format!(
                    "{}: the drawings feed it with #{} {material}; its submittal ({}) calls for #{} at least [{}]",
                    e.tag,
                    size_key(size),
                    e.source,
                    size_key(&e.min_wire),
                    rule.conductor_cite
                ),
                fix: format!("#{} conductors at least (and conduit to suit)", size_key(&e.min_wire)),
                drawings: format!("{schedule}: #{} {material}", size_key(size)),
                submittal: format!("{}: #{} minimum", e.source, size_key(&e.min_wire)),
            });
        }
    }
    if let (Some(sccr), Some(afc)) = (e.sccr_a, afc)
        && sccr + 1e-9 < afc
    {
        out.push(Conflict {
            check: "sccr",
            message: format!(
                "{}: its controller's short-circuit current rating is {sccr:.0} A ({}); {} has {afc:.0} A available fault current",
                e.tag, e.source, e.source_panel
            ),
            fix: format!(
                "A fault current study at {} (its feeder lowers it): at most {sccr:.0} A there, or a listed series-rated combination / current-limiting fuses, or a controller rated for it",
                e.tag
            ),
            drawings: format!("{schedule}: {afc:.0} A available fault current"),
            submittal: format!("{}: short-circuit current rating {sccr:.0} A", e.source),
        });
    }
    out
}

/// A text as words for phrase matching: upper case, split at white space, each word's edge
/// punctuation trimmed (`Min.` → `MIN`), a space each side.
fn phrase_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '.' | ',' | ':' | ';' | '(' | ')' | '*' | '\u{2022}' | '"' | '\'')).to_ascii_uppercase())
        .filter(|w| !w.is_empty())
        .take(400_000)
        .collect()
}

/// Where `phrase` starts in `words` (each of its words in turn), every place.
fn phrase_at(words: &[String], phrase: &str) -> Vec<usize> {
    let p = phrase_words(phrase);
    if p.is_empty() || p.len() > words.len() {
        return Vec::new();
    }
    (0..=words.len() - p.len()).filter(|&i| p.iter().enumerate().all(|(j, w)| words.get(i + j) == Some(w))).collect()
}

/// A conductor size word (`4`, `#10`, `1/0`, `250`): as the ampacity table keys it.
fn wire_size(w: &str) -> Option<String> {
    let t = w.trim_start_matches('#');
    let ok = !t.is_empty()
        && (t.chars().all(|c| c.is_ascii_digit()) || t.split_once('/').is_some_and(|(a, b)| a.chars().all(|c| c.is_ascii_digit()) && b == "0"));
    (ok && t.len() <= 4).then(|| t.to_string())
}

/// What a submittal's notes ask of its wiring and connections (the rule's words): copper only,
/// the least conductor size near a "wire min" phrase (`4 AWG Wire Min.`, `Min. wire size #10`),
/// a disconnect that comes with the unit, a service receptacle, a data connection. Taken over
/// what `e` has already (a note said once is enough).
pub(crate) fn text_notes(e: &mut EquipmentElectrical, text: &str, rule: &super::coderules::CodeRule) {
    let words = phrase_words(text);
    let any = |list: &[String]| list.iter().any(|p| !phrase_at(&words, p).is_empty());
    e.copper_only |= any(&rule.copper_only_words);
    e.unit_disconnect |= any(&rule.unit_disconnect_words) || any(&rule.unit_disconnect_more_words);
    e.needs_receptacle |= any(&rule.receptacle_need_words);
    e.needs_data |= any(&rule.data_need_words);
    if e.min_wire.is_empty() {
        'found: for p in &rule.min_wire_words {
            let n = phrase_words(p).len();
            for at in phrase_at(&words, p) {
                // `<size> AWG` / `<size> KCMIL` just before it, else a size right after it.
                let before = (at.saturating_sub(3)..at).rev().find_map(|i| {
                    let unit = words.get(i + 1)?;
                    (unit == "AWG" || unit == "KCMIL").then(|| wire_size(words.get(i)?)).flatten()
                });
                let after = || {
                    (at + n..(at + n + 3).min(words.len())).find_map(|i| {
                        let w = words.get(i)?;
                        if w == "SIZE" || w == "AWG" { None } else { wire_size(w) }
                    })
                };
                if let Some(size) = before.or_else(after) {
                    e.min_wire = size;
                    break 'found;
                }
            }
        }
    }
}

/// Where a tag is named on a submittal page, the tag lists expanded: `XF-14`, `Mark: XF-15, 16,
/// 17`, `Unit Tags: XF-11, 12`, `AHU/CU-11,12,&15`, `CU-11 and 12`. Whether `tag` (`XF-16`) is one of
/// them. A list is numbers joined by commas, `&` or `and`; a number after a space alone (a
/// table's next cell: `EF-4   1   …`) is not.
pub(crate) fn names_tag(page: &str, tag: &str) -> bool {
    let want = tag.trim().to_ascii_uppercase();
    let Some((prefix, _)) = want.split_once('-') else { return false };
    if prefix.is_empty() {
        return false;
    }
    let up = page.to_ascii_uppercase();
    let b = up.as_bytes();
    let head = format!("{prefix}-");
    let digits = |mut i: usize| -> (usize, usize) {
        let s = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) && i - s < 6 {
            i += 1;
        }
        (s, i)
    };
    // A number's end: no letter or digit next, nor a decimal or a fraction (`1.5`, `1/4`).
    let word_end = |i: usize| {
        !b.get(i).is_some_and(|c| c.is_ascii_alphanumeric())
            && !(b.get(i).is_some_and(|c| *c == b'.' || *c == b'/') && b.get(i + 1).is_some_and(u8::is_ascii_digit))
    };
    let num = |s: usize, e: usize| up.get(s..e).map(|n| format!("{prefix}-{n}"));
    for (at, _) in up.match_indices(&head).take(10_000) {
        // Its own word: nothing of a longer tag before it (`/` before it is fine: `AHU/CU-14`).
        if at > 0 && b.get(at - 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'-') {
            continue;
        }
        let (s, mut i) = digits(at + head.len());
        if s == i || !word_end(i) {
            continue;
        }
        if num(s, i).as_deref() == Some(want.as_str()) {
            return true;
        }
        // The numbers listed after it, joined by `,`, `&` or `and`.
        for _ in 0..12 {
            let mut j = i;
            let mut joined = false;
            loop {
                match b.get(j) {
                    Some(b' ' | b'\t') => j += 1,
                    Some(b',' | b'&') => {
                        joined = true;
                        j += 1;
                    }
                    Some(b'A') if up.get(j..j + 4) == Some("AND ") => {
                        joined = true;
                        j += 3;
                    }
                    _ => break,
                }
            }
            let (s2, e2) = digits(j);
            if !joined || s2 == e2 || !word_end(e2) {
                break;
            }
            if num(s2, e2).as_deref() == Some(want.as_str()) {
                return true;
            }
            i = e2;
        }
    }
    false
}

/// A submittal's pages (`--- page N of M ---` between them; a text with none is one page).
pub(crate) fn pages(text: &str) -> Vec<&str> {
    if !text.contains("--- page ") {
        return vec![text];
    }
    text.split("--- page ").filter(|p| !p.trim().is_empty()).take(2_000).collect()
}

/// The pages of a submittal that name a tag, joined: its own section of a submittal of several
/// units. Empty: no page names it.
pub(crate) fn tag_pages(text: &str, tag: &str) -> String {
    pages(text).into_iter().filter(|p| names_tag(p, tag)).collect::<Vec<_>>().join("\n--- page ")
}

/// Circuits a submittal may name, at most.
const MAX_EXTRA_CIRCUITS: usize = 40;

/// The objects for the other circuits an equipment's submittal calls for (quantity above zero),
/// no drawing having them: a disconnect in a room named like the equipment, the design value and
/// the basis on it. Those the spec has already aren't built twice. What was built, and what has
/// no room to go in.
pub(crate) fn extra_circuit_tags(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    e: &EquipmentElectrical,
    rooms: &[(String, String, Vec<cadaddy_geom::Vec2>)],
    cite: &str,
) -> (Vec<String>, Vec<String>) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut words: Vec<String> =
        e.description.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 3).map(str::to_string).collect();
    words.extend(stem(&e.tag));
    let levels: Vec<String> = sp.levels.iter().map(|l| l.name.clone()).collect();
    let mut used = std::collections::BTreeMap::new();
    let (mut built, mut no_room) = (Vec::new(), Vec::new());
    for x in e.extra_circuits.iter().filter(|x| x.qty > 0) {
        let rating = match (x.volts, x.amps) {
            (Some(v), Some(a)) => format!("{v:.0}V {a:.0}A "),
            _ => String::new(),
        };
        let item = format!("{rating}disconnect, {}: {}", e.tag, x.name);
        for n in 0..x.qty {
            if sp.tags.iter().filter(|t| t.fields.get("device_type").is_some_and(|f| f.value == item)).count() >= x.qty as usize {
                break;
            }
            // A spot nothing else stands on (a tag's id is its place).
            let spot = (0..8).find_map(|_| {
                let (level, room, at) = super::coderules::spot_in_room(&words, rooms, &levels, &mut used)?;
                let id = format!("device:{level}:{:.1},{:.1}", at.x / upf, at.y / upf);
                Some((level, room, at, id))
            });
            let spot = spot.filter(|(_, _, _, id)| !sp.tags.iter().any(|t| t.id == *id)).or_else(|| {
                (0..7).find_map(|_| {
                    let (level, room, at) = super::coderules::spot_in_room(&words, rooms, &levels, &mut used)?;
                    let id = format!("device:{level}:{:.1},{:.1}", at.x / upf, at.y / upf);
                    (!sp.tags.iter().any(|t| t.id == id)).then_some((level, room, at, id))
                })
            });
            let Some((level, room, at, id)) = spot else {
                no_room.push(item.clone());
                break;
            };
            let mut t = cadaddy_walk3d::tags::ElementTag {
                id,
                kind: "equipment_connection".into(),
                level: level.clone(),
                at_ft: [at.x / upf, at.y / upf],
                ..Default::default()
            };
            t.set("type", "DISC", "rule");
            t.set("device_type", item.clone(), "submittal");
            let serves = format!("{}: {}", e.tag, x.name);
            t.set("serves", if n == 0 { serves } else { format!("{serves} #{}", n + 1) }, "submittal");
            t.set("sheet", e.source.clone(), "submittal");
            t.set("symbol", "0", "rule");
            t.set("design_value", "not on the drawings", "plan");
            t.set("corrected_value", item.clone(), "submittal");
            t.set("correction_reason", format!("{} calls for it; the drawings have no circuit for it", e.source), "submittal");
            t.set("correction_rule", cite, "rule");
            built.push(format!("{item} in {room} on {level}"));
            sp.tags.push(t);
        }
    }
    (built, no_room)
}

/// Units of one equipment looked at, at most.
const MAX_UNITS: usize = 2_000;

/// A placed unit of an equipment that lacks a connection its submittal or the code asks for
/// within reach on its plan: `disconnect` (within sight, NEC 440.14 / 430.102(B); a cord-and-plug
/// unit's receptacle is its disconnect), `receptacle` (a service receptacle), `data`. Its level
/// and box (plan units).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lack {
    pub check: &'static str,
    pub level: String,
    pub lo: [f64; 2],
    pub hi: [f64; 2],
    /// A cord-and-plug unit: its disconnect is a receptacle.
    pub cord: bool,
}

/// How far a point is from a box (0 inside it).
fn gap_to(lo: [f64; 2], hi: [f64; 2], p: [f64; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    dx.hypot(dy)
}

/// A device's words to tell what it is: its kind, type and device type.
fn device_words(t: &cadaddy_walk3d::tags::ElementTag) -> String {
    let f = |k: &str| t.fields.get(k).map(|v| v.value.as_str()).unwrap_or("");
    format!("{} {} {}", t.kind.replace('_', " "), f("type"), f("device_type")).to_ascii_uppercase()
}

/// The placed units of `e` that lack, on their plans, a connection their submittal or the code
/// asks for: a disconnect within sight (unless its submittal says it brings its own), a service
/// receptacle and a data connection within reach where its submittal asks for them. A drawn
/// connection is one unit's: the one its `equipment` field names, else the nearest piece of
/// equipment (a stand or support under a unit is not one).
pub(crate) fn lacking(
    sp: &cadaddy_walk3d::BuildingSpec,
    e: &EquipmentElectrical,
    rule: &super::coderules::CodeRule,
    disc: &super::equipdisc::Rule,
) -> Vec<Lack> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let sight = rule.sight_reach_ft.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(50.0).min(500.0) * upf;
    let aux = rule.aux_reach_ft.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(25.0).min(500.0) * upf;
    let is_support = |q: &cadaddy_walk3d::MepEquipment| q.properties.contains_key("UNIT") || q.kind.to_ascii_lowercase().contains("stand");
    let units: Vec<(usize, &cadaddy_walk3d::MepEquipment)> =
        sp.mep.equipment.iter().enumerate().filter(|(_, q)| q.tag.eq_ignore_ascii_case(&e.tag) && !is_support(q)).take(MAX_UNITS).collect();
    let mut out = Vec::new();
    for (i, u) in units {
        let words = format!("{} {} {} {}", u.kind.replace('_', " "), u.title, e.description, u.tag);
        let cord = disc.connection_for(&words) == "receptacle";
        let here: Vec<&cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().filter(|t| t.level.eq_ignore_ascii_case(&u.level)).collect();
        let at = |t: &cadaddy_walk3d::tags::ElementTag| [t.at_ft[0] * upf, t.at_ft[1] * upf];
        let lack = |check: &'static str| Lack { check, level: u.level.clone(), lo: u.min, hi: u.max, cord };
        if !e.unit_disconnect {
            // Each connection drawn on its level is its owner's: the equipment it names, else the
            // nearest; this unit is met by one of its own within sight.
            let met = here.iter().any(|t| {
                let w = device_words(t);
                let is_conn = disc.is_connection(&w) || (cord && w.contains("RECEPTACLE"));
                if !is_conn || gap_to(u.min, u.max, at(t)) > sight {
                    return false;
                }
                let named = t.fields.get("equipment").map(|f| f.value.trim().to_ascii_uppercase()).filter(|v| !v.is_empty());
                let owner = sp
                    .mep
                    .equipment
                    .iter()
                    .enumerate()
                    .filter(|(_, q)| q.level.eq_ignore_ascii_case(&u.level) && !is_support(q))
                    .filter(|(_, q)| named.as_deref().is_none_or(|n| q.tag.eq_ignore_ascii_case(n)))
                    .min_by(|a, b| gap_to(a.1.min, a.1.max, at(t)).total_cmp(&gap_to(b.1.min, b.1.max, at(t))))
                    .map(|(j, _)| j);
                owner == Some(i)
            });
            if !met {
                out.push(lack("disconnect"));
            }
        }
        if e.needs_receptacle && !here.iter().any(|t| device_words(t).contains("RECEPTACLE") && gap_to(u.min, u.max, at(t)) <= aux) {
            out.push(lack("receptacle"));
        }
        if e.needs_data
            && !here.iter().any(|t| {
                let w = device_words(t);
                (w.contains("DATA") || w.contains("LOW VOLTAGE OUTLET") || w.contains("TELECOM")) && gap_to(u.min, u.max, at(t)) <= aux
            })
        {
            out.push(lack("data"));
        }
    }
    out
}

/// What each kind of lack is, in words, and the object built for it: (the finding's words, the
/// tag kind, the device type).
fn lack_object(l: &Lack, tag: &str) -> (&'static str, &'static str, String) {
    match (l.check, l.cord) {
        ("disconnect", true) => {
            ("no receptacle (its disconnect: a cord-and-plug unit) within sight", "receptacle", format!("receptacle, {tag} cord connection"))
        }
        ("disconnect", false) => ("no disconnect within sight", "equipment_connection", format!("disconnect, {tag}")),
        ("receptacle", _) => ("no service receptacle within reach", "receptacle", format!("service receptacle, {tag}")),
        _ => ("no data connection within reach", "data_outlet", format!("data outlet, {tag}")),
    }
}

/// The conflicts of a unit's lacks, one per kind (the units counted), each document's word.
pub(crate) fn lack_conflicts(e: &EquipmentElectrical, lacks: &[Lack], rule: &super::coderules::CodeRule) -> Vec<Conflict> {
    let mut out = Vec::new();
    for check in ["disconnect", "receptacle", "data"] {
        let mine: Vec<&Lack> = lacks.iter().filter(|l| l.check == check).collect();
        let Some(first) = mine.first() else { continue };
        let (what, _, _) = lack_object(first, &e.tag);
        let mut levels: Vec<&str> = mine.iter().map(|l| l.level.as_str()).collect();
        levels.dedup();
        let (cite, asks) = match check {
            "disconnect" => (rule.sight_cite.as_str(), "names no disconnect on or in the unit".to_string()),
            "receptacle" => (rule.aux_cite.as_str(), "calls for a service receptacle".to_string()),
            _ => (rule.aux_cite.as_str(), "calls for a data connection".to_string()),
        };
        out.push(Conflict {
            check: match check {
                "disconnect" => "disconnect",
                "receptacle" => "receptacle",
                _ => "data",
            },
            message: format!("{}: {what} on {} ({} placed); its submittal ({}) {asks} [{cite}]", e.tag, levels.join(", "), mine.len(), e.source),
            fix: match check {
                "disconnect" if first.cord => "A receptacle for its cord beside it".to_string(),
                "disconnect" => "A disconnect within sight of it (a safety switch beside the unit), on its circuit".to_string(),
                "receptacle" => "A 125 V service receptacle within 25 ft, on a circuit of its own".to_string(),
                _ => "A data outlet and its pathway at the unit".to_string(),
            },
            drawings: format!("the plans of {}: {what}", levels.join(", ")),
            submittal: format!("{}: {asks}", e.source),
        });
    }
    out
}

/// The objects for the lacks: each beside its unit on its plan (a disconnect to one side, a
/// receptacle to the other, a data outlet below), the design value ("not on the drawings"), the
/// corrected value and the rule on it; `circuit` the unit's circuit (a disconnect's). Not built
/// twice. What was built.
pub(crate) fn build_lacking(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    e: &EquipmentElectrical,
    lacks: &[Lack],
    circuit: Option<&str>,
    rule: &super::coderules::CodeRule,
) -> Vec<String> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut built = Vec::new();
    for l in lacks.iter().take(MAX_UNITS) {
        let (what, kind, item) = lack_object(l, &e.tag);
        let mid = (l.lo[1] + l.hi[1]) / 2.0 / upf;
        let base = match (l.check, l.cord) {
            ("disconnect", false) => [l.hi[0] / upf + 1.0, mid],
            ("data", _) => [(l.lo[0] + l.hi[0]) / 2.0 / upf, l.lo[1] / upf - 1.0],
            _ => [l.lo[0] / upf - 1.0, mid],
        };
        // A spot no other tag stands on (a tag's id is its place).
        let spot = (0..8).map(|k| [base[0], base[1] + 0.5 * k as f64]).find_map(|p| {
            let id = format!("device:{}:{:.1},{:.1}", l.level, p[0], p[1]);
            (!sp.tags.iter().any(|t| t.id == id)).then_some((id, p))
        });
        let Some((id, at_ft)) = spot else { continue };
        if sp.tags.iter().any(|t| {
            t.level.eq_ignore_ascii_case(&l.level)
                && t.fields.get("device_type").is_some_and(|f| f.value == item)
                && (t.at_ft[0] - at_ft[0]).hypot(t.at_ft[1] - at_ft[1]) < 4.0
        }) {
            continue;
        }
        let cite = match l.check {
            "disconnect" => rule.sight_cite.as_str(),
            _ => rule.aux_cite.as_str(),
        };
        let mut t = cadaddy_walk3d::tags::ElementTag { id, kind: kind.into(), level: l.level.clone(), at_ft, ..Default::default() };
        if kind == "equipment_connection" {
            t.set("type", "DISC", "rule");
        }
        t.set("device_type", item.clone(), "submittal");
        t.set("equipment", e.tag.clone(), "submittal");
        t.set("serves", e.tag.clone(), "submittal");
        if let Some(c) = circuit.filter(|c| !c.is_empty() && l.check == "disconnect") {
            t.set("circuit", c, "schedule");
        }
        t.set("sheet", e.source.clone(), "submittal");
        t.set("symbol", "0", "rule");
        t.set("design_value", "not on the drawings", "plan");
        t.set("corrected_value", item.clone(), "rule");
        t.set("correction_reason", format!("{}: {what} on {}; built beside it ({})", e.tag, l.level, e.source), "rule");
        t.set("correction_rule", cite, "rule");
        built.push(format!("{item} on {}", l.level));
        sp.tags.push(t);
    }
    built
}

/// What is built instead of the drawings' where the submittal differs, from the same data the
/// checks use: its protection (the most the submittal allows; under its MCA, the smallest
/// standard size at or above it), its conductors (the smallest with the ampacity it needs, in
/// copper where its notes say copper only, at least the size they name) and their conduit by
/// fill. Nothing to correct: empty. An equipment's
/// short-circuit rating is no one thing to build: a study stays a question.
pub(crate) fn correct(e: &mut EquipmentElectrical, feeder: Option<(&str, &str)>, rule: &super::coderules::CodeRule) {
    let mut why = Vec::new();
    if let (Some((amps, poles)), Some(max)) = (ocpd_of(&e.drawing_ocpd), e.max_ocpd_a)
        && amps > max + 1e-9
        && max > 0.0
    {
        let class = if e.fuse_class.is_empty() { String::new() } else { format!(" Class {} fuses", e.fuse_class) };
        e.corrected_ocpd = format!("{max:.0}A/{poles}P{class}");
        why.push(format!("drawings {}, its submittal allows at most {max:.0} A", e.drawing_ocpd));
    }
    // A breaker under its minimum circuit ampacity: the smallest standard size at or above it.
    if let (Some((amps, poles)), Some(mca)) = (ocpd_of(&e.drawing_ocpd), e.mca)
        && amps + 1e-9 < mca
        && let Some(at) = least_protection(mca, e.max_ocpd_a, rule)
    {
        e.corrected_ocpd = format!("{at:.0}A/{poles}P");
        why.push(format!("drawings {}, under its submittal's {mca} A minimum circuit ampacity", e.drawing_ocpd));
    }
    let need = e.mca.or_else(|| e.fla.or(e.nameplate_a).map(|a| a * rule.conductor_factor.unwrap_or(1.25)));
    if let Some((size, material)) = feeder {
        let mat = material.trim().to_ascii_lowercase();
        // Copper where its submittal says copper only; its own material else.
        let target = if e.copper_only && mat.starts_with("al") { "cu".to_string() } else { mat.clone() };
        let have = rule.ampacity_75c.get(mat.as_str()).and_then(|t| t.get(size_key(size))).copied();
        let mut want = 0.0f64;
        let mut parts = Vec::new();
        if let (Some(need), Some(h)) = (need, have)
            && h + 1e-9 < need
        {
            want = want.max(need);
            parts.push(format!("feeder's #{} {material} is under the {need:.0} A it needs", size_key(size)));
        }
        if target != mat {
            want = want.max(have.unwrap_or(0.0)).max(need.unwrap_or(0.0));
            parts.push(format!("feeder's #{} {material}: its submittal calls for copper only", size_key(size)));
        }
        let least = rule.ampacity_75c.get(target.as_str()).and_then(|t| t.get(size_key(&e.min_wire))).copied();
        if !e.min_wire.is_empty()
            && let (Some(h), Some(l)) = (have, least)
            && h + 1e-9 < l
        {
            want = want.max(l);
            parts.push(format!("feeder's #{} {material} is under the #{} its submittal calls for", size_key(size), size_key(&e.min_wire)));
        }
        if want > 0.0
            && let Some(table) = rule.ampacity_75c.get(target.as_str())
            && let Some((bigger, a)) = table.iter().filter(|(_, a)| **a + 1e-9 >= want).min_by(|x, y| x.1.total_cmp(y.1))
        {
            let phases = e.phase.unwrap_or(3).clamp(1, 3);
            let hots = if phases == 1 { 2 } else { phases };
            e.corrected_conductors = format!("{hots} #{bigger} {target}");
            why.push(format!("{}; #{bigger} {target} carries {a:.0} A (75 C)", parts.join("; ")));
            // Its conduit for those conductors and a ground (of the same size at most).
            let rl = super::conduitroute::rules();
            let fill = [(hots, bigger.clone(), true), (1, bigger.clone(), false)];
            if let Some((trade, _, _)) = super::conduitroute::fill_size(&fill, "EMT", 0.0, rl) {
                e.corrected_conduit = format!("{}\" EMT", super::mepatoms::trade_size(trade));
            }
        }
    }
    if !why.is_empty() {
        e.correction_reason = format!("{}: {} ({})", e.tag, why.join("; "), e.source);
        e.correction_rule = rule.cite.clone();
    }
}

/// The load its submittal says it draws, VA: its nameplate amps (else full-load, else minimum
/// circuit amps) at its volts and phase. None: no volts or no amps.
pub(crate) fn submitted_va(e: &EquipmentElectrical) -> Option<f64> {
    let volts = e.volts.filter(|v| v.is_finite() && *v > 0.0)?;
    let amps = e.nameplate_a.or(e.fla).or(e.mca).filter(|a| a.is_finite() && *a > 0.0)?;
    Some(volts * amps * if e.phase == Some(3) { 3f64.sqrt() } else { 1.0 })
}

/// A schedule row takes its load from a submittal, the printed load kept beside it, unless within
/// 1% of it (rounding, not a correction). What it had, when it changed a printed load.
pub(crate) fn take_submitted_load(c: &mut cadaddy_walk3d::PanelCircuit, va: f64) -> Option<f64> {
    match c.load_va {
        Some(was) if (was - va).abs() <= 0.01 * was.abs() => None,
        Some(was) => {
            c.design_load_va = Some(was);
            c.load_va = Some(va);
            Some(was)
        }
        None => {
            c.load_va = Some(va);
            None
        }
    }
}

/// The load's word to find its submittal and schedule row by: its label's letters (`ELEV-1` →
/// `ELEV`), at least three.
pub(crate) fn stem(tag: &str) -> Option<String> {
    let s: String = tag.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    (s.len() >= 3).then_some(s)
}

/// A submittal path's revision order: `Rev 2-0` → (2, 0); none → (0, 0).
pub(crate) fn revision(path: &str) -> (u32, u32) {
    let up = path.to_ascii_uppercase();
    up.match_indices("REV ")
        .filter_map(|(i, _)| {
            let r = up.get(i + 4..)?;
            let (a, b) = r.split_once('-')?;
            let b: String = b.chars().take_while(char::is_ascii_digit).collect();
            Some((a.trim().parse().ok()?, b.parse().ok()?))
        })
        .max()
        .unwrap_or((0, 0))
}

/// What a submittal's review stamp says (in its text): the option checked (`X` or a box before
/// it) when the stamp lists them all; the one option when only one is there; else unknown (empty).
pub(crate) fn review_status(text: &str) -> String {
    let up: String = text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    let options = [
        ("REVISE AND RESUBMIT", "revise and resubmit"),
        ("REVISE & RESUBMIT", "revise and resubmit"),
        ("RETURN FOR CORRECTIONS", "revise and resubmit"),
        ("REJECTED", "rejected"),
        ("EXCEPTIONS AS NOTED", "exceptions as noted"),
        ("APPROVED AS NOTED", "exceptions as noted"),
        ("NO EXCEPTIONS TAKEN", "no exceptions taken"),
    ];
    for (k, v) in options {
        // The mark as a word of its own right before the option.
        let marked =
            ["X", "[X]", "\u{2612}", "\u{2713}", "\u{2714}"].iter().any(|m| up.starts_with(&format!("{m} {k}")) || up.contains(&format!(" {m} {k}")));
        if marked {
            return v.into();
        }
    }
    let present: Vec<&str> = options.iter().filter(|(k, _)| up.contains(k)).map(|(_, v)| *v).collect();
    match present.as_slice() {
        [one] => (*one).into(),
        _ => String::new(),
    }
}

/// The data an agent read off a scanned submittal (a job answer).
pub(crate) fn from_answer(a: &serde_json::Value) -> EquipmentElectrical {
    let n = |k: &str| a.get(k).and_then(serde_json::Value::as_f64).filter(|v| v.is_finite() && *v > 0.0);
    EquipmentElectrical {
        volts: n("volts"),
        phase: n("phase").map(|p| p as u32),
        nameplate_a: n("nameplate_a"),
        fla: n("fla"),
        mca: n("mca"),
        max_ocpd_a: n("max_ocpd_a"),
        fuse_class: a.get("fuse_class").and_then(serde_json::Value::as_str).unwrap_or_default().to_ascii_uppercase(),
        accel_a: n("accel_a"),
        hp: n("hp"),
        kw: n("kw"),
        sccr_a: n("sccr_a"),
        copper_only: a.get("copper_only").and_then(serde_json::Value::as_bool).unwrap_or(false),
        min_wire: a.get("min_wire").and_then(serde_json::Value::as_str).and_then(wire_size).unwrap_or_default(),
        unit_disconnect: a.get("unit_disconnect").and_then(serde_json::Value::as_bool).unwrap_or(false),
        needs_receptacle: a.get("needs_receptacle").and_then(serde_json::Value::as_bool).unwrap_or(false),
        needs_data: a.get("needs_data").and_then(serde_json::Value::as_bool).unwrap_or(false),
        extra_circuits: a
            .get("extra_circuits")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_EXTRA_CIRCUITS)
            .filter_map(|c| {
                Some(cadaddy_walk3d::ExtraCircuit {
                    name: c.get("name")?.as_str()?.trim().to_string(),
                    volts: c.get("volts").and_then(serde_json::Value::as_f64),
                    amps: c.get("amps").and_then(serde_json::Value::as_f64),
                    qty: c.get("qty").and_then(serde_json::Value::as_u64).map_or(1, |q| q.min(20) as u32),
                })
            })
            .collect(),
        source: a.get("source").and_then(serde_json::Value::as_str).unwrap_or("an agent's reading of the submittal").to_string(),
        ..EquipmentElectrical::default()
    }
}

/// The schedule row naming a load (its description, its source panel): its tag, else a
/// description that is the load (`ELEVATOR` for `ELEV-1`: it starts with the tag's letters), not
/// one that only mentions it (`LTG - ELEVATOR PIT`).
pub(crate) fn schedule_row(sp: &cadaddy_walk3d::BuildingSpec, tag: &str) -> Option<(String, String)> {
    // A tag of two letters (`EF-9`) has no stem to go by: its tag alone.
    let st = stem(tag);
    let mut best: Option<(u8, String, String)> = None;
    for p in &sp.mep.panel_schedules {
        for c in &p.circuits {
            let up = c.description.to_ascii_uppercase();
            let words: Vec<&str> = up.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').filter(|w| !w.is_empty()).collect();
            // Its tag; else a description that is only the load (`ELEVATOR`); else one that starts
            // with it (`ELEVATOR SUMP PUMP` is a pump, ranked below).
            let rank = if words.iter().any(|w| w.eq_ignore_ascii_case(tag)) {
                4
            } else if st.as_deref().is_some_and(|st| words.len() == 1 && words.first().is_some_and(|w| w.starts_with(st))) {
                3
            } else if st.as_deref().is_some_and(|st| words.first().is_some_and(|w| w.starts_with(st))) {
                2
            } else {
                0
            };
            if rank > 0 && best.as_ref().is_none_or(|b| rank > b.0) {
                best = Some((rank, c.description.clone(), p.panel.clone()));
            }
        }
    }
    best.map(|(_, d, p)| (d, p))
}

/// A text line's cells (the PDF text's column breaks).
fn cells(line: &str) -> Vec<&str> {
    line.split("   ").map(str::trim).filter(|c| !c.is_empty()).collect()
}

/// A number in a cell (`3,600` → 3600, `19.9` → 19.9).
fn cell_number(c: &str) -> Option<f64> {
    c.trim().replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Whether a cell reads like a maker's model number: letters and digits, at least six.
fn model_like(c: &str) -> bool {
    c.len() >= 6
        && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        && c.chars().any(|ch| ch.is_ascii_digit())
        && c.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// The model a submittal's item schedule gives a tag: the line that starts with the tag
/// (`PTAC-1   127   9000   208/230V   PDH09K3SGR3`), its last model-like cell.
pub(crate) fn model_of(text: &str, tag: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let c = cells(l);
        (c.first().is_some_and(|f| f.eq_ignore_ascii_case(tag)) && c.len() >= 2)
            .then(|| c.iter().skip(1).rev().find(|x| model_like(x)).map(|x| x.to_string()))?
    })
}

/// A model's column in a submittal's tables: each row under a `Model` line that names it, with as
/// many cells as that line, its label and the model's value (to the next `Model` line).
pub(crate) fn model_column(text: &str, model: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at: Option<(usize, usize)> = None;
    for l in text.lines().take(20_000) {
        let c = cells(l);
        if c.first().is_some_and(|f| f.eq_ignore_ascii_case("MODEL")) {
            at = c.iter().position(|x| x.eq_ignore_ascii_case(model)).map(|i| (i, c.len()));
            continue;
        }
        let Some((i, n)) = at else { continue };
        if c.len() == n
            && let (Some(label), Some(v)) = (c.first(), c.get(i))
        {
            out.push((label.to_string(), v.to_string()));
        }
    }
    out
}

/// A unit's power-cord rating for its heater at its voltage, from the cord table's rows
/// (`PXPCZ23020A   230/208   20   19.9 3,600`: cord, volts, branch circuit amps, MCA and watts):
/// the row at the voltage whose watts are the heater's (within a fifth). (Branch amps, MCA.)
pub(crate) fn cord_rating(text: &str, heater_kw: f64, volts: &str) -> Option<(f64, f64)> {
    if !(heater_kw.is_finite() && heater_kw > 0.0) {
        return None;
    }
    let want = heater_kw * 1000.0;
    text.lines()
        .take(20_000)
        .filter_map(|l| {
            let c = cells(l);
            if c.len() < 4 || !c.get(1).is_some_and(|v| v.contains(volts)) || !c.first().is_some_and(|x| model_like(x)) {
                return None;
            }
            let branch = c.get(2).and_then(|x| cell_number(x))?;
            let rest: Vec<f64> = c.get(3)?.split_whitespace().filter_map(cell_number).collect();
            let (mca, watts) = (*rest.first()?, *rest.last()?);
            ((watts - want).abs() <= 0.2 * want && rest.len() >= 2).then_some((branch, mca, (watts - want).abs()))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(b, m, _)| (b, m))
}

/// A multi-model submittal's data for a tag: its model (the item schedule), the model's column
/// (voltage, phase, heater size) and, for a unit wired by a cord, the cord rating for its heater
/// at the lower of its voltages (its branch circuit amps the most protection, its MCA).
pub(crate) fn submittal_for_model(text: &str, tag: &str) -> Option<EquipmentElectrical> {
    let model = model_of(text, tag)?;
    let col = model_column(text, &model);
    if col.is_empty() {
        return None;
    }
    let row = |words: &[&str]| {
        col.iter().find(|(l, _)| {
            let u = l.to_ascii_uppercase();
            words.iter().all(|w| u.contains(w))
        })
    };
    let volts_cell = row(&["VOLTAGE"]).map(|(_, v)| v.clone()).unwrap_or_default();
    // `230/208`: the lower, the building's.
    let volts = volts_cell.split('/').filter_map(cell_number).fold(f64::INFINITY, f64::min);
    let phase = row(&["VOLTAGE"]).and_then(|(l, _)| l.to_ascii_uppercase().contains("1 PHASE").then_some(1));
    let heater = row(&["HEATER", "KW"]).and_then(|(_, v)| cell_number(v));
    let mut e =
        EquipmentElectrical { tag: tag.to_string(), volts: volts.is_finite().then_some(volts), phase, kw: heater, ..EquipmentElectrical::default() };
    if let (Some(kw), true) = (heater, volts.is_finite()) {
        let key = format!("{volts:.0}");
        if let Some((branch, mca)) = cord_rating(text, kw, &key) {
            e.max_ocpd_a = Some(branch);
            e.mca = Some(mca);
        }
    }
    e.source = format!("model {model}");
    Some(e)
}

/// Every connection's data merged: a value the text gives over nothing.
pub(crate) fn merge(into: &mut EquipmentElectrical, from: &EquipmentElectrical) {
    macro_rules! take {
        ($f:ident) => {
            if into.$f.is_none() {
                into.$f = from.$f;
            }
        };
    }
    take!(volts);
    take!(phase);
    take!(nameplate_a);
    take!(fla);
    take!(mca);
    take!(max_ocpd_a);
    take!(accel_a);
    take!(hp);
    take!(kw);
    take!(sccr_a);
    if into.fuse_class.is_empty() {
        into.fuse_class.clone_from(&from.fuse_class);
    }
    if into.source.is_empty() {
        into.source.clone_from(&from.source);
    }
    if into.extra_circuits.is_empty() {
        into.extra_circuits.clone_from(&from.extra_circuits);
    }
    into.copper_only |= from.copper_only;
    into.unit_disconnect |= from.unit_disconnect;
    into.needs_receptacle |= from.needs_receptacle;
    into.needs_data |= from.needs_data;
    if into.min_wire.is_empty() {
        into.min_wire.clone_from(&from.min_wire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equip_rule() -> super::super::coderules::CodeRule {
        super::super::coderules::rules().into_iter().find(|r| r.id == "E-EQUIP-ELEC").unwrap_or_default()
    }

    /// Q6646: a maker's data sheet writes its unit between the key and the number (`MOPA  A.  20`,
    /// `MOP (A) 15`), or calls the most protection MOPA: read as the others.
    #[test]
    fn a_data_sheets_mopa_and_unit_between_forms_are_read() {
        let e = text_electrical("Voltage, Phase, Cycle V/Ph/Hz 208-230/1/60\nMCA   A.   19\nMOPA   A.   20\n");
        assert_eq!((e.mca, e.max_ocpd_a), (Some(19.0), Some(20.0)), "{e:?}");
        let f = text_electrical("MOP (A) 15\nMCA (A) 13.4\nMOP - Maximum Overcurrent Protection   MCA - Minimum Circuit Ampacity\n");
        assert_eq!((f.mca, f.max_ocpd_a), (Some(13.4), Some(15.0)), "{f:?}");
        let g = text_electrical("Min. Circuit Ampacity (MCA)   4\nMax. Overcurrent Protection (MOP)   15\nShort Circuit Current Rtg (SCCR)   5 kA\n");
        assert_eq!((g.mca, g.max_ocpd_a, g.sccr_a), (Some(4.0), Some(15.0), Some(5000.0)), "{g:?}");
        // The forms read before still read.
        let k = text_electrical("MCA   A.   15\nMOPA   A.   15\nSCCR   kA   5K\n");
        assert_eq!((k.mca, k.max_ocpd_a, k.sccr_a), (Some(15.0), Some(15.0), Some(5000.0)), "{k:?}");
        let h = text_electrical("Unit Main Input   74.6 Amps MCA - 80 Amps MOP - 208 V");
        assert_eq!((h.mca, h.max_ocpd_a), (Some(74.6), Some(80.0)), "{h:?}");
    }

    /// A submittal of several units names each on its own pages, alone or in a list joined by
    /// commas, `&` or `and` (`XF-15, 16, 17`, `AHU/CU-11,12,&15`); a number a table puts in the next
    /// cell, a decimal or a fraction after a tag is none of its list.
    #[test]
    fn a_tag_is_named_alone_or_in_a_list() {
        assert!(names_tag("Mark: XF-14", "XF-14"));
        assert!(names_tag("Mark: XF-15, 16, 17", "XF-16"));
        assert!(names_tag("Unit Tags:   XF-11, 12, 13, 14, RX-1, 2", "XF-14"));
        assert!(names_tag("AHU/CU-11,12,13,&15 Coordinate", "CU-15"));
        assert!(names_tag("AHU/CU-14", "CU-14"));
        assert!(names_tag("units cu-11 and 12", "CU-12"));
        assert!(!names_tag("AHU/CU-11,12,13,&15", "CU-14"));
        assert!(!names_tag("XF-14   1   ZZ-19   Nominal   1.5   16", "XF-1"), "a table's next cell");
        assert!(!names_tag("XF-14, 1.5 HP", "XF-1"), "a decimal");
        assert!(!names_tag("XF-14, 1/4 HP", "XF-1"), "a fraction");
        assert!(!names_tag("RXF-14 and DXF-14", "XF-14"), "part of a longer tag");
        assert!(!names_tag("XF-141", "XF-14"));
        assert!(!names_tag("anything", "NOTAG"));
        let text = "--- page 1 of 3 ---\nUnit Tags: XF-12, 9\n--- page 2 of 3 ---\nMark: XF-9\nMCA 4\n--- page 3 of 3 ---\nMark: XF-12\nMCA 9\n";
        let mine = tag_pages(text, "XF-9");
        assert!(mine.contains("MCA 4") && !mine.contains("MCA 9"), "{mine}");
        assert!(tag_pages(text, "XF-7").is_empty());
    }

    /// The notes of a submittal (the rule's words): copper only, the least wire size, a disconnect
    /// that comes with the unit, a service receptacle and a data connection to provide.
    #[test]
    fn a_submittals_notes_say_what_its_wiring_and_connections_need() {
        let rule = equip_rule();
        let mut e = EquipmentElectrical::default();
        text_notes(
            &mut e,
            "Unit Main Input 74.6 Amps MCA \u{2022} 80 Amps MOP \u{2022} 208 V \u{2022} 4 AWG Wire Min.\n\
             4. CONNECTION FROM BREAKER TO UNITS SAFETY DISCONNECT SWITCH TO BE COPPER WIRE ONLY.\n\
             GFCI 15 amp Convenience Outlet. 120V power supply by others.\n",
            &rule,
        );
        assert!(e.copper_only && e.needs_receptacle && !e.needs_data && !e.unit_disconnect, "{e:?}");
        assert_eq!(e.min_wire, "4");
        let mut f = EquipmentElectrical::default();
        text_notes(&mut f, "Plug type disconnect - Adjustable mounting brackets\nMin. wire size #10\nBMS data connection (RJ45) by others", &rule);
        assert!(f.unit_disconnect && f.needs_data && !f.copper_only, "{f:?}");
        assert_eq!(f.min_wire, "10");
        let mut g = EquipmentElectrical::default();
        text_notes(&mut g, "Copper fins, aluminium casing. Disconnect by others.", &rule);
        assert!(!g.copper_only && !g.unit_disconnect && g.min_wire.is_empty(), "{g:?}");
    }

    /// Q6646: a breaker under its MCA, aluminium where copper only is asked, conductors under the
    /// least size: each a conflict with both documents' words, and corrected.
    #[test]
    fn under_mca_copper_only_and_least_wire_are_conflicts_and_corrected() {
        let rule = equip_rule();
        let e = EquipmentElectrical {
            tag: "HP-9".into(),
            source_panel: "QP".into(),
            drawing_ocpd: "15A/2P".into(),
            mca: Some(19.0),
            max_ocpd_a: Some(30.0),
            phase: Some(1),
            source: "submittal Split Systems.pdf".into(),
            ..Default::default()
        };
        let c = check(&e, Some(("12", "cu")), None, &rule);
        let under = c.iter().find(|c| c.check == "protection_min").expect("under its MCA");
        assert!(under.message.contains("19") && under.fix.contains("20 A"), "{under:?}");
        assert!(under.drawings.starts_with("panel QP schedule: 15A/2P") && under.submittal.contains("minimum circuit ampacity 19"), "{under:?}");
        let mut fixed = e.clone();
        correct(&mut fixed, Some(("12", "cu")), &rule);
        assert_eq!(fixed.corrected_ocpd, "20A/2P", "{fixed:?}");
        // Copper only, fed in aluminium; and a least size the drawings' conductors are under.
        let f = EquipmentElectrical {
            tag: "RTU-7".into(),
            source_panel: "DPX".into(),
            drawing_ocpd: "80A/3P".into(),
            mca: Some(74.6),
            max_ocpd_a: Some(80.0),
            copper_only: true,
            min_wire: "2".into(),
            source: "submittal Rooftop Unit.pdf".into(),
            ..Default::default()
        };
        let c = check(&f, Some(("2", "al")), None, &rule);
        let checks: Vec<&str> = c.iter().map(|c| c.check).collect();
        assert!(checks.contains(&"conductor_material") && checks.contains(&"conductor_min"), "{checks:?}");
        let mut fixed = f.clone();
        correct(&mut fixed, Some(("2", "al")), &rule);
        assert!(fixed.corrected_conductors.ends_with(" cu") && fixed.corrected_conductors.starts_with("3 #"), "{fixed:?}");
        assert!(fixed.correction_reason.contains("copper only"), "{fixed:?}");
        // Copper, of the least size and more: nothing.
        assert!(check(&f, Some(("1", "cu")), None, &rule).iter().all(|c| c.check != "conductor_material" && c.check != "conductor_min"));
    }

    fn roof_spec(tags: serde_json::Value) -> cadaddy_walk3d::BuildingSpec {
        let v = serde_json::json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "Roof", "elevation_ft": 30, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
            "mep": {"equipment": [
                {"tag": "HP-9", "kind": "heat_pump", "level": "Roof", "min": [600, 240], "max": [636, 276], "height_ft": 3},
                {"tag": "HP-9 STAND", "kind": "equipment_stand", "level": "Roof", "min": [600, 240], "max": [636, 276], "height_ft": 1, "properties": {"UNIT": "HP-9"}},
                {"tag": "HP-8", "kind": "heat_pump", "level": "Roof", "min": [672, 240], "max": [708, 276], "height_ft": 3}
            ]},
            "tags": tags
        });
        cadaddy_walk3d::BuildingSpec::from_json(&v.to_string()).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Q6646: a unit's disconnect within sight is one drawn for it (its own, not its neighbour's
    /// beside it); its submittal saying it brings its own is enough; a service receptacle within
    /// reach where its submittal asks for one. What is missing is built beside it, once.
    #[test]
    fn a_units_disconnect_is_its_own_within_sight() {
        let rule = equip_rule();
        let disc = super::super::equipdisc::Rule::read();
        let e = EquipmentElectrical { tag: "HP-9".into(), mca: Some(19.0), source: "submittal Split Systems.pdf".into(), ..Default::default() };
        // HP-8's disconnect beside HP-8 (6 ft from HP-9: within 50 ft, but HP-8's).
        let neighbours = serde_json::json!([{"id": "device:Roof:60.0,21.5", "kind": "equipment_connection", "level": "Roof", "at_ft": [60.0, 21.5],
            "fields": {"type": {"value": "DISC", "source": "plan"}, "device_type": {"value": "disconnect", "source": "plan"}}}]);
        let sp = roof_spec(neighbours.clone());
        let lacks = lacking(&sp, &e, &rule, &disc);
        assert_eq!(lacks.iter().map(|l| l.check).collect::<Vec<_>>(), vec!["disconnect"], "{lacks:?}");
        // One drawn naming HP-9 a few feet off it: met.
        let mut own = neighbours.as_array().cloned().unwrap_or_default();
        own.push(serde_json::json!({"id": "device:Roof:49.0,21.5", "kind": "equipment_connection", "level": "Roof", "at_ft": [49.0, 21.5],
            "fields": {"equipment": {"value": "HP-9", "source": "plan"}}}));
        assert!(lacking(&roof_spec(serde_json::Value::Array(own)), &e, &rule, &disc).is_empty());
        // Its own disconnect said by its submittal: met; a receptacle asked for and none near: lacking.
        let f = EquipmentElectrical { unit_disconnect: true, needs_receptacle: true, ..e.clone() };
        let l = lacking(&sp, &f, &rule, &disc);
        assert_eq!(l.iter().map(|l| l.check).collect::<Vec<_>>(), vec!["receptacle"]);
        // The conflicts say both documents; the objects are built beside it, once.
        let c = lack_conflicts(&e, &lacks, &rule);
        assert_eq!(c.len(), 1);
        assert!(c[0].message.contains("no disconnect within sight") && c[0].message.contains("440.14"), "{c:?}");
        let mut sp2 = sp.clone();
        let built = build_lacking(&mut sp2, &e, &lacks, Some("QP-10,12"), &rule);
        assert_eq!(built.len(), 1, "{built:?}");
        let t = sp2.tags.last().expect("built");
        assert_eq!(t.fields["circuit"].value, "QP-10,12");
        assert_eq!(t.fields["design_value"].value, "not on the drawings");
        assert!(t.at_ft[0] > 53.0 && t.at_ft[0] < 55.0, "beside HP-9's east side: {:?}", t.at_ft);
        assert!(build_lacking(&mut sp2, &e, &lacks, None, &rule).is_empty(), "not built twice");
        assert!(lacking(&sp2, &e, &rule, &disc).is_empty(), "the built one meets it");
    }

    /// Where its submittal differs, what is built stands beside what the drawings say.
    #[test]
    fn a_connection_its_submittal_differs_from_is_corrected_not_just_reported() {
        let rule = super::super::coderules::rules().into_iter().find(|r| r.sets == "equipment_electrical").unwrap_or_default();
        let mut e = EquipmentElectrical {
            tag: "RTU-1".into(),
            drawing_ocpd: "90A/3P".into(),
            mca: Some(74.6),
            max_ocpd_a: Some(80.0),
            phase: Some(3),
            source: "submittal p.2".into(),
            ..Default::default()
        };
        correct(&mut e, Some(("8", "cu")), &rule);
        assert_eq!(e.corrected_ocpd, "80A/3P");
        assert_eq!(e.corrected_conductors, "3 #4 cu", "{e:?}");
        assert!(e.corrected_conduit.ends_with("EMT"), "{e:?}");
        assert!(e.correction_reason.contains("drawings 90A/3P") && e.correction_rule.contains("NEC"), "{e:?}");
        assert_eq!(e.drawing_ocpd, "90A/3P");
        // Fuses it names; nothing to correct, nothing said.
        let mut f = EquipmentElectrical {
            tag: "ELEV-1".into(),
            drawing_ocpd: "70A/3P".into(),
            max_ocpd_a: Some(60.0),
            fuse_class: "RK1".into(),
            ..Default::default()
        };
        correct(&mut f, None, &rule);
        assert_eq!(f.corrected_ocpd, "60A/3P Class RK1 fuses");
        let mut g =
            EquipmentElectrical { tag: "P-1".into(), drawing_ocpd: "20A/2P".into(), max_ocpd_a: Some(20.0), mca: Some(19.9), ..Default::default() };
        correct(&mut g, Some(("12", "cu")), &rule);
        assert!(g.correction_reason.is_empty() && g.corrected_ocpd.is_empty(), "{g:?}");
    }

    /// A submittal's other circuits become objects, once; quantity 0 or no such room: none.
    #[test]
    fn the_circuits_a_submittal_adds_are_built_as_objects() {
        let v = |x: f64, y: f64| cadaddy_geom::Vec2::new(x, y);
        let mut sp = cadaddy_walk3d::BuildingSpec::from_json(
            r#"{"version": 1, "units_per_foot": 12, "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let e = from_answer(&serde_json::json!({
            "source": "shop drawings sheet 5",
            "extra_circuits": [
                {"name": "CAB LIGHTS AND FAN", "volts": 120, "amps": 15, "qty": 1},
                {"name": "HOISTWAY LIGHTING", "volts": 120, "amps": 15, "qty": 0}
            ]
        }));
        // Merged into its record, they come along.
        let mut merged = EquipmentElectrical { tag: "ELEV-1".into(), description: "ELEVATOR".into(), ..Default::default() };
        merge(&mut merged, &e);
        assert_eq!(merged.extra_circuits.len(), 2);
        let e = merged;
        let rooms = vec![("ELEV.".to_string(), "1".to_string(), vec![v(0.0, 0.0), v(120.0, 0.0), v(120.0, 60.0), v(0.0, 60.0)])];
        let (built, none) = extra_circuit_tags(&mut sp, &e, &rooms, "NEC 110.3(B)");
        assert_eq!(built.len(), 1, "{built:?}");
        assert!(none.is_empty());
        let t = &sp.tags[0];
        assert_eq!(t.kind, "equipment_connection");
        assert_eq!(t.fields["device_type"].value, "120V 15A disconnect, ELEV-1: CAB LIGHTS AND FAN");
        assert_eq!(t.fields["design_value"].value, "not on the drawings");
        assert!(t.fields["correction_reason"].value.contains("sheet 5"));
        assert!(extra_circuit_tags(&mut sp, &e, &rooms, "x").0.is_empty(), "not built twice");
        // A spot another object stands on isn't taken: three circuits, three objects.
        let mut sp2 = sp.clone();
        sp2.tags.clear();
        let first =
            crate::cmd::coderules::spot_in_room(&["ELEV".to_string()], &rooms, &["Level 1".to_string()], &mut std::collections::BTreeMap::new())
                .expect("a spot");
        sp2.tags.push(cadaddy_walk3d::tags::ElementTag {
            id: format!("device:Level 1:{:.1},{:.1}", first.2.x / 12.0, first.2.y / 12.0),
            ..Default::default()
        });
        let three = from_answer(
            &serde_json::json!({"source": "s", "extra_circuits": [{"name": "A", "qty": 1}, {"name": "B", "qty": 1}, {"name": "C", "qty": 1}]}),
        );
        let three = EquipmentElectrical { tag: "ELEV-1".into(), description: "ELEVATOR".into(), ..three };
        assert_eq!(extra_circuit_tags(&mut sp2, &three, &rooms, "x").0.len(), 3);
        let ids: std::collections::BTreeSet<&String> = sp2.tags.iter().map(|t| &t.id).collect();
        assert_eq!(ids.len(), 4, "{ids:?}");
        let (b, n) = extra_circuit_tags(&mut sp.clone(), &EquipmentElectrical { tag: "DOAS-1".into(), ..e.clone() }, &[], "x");
        assert!(b.is_empty() && !n.is_empty());
    }

    /// A PTAC submittal of several models: the tag's model (its item line), its column (230/208 V,
    /// 1 phase, a 3.5 kW heater) and the cord for that heater at 208 V (20 A branch, MCA 19.9).
    #[test]
    fn a_multi_model_submittals_data_is_the_tags_model() {
        let text = "ITEM   PLAN DESIGNATION   QUANTITY   COOLING Btu   VOLTAGE   FRIEDRICH MODEL\n\
            PTAC-1   127   9000   208/230V   PDH09K3SGR3\n\
            Model   PDH07K3SGR3   PDH07R3SGR3   PDH09K3SGR3   PDH09R3SGR3   PDH15K5SGR3\n\
            Heater Size (kW)   3.5   3.5   3.5   3.5   5\n\
            Voltage (1 Phase, 60 Hz)   230/208   265   230/208   265   230/208\n\
            POWER CORD #   VOLTAGE   BRANCH CKT AMPS   MCA   WATTS\n\
            PXPCZ23015A   230/208   15   13.9 2,500   Receptacles\n\
            PXPCZ23020A   230/208   20   19.9 3,600\n\
            PXPCZ23030A   230/208   30   27.5 5,000\n\
            PXPCZ26520A   265V   20   16.8 3,500\n";
        assert_eq!(model_of(text, "PTAC-1").as_deref(), Some("PDH09K3SGR3"));
        let e = submittal_for_model(text, "PTAC-1").expect("data");
        assert_eq!((e.volts, e.phase, e.kw, e.max_ocpd_a, e.mca), (Some(208.0), Some(1), Some(3.5), Some(20.0), Some(19.9)));
        assert!(submittal_for_model(text, "PTAC-2").is_none());
    }
    use cadaddy_geom::Vec2;

    fn t(x: f64, y: f64, v: &str) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    #[test]
    fn a_one_lines_load_is_the_label_under_its_feeder_tag_with_the_protection_above() {
        let v = vec![
            t(1.0, 5.0, "70A/3P"),
            t(1.0, 4.3, "74"),
            t(1.0, 3.4, "ELEV-1"),
            t(2.0, 5.0, "200A/3P"),
            t(2.0, 4.3, "204"),
            t(2.0, 3.6, "A"),
            t(3.0, 4.3, "94"),
            t(3.0, 3.5, "DOAS-1"),
        ];
        let l = oneline_loads(&v, &["A".into()], &["74".into(), "204".into(), "94".into()]);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(l[0], OnelineLoad { tag: "ELEV-1".into(), feeder: "74".into(), ocpd: "70A/3P".into() });
        assert_eq!(l[1].tag, "DOAS-1");
    }

    #[test]
    fn an_elevators_data_reads_and_its_checks_fail_where_the_drawings_differ() {
        let text = "ELECTRICAL MAXIMUM ALLOWABLE VOLTAGE VARIATION IS +/- 10% NAMEPLATE AMPS: 42 MAX ACCEL AMPS: 46 \
                    PROTECTION DEVICE REQUIRED PRIOR TO INSTALLATION MAX MOTOR BRANCH SHORT-CIRCUIT PROTECTION IS UL CLASS RK1 FUSE (AMP): 60 \
                    TRANS RATED VOLTAGE: 208, 3 PH, 60 HZ SUPPLIED VOLTAGE: 208 SHORT CIRCUIT CURRENT RATING (SCCR): 10000RMS SYM. AMPS";
        let mut e = text_electrical(text);
        assert_eq!(
            (e.nameplate_a, e.accel_a, e.max_ocpd_a, e.sccr_a, e.volts, e.phase),
            (Some(42.0), Some(46.0), Some(60.0), Some(10000.0), Some(208.0), Some(3))
        );
        assert_eq!(e.fuse_class, "RK1");
        e.tag = "ELEV-1".into();
        e.source_panel = "MDP".into();
        e.drawing_ocpd = "70A/3P".into();
        e.source = "elevator submittal p.9".into();
        let rule = super::super::coderules::rules().into_iter().find(|r| r.id == "E-EQUIP-ELEC").unwrap_or_default();
        let c = check(&e, Some(("4", "cu")), Some(57_287.0), &rule);
        let checks: Vec<&str> = c.iter().map(|x| x.check).collect();
        assert_eq!(checks, ["protection", "sccr"], "{c:#?}");
        assert!(c[0].message.contains("70A/3P") && c[0].message.contains("60 A Class RK1"));
        assert!(c[1].message.contains("10000") && c[1].message.contains("57287"));
        // #8 copper (50 A) is short of 125% of 42 A (52.5 A).
        assert!(check(&e, Some(("8", "cu")), None, &rule).iter().any(|x| x.check == "conductors"));
    }

    #[test]
    fn a_units_data_reads_with_its_numbers_before_their_keys() {
        let text = "Unit Voltage: 3 phs 208 V 60Hz Unit Main Input 74.6 Amps MCA • 80 Amps MOP • 208 V • 4 AWG Wire Min. SCCR: 10 kAmp \
                    SUPPLY MOTOR Model DTP0024 • 2.000 HP, 3 Phs, 208 V, 60 Hz, 6.1 FLA";
        let e = text_electrical(text);
        assert_eq!((e.mca, e.max_ocpd_a, e.sccr_a, e.volts, e.phase, e.fla), (Some(74.6), Some(80.0), Some(10_000.0), Some(208.0), Some(3), None));
    }

    #[test]
    fn a_submittal_sets_its_schedule_rows_load_and_the_print_stays_beside_it() {
        let elec = |volts, phase, nameplate_a, mca| EquipmentElectrical { volts, phase, nameplate_a, mca, ..Default::default() };
        // 42 A at 208 V, three phase.
        let va = submitted_va(&elec(Some(208.0), Some(3), Some(42.0), None)).unwrap();
        assert!((va - 15131.0).abs() < 1.0, "{va}");
        // Minimum circuit amps at 208 V, single phase, with no nameplate.
        assert!((submitted_va(&elec(Some(208.0), Some(1), None, Some(19.9))).unwrap() - 4139.2).abs() < 0.1);
        assert_eq!(submitted_va(&elec(None, Some(3), Some(42.0), None)), None, "no volts");
        assert_eq!(submitted_va(&elec(Some(208.0), Some(3), None, None)), None, "no amps");

        let row = |load_va| cadaddy_walk3d::PanelCircuit { number: "1".into(), load_va, ..Default::default() };
        let mut c = row(Some(16212.0));
        assert_eq!(take_submitted_load(&mut c, va), Some(16212.0));
        assert_eq!((c.load_va, c.design_load_va), (Some(va), Some(16212.0)));
        // Within 1%: the print stands.
        let mut c = row(Some(4140.0));
        assert_eq!(take_submitted_load(&mut c, 4139.2), None);
        assert_eq!((c.load_va, c.design_load_va), (Some(4140.0), None));
        // The schedule silent: the submittal's, nothing to keep.
        let mut c = row(None);
        assert_eq!(take_submitted_load(&mut c, va), None);
        assert_eq!((c.load_va, c.design_load_va), (Some(va), None));
    }

    #[test]
    fn a_loads_schedule_row_is_the_one_that_is_it() {
        use cadaddy_walk3d::{BuildingSpec, PanelCircuit, PanelSchedule};
        let row = |n: &str, d: &str| PanelCircuit { number: n.into(), description: d.into(), ..PanelCircuit::default() };
        let mut sp = BuildingSpec::default();
        sp.mep.panel_schedules = vec![
            PanelSchedule {
                panel: "A".into(),
                circuits: vec![row("11", "LTG - ELEVATOR PIT"), row("15", "ELEVATOR SUMP PUMP")],
                ..PanelSchedule::default()
            },
            PanelSchedule { panel: "MDP".into(), circuits: vec![row("8", "ELEVATOR"), row("9", "DOAS-1")], ..PanelSchedule::default() },
        ];
        assert_eq!(schedule_row(&sp, "ELEV-1"), Some(("ELEVATOR".into(), "MDP".into())));
        assert_eq!(schedule_row(&sp, "DOAS-1"), Some(("DOAS-1".into(), "MDP".into())));
    }

    #[test]
    fn revisions_and_stamps_order_submittals() {
        assert!(revision("x/Rev 1-1/a.pdf") > revision("x/Rev 1-0/a.pdf"));
        assert_eq!(revision("a.pdf"), (0, 0));
        assert_eq!(review_status("NO EXCEPTIONS TAKEN X EXCEPTIONS AS NOTED REVISE & RESUBMIT RETURN FOR CORRECTIONS"), "exceptions as noted");
        assert_eq!(review_status("NO EXCEPTIONS TAKEN EXCEPTIONS AS NOTED REVISE & RESUBMIT"), "");
        assert_eq!(review_status("Stamp: NO EXCEPTIONS TAKEN"), "no exceptions taken");
        assert_eq!(stem("ELEV-1").as_deref(), Some("ELEV"));
    }
}
