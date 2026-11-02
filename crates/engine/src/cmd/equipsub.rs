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
}

#[cfg(test)]
mod tests {
    use super::*;
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
