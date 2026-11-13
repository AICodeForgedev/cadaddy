//! A panel schedule's header onto its panel elements (Q6591).
//!
//! The panel schedule reader keeps each header fact as printed (`bus amps` → `225A`,
//! `volts/phase` → `208Y/120V, 3PH, 4W`, `mounting`, `nema rating`, `aic rating`). The fill
//! (`bim.props.fill`) puts them on the panel the schedule is for as its properties (`bus_amps`,
//! `main`, `voltage`, `phase`, `enclosure`, `aic`, `mounting`), with the schedule itself as
//! `schedule`, each with the schedule as its source. A typical schedule (`TYPICAL OF n`) is the
//! schedule of every panel of its type: each of them takes its header.
//!
//! Which labels give which property, and how each value reads, is rules data
//! (`panelheader.json`); nothing here knows one job.

use std::collections::BTreeMap;

use cadaddy_doc::props::{Prop, Source, SourceKind};
use cadaddy_walk3d::PanelSchedule;
use serde::Deserialize;
use serde_json::Value;

const RULES: &str = include_str!("panelheader.json");
/// Longest header value kept (a hostile spec's).
const MAX_VALUE: usize = 200;

/// The rules: each panel property and the header labels that give it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub fields: Vec<Field>,
    pub typical_note: String,
}

/// One panel property a header gives.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Field {
    pub key: String,
    /// The labels that give it (whole words of the printed label), best first.
    pub labels: Vec<String>,
    /// Labels that hold one of `labels`' words but are something else (`GROUND BUS`).
    pub not_labels: Vec<String>,
    /// `text`, `amps`, `volts`, `phase` or `word`.
    pub read: String,
    pub unit: String,
    /// Put before a bare printed value (`1` → `NEMA 1`).
    pub prefix: String,
    /// For `word`: each value and the printed words that mean it.
    pub values: BTreeMap<String, Vec<String>>,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// Upper-case words of a text, for matching whole words and phrases (`/` and `.` kept).
fn norm(t: &str) -> String {
    let u: String = t.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '/' || c == '.' { c } else { ' ' }).collect();
    format!(" {} ", u.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn has_phrase(text: &str, phrase: &str) -> bool {
    let p = norm(phrase);
    !p.trim().is_empty() && norm(text).contains(&p)
}

/// The header fact a field reads: the first of its labels the header has (a label that is one
/// of its `not_labels` is not it). (label, value).
fn fact_for<'a>(f: &Field, header: &'a BTreeMap<String, String>) -> Option<(&'a str, &'a str)> {
    let usable = |k: &str, v: &str| !v.trim().is_empty() && !f.not_labels.iter().any(|n| norm(k) == norm(n));
    f.labels.iter().find_map(|l| header.iter().find(|(k, v)| usable(k, v) && has_phrase(k, l)).map(|(k, v)| (k.as_str(), v.as_str())))
}

