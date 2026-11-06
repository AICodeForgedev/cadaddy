//! Equipment connections against their submittals (Q6176): each load the one-line feeds (a
//! feeder-type tag with its protection above it and a label below that isn't a panel: `ELEV-1`,
//! `DOAS-1`), its approved submittal's electrical data (read from its text; a scanned submittal is
//! read by an agent, asked through the job), and the checks (`coderules.json` E-EQUIP-ELEC): its
//! protection no larger than the maker's maximum, its conductors' ampacity at least 125% of its
//! nameplate or full-load amps, its controller's short-circuit rating at least the available fault
//! current at its source. A difference is an RFI with both values and the fix.

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
            // Right after (a colon allowed): `SCCR: 10 kAmp`, `NAMEPLATE AMPS: 42`.
            let after = i + kw.len();
            if let Some(v) = words.get(after).and_then(|w| num(w)) {
                return Some(if words.get(after + 1).is_some_and(|w| kilo(w)) { v * 1000.0 } else { v });
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
        max_ocpd_a: number_at(&up, &["MAXIMUM OVERCURRENT PROTECTION", "MAX FUSE SIZE", "MAX FUSE", "MOCP", "MOP", "MAX OCPD", "FUSE (AMP)"]),
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

/// A failed check: what, both values, the fix.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Conflict {
    pub check: &'static str,
    pub message: String,
    pub fix: String,
}

/// The checks of one connection: `feeder` the drawings' conductors (size, material), `afc` the
/// available fault current at its source panel (its schedule's AFC), the rule's data.
pub(crate) fn check(e: &EquipmentElectrical, feeder: Option<(&str, &str)>, afc: Option<f64>, rule: &super::coderules::CodeRule) -> Vec<Conflict> {
    let mut out = Vec::new();
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
        });
    }
    let base = e.mca.or_else(|| e.fla.or(e.nameplate_a).map(|a| a * rule.conductor_factor.unwrap_or(1.25)));
    if let (Some(need), Some((size, material))) = (base, feeder) {
        let have = rule.ampacity_75c.get(material).and_then(|t| t.get(size.trim_start_matches('#').trim_end_matches(" kcmil")));
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
        });
    }
    out
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
        for _ in 0..x.qty {
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

/// What is built instead of the drawings' where the submittal differs, from the same data the
/// checks use: its protection (the most the submittal allows), its conductors (the smallest with
/// the ampacity it needs) and their conduit by fill. Nothing to correct: empty. An equipment's
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
    let need = e.mca.or_else(|| e.fla.or(e.nameplate_a).map(|a| a * rule.conductor_factor.unwrap_or(1.25)));
    if let (Some(need), Some((size, material))) = (need, feeder)
        && let Some(table) = rule.ampacity_75c.get(material)
    {
        let have = table.get(size.trim_start_matches('#').trim_end_matches(" kcmil")).copied();
        if have.is_some_and(|h| h + 1e-9 < need)
            && let Some((bigger, a)) = table.iter().filter(|(_, a)| **a + 1e-9 >= need).min_by(|x, y| x.1.total_cmp(y.1))
        {
            let phases = e.phase.unwrap_or(3).clamp(1, 3);
            let hots = if phases == 1 { 2 } else { phases };
            e.corrected_conductors = format!("{hots} #{bigger} {material}");
            why.push(format!(
                "feeder's #{} {material} is under the {need:.0} A it needs; #{bigger} carries {a:.0} A (75 C)",
                size.trim_start_matches('#')
            ));
            // Its conduit for those conductors and a ground (of the same size at most).
            let rl = super::conduitroute::rules();
            let parts = [(hots, bigger.clone(), true), (1, bigger.clone(), false)];
            if let Some((trade, _, _)) = super::conduitroute::fill_size(&parts, "EMT", 0.0, rl) {
                e.corrected_conduit = format!("{}\" EMT", super::mepatoms::trade_size(trade));
            }
        }
    }
    if !why.is_empty() {
        e.correction_reason = format!("{}: {} ({})", e.tag, why.join("; "), e.source);
        e.correction_rule = rule.cite.clone();
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
    let st = stem(tag)?;
    let mut best: Option<(u8, String, String)> = None;
    for p in &sp.mep.panel_schedules {
        for c in &p.circuits {
            let up = c.description.to_ascii_uppercase();
            let words: Vec<&str> = up.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').filter(|w| !w.is_empty()).collect();
            // Its tag; else a description that is only the load (`ELEVATOR`); else one that starts
            // with it (`ELEVATOR SUMP PUMP` is a pump, ranked below).
            let rank = if words.iter().any(|w| w.eq_ignore_ascii_case(tag)) {
                4
            } else if words.len() == 1 && words.first().is_some_and(|w| w.starts_with(&st)) {
                3
            } else if words.first().is_some_and(|w| w.starts_with(&st)) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