/// The first number of a text (`225A` → 225; `1,200 A` → 1200).
fn first_number(t: &str) -> Option<f64> {
    let s: String = t.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    s.replace(',', "").trim_end_matches('.').parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// A volts/phase fact's parts: (volts, phase). `208Y/120V, 3PH, 4W` → (`208Y/120V`, `3PH, 4W`);
/// `277/480 VOLT 3 PHASE 4 WIRE` → (`277/480V`, `3PH, 4W`); `120/240V-1Ø-3W` → (`120/240V`,
/// `1PH, 3W`). A bare number under a label that is only volts or only phases is that.
pub(crate) fn volts_phase(v: &str, label: &str) -> (Option<String>, Option<String>) {
    let u: String = v.to_uppercase().replace(['\u{d8}', '\u{3a6}', '\u{2205}', '\u{3c6}'], "PH");
    let toks: Vec<&str> = u.split([',', ' ', '-', ';']).filter(|t| !t.is_empty()).take(40).collect();
    let (mut volts, mut phase, mut wire): (Vec<String>, Option<u32>, Option<u32>) = (Vec::new(), None, None);
    let mut i = 0;
    while let Some(t) = toks.get(i) {
        i += 1;
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = t.get(digits.len()..).unwrap_or_default();
        // `3PH`, `3 PHASE`, `4W`, `4 WIRE`: the unit on the number or the word after it.
        let next = toks.get(i).copied().unwrap_or_default();
        let (unit, joined) = if !digits.is_empty() && rest.is_empty() && next.chars().all(|c| c.is_ascii_alphabetic()) && !next.is_empty() {
            (next, true)
        } else {
            (rest, false)
        };
        let n = digits.parse::<u32>().ok();
        if let Some(n) = n.filter(|n| (1..=3).contains(n))
            && unit.starts_with("PH")
        {
            phase = Some(n);
            i += usize::from(joined);
        } else if let Some(n) = n.filter(|n| (2..=5).contains(n))
            && (unit == "W" || unit.starts_with("WIRE"))
        {
            wire = Some(n);
            i += usize::from(joined);
        } else if !digits.is_empty() && (rest.contains('/') || rest.starts_with('V') || rest.starts_with("Y/")) {
            volts.push(if rest.ends_with('V') || rest.contains("V/") || rest.starts_with('V') { t.to_string() } else { format!("{t}V") });
        } else if !digits.is_empty() && rest.is_empty() && unit.starts_with('V') {
            // `480 VOLTS`.
            volts.push(format!("{digits}V"));
            i += usize::from(joined);
        }
    }
    let lab = norm(label);
    let only_volts = lab.contains("VOLT") && !lab.contains("PH");
    let only_phase = lab.contains("PH") && !lab.contains("VOLT") && !lab.contains(" V/");
    let bare = first_number(&u).filter(|_| toks.len() == 1 && toks.first().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())));
    if volts.is_empty()
        && only_volts
        && let Some(n) = bare.filter(|n| (90.0..=1000.0).contains(n))
    {
        volts.push(format!("{n:.0}V"));
    }
    if phase.is_none() && only_phase {
        phase = bare.filter(|n| (1.0..=3.0).contains(n)).map(|n| n as u32);
    }
    let volts = (!volts.is_empty()).then(|| volts.join(" "));
    let phase = phase.map(|p| match wire {
        Some(w) => format!("{p}PH, {w}W"),
        None => format!("{p}PH"),
    });
    (volts, phase)
}

/// One printed word list's value: the `values` key whose words the printed text has.
fn word_of(f: &Field, printed: &str) -> Option<String> {
    f.values.iter().find(|(_, words)| words.iter().any(|w| has_phrase(printed, w))).map(|(k, _)| k.clone())
}

/// The schedule's name as the panels' `schedule` (and their header values' source).
pub(crate) fn schedule_name(s: &PanelSchedule) -> String {
    let sheet = if s.source.trim().is_empty() { String::new() } else { format!(" ({})", s.source.trim()) };
    match s.typical_of {
        Some(n) => format!("panel {} schedule{sheet}, typical of {n}", s.panel.trim()),
        None => format!("panel {} schedule{sheet}", s.panel.trim()),
    }
}

/// What a panel takes from its schedule's header: (key, property), each with the schedule as
/// its source and the printed label (and value, where it reads otherwise) as its note.
/// `typical`: the panel is one of a typical schedule's.
pub(crate) fn props_of(s: &PanelSchedule, typical: bool) -> Vec<(String, Prop)> {
    let r = rules();
    let name = schedule_name(s);
    let src = Source::from_doc(SourceKind::Schedule, &name);
    let typical_note = if typical && !r.typical_note.is_empty() { format!("; {}", r.typical_note) } else { String::new() };
    let mut out: Vec<(String, Prop)> = Vec::new();
    for f in &r.fields {
        let Some((label, printed)) = fact_for(f, &s.header) else { continue };
        let printed: String = printed.trim().chars().take(MAX_VALUE).collect();
        let value: Option<Value> = match f.read.as_str() {
            "amps" => {
                first_number(&printed).filter(|a| *a <= 10_000.0).map(|a| if a.fract() == 0.0 { Value::from(a as u64) } else { Value::from(a) })
            }
            "volts" => volts_phase(&printed, label).0.map(Value::from),
            "phase" => volts_phase(&printed, label).1.map(Value::from),
            "word" => Some(Value::from(word_of(f, &printed).unwrap_or_else(|| printed.to_ascii_lowercase()))),
            _ => {
                let bare = printed.chars().count() <= 4 && printed.chars().any(|c| c.is_ascii_digit());
                Some(Value::from(if bare && !f.prefix.is_empty() { format!("{}{printed}", f.prefix) } else { printed.clone() }))
            }
        };
        let Some(value) = value else { continue };
        let same = value.as_str().is_some_and(|v| v == printed);
        let note = if same {
            format!("header {}{typical_note}", label.to_ascii_uppercase())
        } else {
            format!("header {}: {printed}{typical_note}", label.to_ascii_uppercase())
        };
        let mut p = Prop::new(value, src.clone());
        p.note = Some(note);
        if !f.unit.is_empty() && f.read == "amps" {
            p = p.with_unit(&f.unit);
        }
        out.push((f.key.clone(), p));
    }
    let mut p = Prop::new(name, src);
    if typical {
        p.note = Some(r.typical_note.clone()).filter(|n| !n.is_empty());
    }
    out.push(("schedule".into(), p));
    out
}

/// The kinds of gear a room's own (typical) panel can be.
const ROOM_PANEL_KINDS: [&str; 5] = ["panel", "panelboard", "load_center", "loadcenter", "panel_board"];

/// Whether a gear kind (`panelboard`) can be a room's own panel.
pub(crate) fn is_room_panel_kind(kind: &str) -> bool {
    ROOM_PANEL_KINDS.iter().any(|x| kind.trim().eq_ignore_ascii_case(x))
}

/// The schedule of a built panel element: its own (by tag), else the typical schedule its
/// `PANEL` names when it is a room's panel of that type (a panelboard whose `PANEL` names a
/// typical schedule, or whose tag is `<type>-…`). `part` is its build part (`equipment:panel`),
/// `panel_prop` its record's `PANEL`. (schedule, typical).
pub(crate) fn schedule_of<'a, S: std::borrow::Borrow<PanelSchedule>>(
    tag: &str,
    part: &str,
    panel_prop: Option<&str>,
    schedules: &'a std::collections::HashMap<String, S>,
) -> Option<(&'a PanelSchedule, bool)> {
    let t = tag.trim().to_ascii_uppercase();
    if let Some(s) = schedules.get(&t).filter(|_| !t.is_empty()) {
        return Some((s.borrow(), false));
    }
    let room_panel = part.split_once(':').is_some_and(|(_, k)| is_room_panel_kind(k));
    let letter = panel_prop.map(|p| p.trim().to_ascii_uppercase()).filter(|p| !p.is_empty() && *p != t)?;
    let s = schedules.get(&letter)?.borrow();
    (room_panel && (s.typical_of.is_some() || t.starts_with(&format!("{letter}-")))).then_some((s, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sched(panel: &str, typical: Option<u32>, header: &[(&str, &str)]) -> PanelSchedule {
        PanelSchedule {
            panel: panel.into(),
            typical_of: typical,
            source: "E-9".into(),
            header: header.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..PanelSchedule::default()
        }
    }

    fn get<'a>(ps: &'a [(String, Prop)], k: &str) -> Option<&'a Prop> {
        ps.iter().find(|(key, _)| key == k).map(|(_, p)| p)
    }

    #[test]
    fn volts_and_phase_read_in_the_usual_ways() {
        let vp = |v: &str| volts_phase(v, "volts/phase");
        assert_eq!(vp("208Y/120V, 3PH, 4W"), (Some("208Y/120V".into()), Some("3PH, 4W".into())));
        assert_eq!(vp("277/480 VOLT 3 PHASE 4 WIRE"), (Some("277/480V".into()), Some("3PH, 4W".into())));
        assert_eq!(vp("120/240V-1\u{d8}-3W"), (Some("120/240V".into()), Some("1PH, 3W".into())));
        assert_eq!(volts_phase("480", "voltage"), (Some("480V".into()), None));
        assert_eq!(volts_phase("3", "phase"), (None, Some("3PH".into())));
        assert_eq!(volts_phase("", "volts"), (None, None));
        assert_eq!(volts_phase("SEE NOTE", "volts/phase"), (None, None));
    }

    /// Q6591: one schedule's header labels, two layouts: each fact goes on its property, the
    /// value as the form wants it, the printed label (and value where it differs) noted.
    #[test]
    fn a_header_gives_each_panel_property() {
        let s = sched(
            "LP-2",
            None,
            &[
                ("bus amps", "225A"),
                ("main size / type", "MLO"),
                ("mounting", "SURFACE"),
                ("volts/phase", "208Y/120V, 3PH, 4W"),
                ("nema rating", "NEMA 1"),
                ("aic rating", "22,000A"),
                ("ground bus", "YES"),
            ],
        );
        let ps = props_of(&s, false);
        let v = |k: &str| get(&ps, k).map(Prop::text).unwrap_or_default();
        assert_eq!(v("bus_amps"), "225");
        assert_eq!(get(&ps, "bus_amps").and_then(|p| p.unit.clone()).as_deref(), Some("A"));
        assert_eq!((v("main"), v("mounting"), v("voltage"), v("phase")), ("MLO".into(), "surface".into(), "208Y/120V".into(), "3PH, 4W".into()));
        assert_eq!((v("enclosure"), v("aic")), ("NEMA 1".into(), "22,000A".into()));
        assert_eq!(v("schedule"), "panel LP-2 schedule (E-9)");
        let src = get(&ps, "voltage").map(|p| p.source.clone()).unwrap_or_default();
        assert_eq!((src.kind, src.doc.as_deref()), (SourceKind::Schedule, Some("panel LP-2 schedule (E-9)")));
        assert_eq!(get(&ps, "voltage").and_then(|p| p.note.clone()).as_deref(), Some("header VOLTS/PHASE: 208Y/120V, 3PH, 4W"));

        // The other usual layout: separate volts and phase, a bus rating, a bare NEMA number,
        // flush written as recessed, a main breaker.
        let s = sched(
            "2HB",
            None,
            &[("bus rating", "400 A"), ("main", "400A MCB"), ("mtg", "RECESSED"), ("voltage", "480"), ("phase", "3"), ("nema", "3R")],
        );
        let ps = props_of(&s, false);
        let v = |k: &str| get(&ps, k).map(Prop::text).unwrap_or_default();
        assert_eq!((v("bus_amps"), v("main"), v("mounting")), ("400".into(), "400A MCB".into(), "flush".into()));
        assert_eq!((v("voltage"), v("phase"), v("enclosure")), ("480V".into(), "3PH".into(), "NEMA 3R".into()));
        assert_eq!(get(&ps, "mounting").and_then(|p| p.note.clone()).as_deref(), Some("header MTG: RECESSED"));
    }

    /// A ground bus is not the bus rating; a header with nothing the rules know gives only the
    /// schedule.
    #[test]
    fn other_buses_and_unknown_labels_give_nothing() {
        let s = sched("P", None, &[("ground bus", "YES"), ("isol. ground bus", "NO"), ("feed thru lugs", "NO")]);
        let ps = props_of(&s, false);
        assert!(get(&ps, "bus_amps").is_none(), "{ps:?}");
        assert_eq!(ps.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["schedule"]);
    }

    /// A typical schedule is every panel of its type's: a room panel whose record names it, or
    /// whose tag is `<type>-…`, takes it (marked typical); other gear naming a panel is fed from it.
    #[test]
    fn a_typical_schedule_is_each_room_panels() {
        let mut m: HashMap<String, PanelSchedule> = HashMap::new();
        m.insert("U".into(), sched("U", Some(40), &[("bus amps", "125A")]));
        m.insert("H1".into(), sched("H1", None, &[("bus amps", "100A")]));
        let own = schedule_of("H1", "equipment:panelboard", None, &m).map(|(s, t)| (s.panel.clone(), t));
        assert_eq!(own, Some(("H1".into(), false)));
        let room = schedule_of("U-207", "equipment:panelboard", Some("U"), &m).map(|(s, t)| (s.panel.clone(), t));
        assert_eq!(room, Some(("U".into(), true)));
        // A damper's power box fed from panel U is not one of U's panels.
        assert!(schedule_of("PB-3", "equipment:power_box", Some("U"), &m).is_none());
        // A panel naming a non-typical schedule only by its tag's type.
        assert!(schedule_of("K-2", "equipment:panelboard", Some("H1"), &m).is_none());
        let ps = props_of(m.get("U").unwrap(), true);
        assert_eq!(get(&ps, "schedule").map(Prop::text).as_deref(), Some("panel U schedule (E-9), typical of 40"));
        assert!(get(&ps, "bus_amps").and_then(|p| p.note.clone()).is_some_and(|n| n.contains("every panel of its type")), "{ps:?}");
    }
}
