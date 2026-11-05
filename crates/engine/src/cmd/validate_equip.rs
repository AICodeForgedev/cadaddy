//! E-EQUIP-ELEC (Q6177): Validate checks each equipment connection (an elevator, a rooftop unit, a
//! pump: what a panel or the one-line feeds that isn't a panel) against its approved submittal and
//! the code, recomputed here: the submittals are read again from the job's folders (the latest
//! approved revision wins; a scanned one is read by an agent, whose answer counts only with the
//! page it read), the requirements are worked out from `coderules.json` E-EQUIP-ELEC and the
//! conduit-fill tables (`conduitroute.json`), and compared with the model's circuit (its panel
//! schedule row: breaker, conductors, conduit; its disconnect; its source's available fault
//! current). The build's own checks are never read back; its `equipment_electrical` RFIs are only
//! matched against the verdicts afterwards: a failure with no RFI is `missing`, an RFI with no
//! failure `extra`.
//!
//! Checks (`measured.check`):
//! - **protection**: the model's breaker or fuses no larger than the submittal's maximum (MOP /
//!   max fuse), of the fuse class it names (a class the model doesn't record is `unsure`).
//! - **conductors**: the model's conductors' 75 C ampacity (parallel sets added) at least the
//!   submittal's minimum circuit amps, else the rule's factor times its full-load / nameplate amps.
//! - **conduit_fill**: the conductors of a set in its conduit within the NEC Chapter 9 fill, for
//!   every raceway type the tables list (some fitting and some not: `unsure`, the type not given).
//! - **disconnect**: the model's disconnect for it at least the rule's factor times its full-load
//!   amps (and the fuses it holds), fused where the submittal names a fuse class; a disconnect
//!   the submittal says is unit-mounted passes as the maker's; none in the model is `unsure`.
//! - **sccr**: the equipment's short-circuit current rating at least the available fault current
//!   at its source (the source panel's schedule).
//! - **revision**: the submittal the model's record names is the latest approved revision.
//!
//! A value nothing gives (no submittal, a scanned one with no answer, an answer citing no page) is
//! `unsure`, never a pass.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::{BuildingSpec, EquipmentElectrical, MepEquipment, PanelCircuit};
use serde_json::{Value, json};

use super::validate_rules::Effective;

pub const RULE: &str = "E-EQUIP-ELEC";
/// The verdicts the rule gives: `missing` a failure the build raised no RFI for, `extra` an RFI
/// of the build's with no failure behind it.
pub const VERDICTS: [&str; 5] = ["pass", "fail", "unsure", "missing", "extra"];
/// The checks, in the order they are reported.
pub const CHECKS: [&str; 6] = ["protection", "conductors", "conduit_fill", "disconnect", "sccr", "revision"];
/// The RFI kind the build raises for equipment connections.
const RFI_KIND: &str = "equipment_electrical";

/// Equipment connections checked, at most.
const MAX_ITEMS: usize = 500;
/// Files looked through for submittals, files read, files read per tag, pages per file.
const MAX_FILES_LOOKED: usize = 5_000;
const MAX_FILES_READ: usize = 80;
const MAX_FILES_PER_TAG: usize = 12;
const MAX_PAGES: usize = 400;
/// Folder depth searched under a submittals folder.
const MAX_DEPTH: usize = 6;
/// RFIs and panel circuits looked at, at most.
const MAX_RFIS: usize = 100_000;
const MAX_CIRCUITS: usize = 200_000;
/// Parallel sets and conductors counted, at most (hostile records ask for billions).
const MAX_SETS: u32 = 40;
const MAX_CONDUCTORS: u32 = 60;
const QUOTE_CHARS: usize = 300;

// ---------------------------------------------------------------- the submittals, re-read

/// One submittal file read again for one connection: its revision (folder), its review stamp,
/// the electrical data its text gives and the pages it came from.
#[derive(Clone, Debug, Default)]
pub(crate) struct SubFile {
    pub name: String,
    pub folder: String,
    pub revision: (u32, u32),
    /// What its stamp says (`exceptions as noted`, `revise and resubmit`…; empty: not read).
    pub status: String,
    pub data: Option<EquipmentElectrical>,
    pub pages: Vec<u32>,
    /// It has no text, or none with electrical data (a scan with only the stamps as text).
    pub scanned: bool,
    /// It could not be read (why).
    pub unread: Option<String>,
    /// What the unit-mounted disconnect words found in it.
    pub unit_disconnect: Option<String>,
}

/// The job's submittals read again, per connection tag (upper case), and the agents' answers.
#[derive(Clone, Debug, Default)]
pub(crate) struct EquipDocs {
    pub by_tag: BTreeMap<String, Vec<SubFile>>,
    /// `equip:<TAG>` job answers (an agent's reading of a scanned submittal).
    pub answers: BTreeMap<String, Value>,
    pub read: usize,
    pub unread: Vec<(String, String)>,
}

/// Does a screen run the rule (`only` empty: every enabled rule)?
pub(crate) fn wanted(rules: &[Effective], only: &[String]) -> bool {
    rules.iter().any(|r| r.enabled && r.rule.id == RULE) && (only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(RULE)))
}

/// Every PDF under a folder (or the file itself), sorted, at most [`MAX_FILES_LOOKED`].
fn pdfs_under(p: &std::path::Path, out: &mut Vec<String>, depth: usize) {
    if out.len() >= MAX_FILES_LOOKED || depth > MAX_DEPTH {
        return;
    }
    if p.is_file() {
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            out.push(p.to_string_lossy().to_string());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        pdfs_under(&e, out, depth + 1);
    }
}

fn upper_words(t: &str) -> BTreeSet<String> {
    t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '#')).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect()
}

/// The letters a connection's submittal is found by (`CHIL-2` → `CHIL`, at least three).
fn stem(tag: &str) -> Option<String> {
    let s: String = tag.trim().chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    (s.len() >= 3).then_some(s)
}

/// The words of a file's path (folders and name) name the connection's letters.
fn path_names(path: &str, st: &str) -> bool {
    let p = std::path::Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
    let folder = p.parent().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
    let parent2 =
        p.parent().and_then(|f| f.parent()).and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
    name.contains(st) || folder.contains(st) || parent2.contains(st)
}

/// Read one submittal file for `tag`: its stamp, and the data of the pages that are about it (a
/// page naming another connection of the same letters and not this one is that one's).
fn read_file(path: &str, tag: &str, unit_words: &[String]) -> SubFile {
    let p = std::path::Path::new(path);
    let name = p.file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string());
    let folder = p.parent().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let mut f = SubFile { name, folder, revision: super::equipsub::revision(path), ..SubFile::default() };
    let bytes = match crate::readfile::read(path) {
        Ok(b) => b,
        Err(e) => {
            f.unread = Some(e.to_string().chars().take(200).collect());
            return f;
        }
    };
    let text = match super::spec_import::pdf_text(&bytes, path) {
        Ok(t) => t,
        Err(e) => {
            let why = e.to_string();
            if why.contains("no text") {
                f.scanned = true;
            } else {
                f.unread = Some(why.chars().take(200).collect());
            }
            return f;
        }
    };
    f.status = super::equipsub::review_status(&text);
    let up_all = text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    f.unit_disconnect = unit_words.iter().find(|w| !w.trim().is_empty() && up_all.contains(&w.to_ascii_uppercase())).cloned();
    let me = tag.trim().to_ascii_uppercase();
    let st = stem(tag).unwrap_or_default();
    let mut data = EquipmentElectrical::default();
    for page in text.split("--- page ").skip(1).take(MAX_PAGES) {
        let n: u32 = page.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let words = upper_words(page);
        let others = words.iter().any(|w| *w != me && w.starts_with(&st) && w.len() > st.len() && w.chars().any(|c| c.is_ascii_digit()));
        if others && !words.contains(&me) {
            continue;
        }
        let got = super::equipsub::text_electrical(page);
        if super::equipsub::has_data(&got) {
            super::equipsub::merge(&mut data, &got);
            if n > 0 && f.pages.len() < 8 {
                f.pages.push(n);
            }
        }
    }
    if super::equipsub::has_data(&data) {
        f.data = Some(data);
    } else {
        f.scanned = true;
    }
    f
}

/// The job's submittals read again for every connection the spec has (each one's files by its
/// letters in their names or folders), and the agents' `equip:<TAG>` answers. None without a job.
pub(crate) fn read_docs(s: &crate::Session, sp: &BuildingSpec) -> Option<EquipDocs> {
    let tags: Vec<String> = connections(sp).into_iter().map(|c| c.tag).collect();
    if tags.is_empty() {
        return None;
    }
    let job = super::guided::load_job(s).ok().flatten()?;
    let mut docs = EquipDocs::default();
    for t in &tags {
        let key = format!("equip:{}", t.to_ascii_uppercase());
        if let Some(a) = job.rules.iter().find(|r| r.key.eq_ignore_ascii_case(&key)).and_then(|r| serde_json::from_str::<Value>(&r.value).ok())
            && a.is_object()
        {
            docs.answers.insert(t.to_ascii_uppercase(), a);
        }
    }
    let mut files: Vec<String> = Vec::new();
    for root in &job.submittals {
        pdfs_under(std::path::Path::new(root.trim()), &mut files, 0);
    }
    files.sort();
    files.dedup();
    let unit_words = super::coderules::rules().into_iter().find(|r| r.id == RULE).map(|r| r.unit_disconnect_words).unwrap_or_default();
    // A file read once serves every tag of its letters.
    let mut cache: BTreeMap<(String, String), SubFile> = BTreeMap::new();
    for t in &tags {
        let Some(st) = stem(t) else { continue };
        let mine: Vec<&String> = files.iter().filter(|f| path_names(f, &st)).take(MAX_FILES_PER_TAG).collect();
        let mut list = Vec::new();
        for f in mine {
            let key = (f.clone(), t.to_ascii_uppercase());
            if !cache.contains_key(&key) {
                if docs.read >= MAX_FILES_READ {
                    docs.unread.push((f.clone(), format!("more than {MAX_FILES_READ} submittal files")));
                    continue;
                }
                docs.read += 1;
                let got = read_file(f, t, &unit_words);
                if let Some(why) = &got.unread {
                    docs.unread.push((got.name.clone(), why.clone()));
                }
                cache.insert(key.clone(), got);
            }
            if let Some(got) = cache.get(&key) {
                list.push(got.clone());
            }
        }
        docs.by_tag.insert(t.to_ascii_uppercase(), list);
    }
    Some(docs)
}

// ---------------------------------------------------------------- what is checked

/// An equipment connection of the model: its tag, the build's record of it, its panel schedule
/// row (the model's circuit) and its equipment object.
#[derive(Clone, Debug, Default)]
pub(crate) struct Connection<'a> {
    pub tag: String,
    pub row: Option<&'a EquipmentElectrical>,
    pub circuit: Option<(&'a str, &'a PanelCircuit)>,
    pub equipment: Option<&'a MepEquipment>,
}

fn words_of(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The panel schedule row that is the connection's: one naming its tag, else one whose whole
/// description is a word starting with its letters (`CHILLER` for `CHIL-2`); the source panel the
/// record names first.
fn circuit_of<'a>(sp: &'a BuildingSpec, tag: &str, panel: &str) -> Option<(&'a str, &'a PanelCircuit)> {
    let st = stem(tag);
    let mut best: Option<(u8, &'a str, &'a PanelCircuit)> = None;
    for p in sp.mep.panel_schedules.iter().filter(|p| p.typical_of.is_none()) {
        for c in p.circuits.iter().take(MAX_CIRCUITS) {
            let w = words_of(&c.description);
            let mut rank = if w.iter().any(|x| x.eq_ignore_ascii_case(tag)) {
                4
            } else if w.len() == 1 && st.as_ref().is_some_and(|s| w.first().is_some_and(|x| x.starts_with(s.as_str()))) {
                2
            } else {
                0
            };
            if rank == 0 {
                continue;
            }
            if !panel.is_empty() && p.panel.eq_ignore_ascii_case(panel) {
                rank += 1;
            }
            if best.as_ref().is_none_or(|b| rank > b.0) {
                best = Some((rank, p.panel.as_str(), c));
            }
        }
    }
    best.map(|(_, p, c)| (p, c))
}

fn is_gear_or_disconnect(e: &MepEquipment) -> bool {
    let k = e.kind.to_ascii_lowercase();
    super::gear::is_gear(e) || k.contains("disconnect") || k.contains("panel") || k.contains("switch") || k.contains("transformer")
}

/// The connections: the build's records (`mep.equipment_electrical`), and equipment of the
/// spec a panel schedule row names by its tag.
pub(crate) fn connections(sp: &BuildingSpec) -> Vec<Connection<'_>> {
    let mut out: Vec<Connection> = Vec::new();
    for r in sp.mep.equipment_electrical.iter().take(MAX_ITEMS) {
        let tag = r.tag.trim();
        if tag.is_empty() || out.iter().any(|c| c.tag.eq_ignore_ascii_case(tag)) {
            continue;
        }
        out.push(Connection {
            tag: tag.to_string(),
            row: Some(r),
            circuit: circuit_of(sp, tag, &r.source_panel),
            equipment: sp.mep.equipment.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(tag)),
        });
    }
    for e in sp.mep.equipment.iter().filter(|e| !is_gear_or_disconnect(e)) {
        let tag = e.tag.trim();
        // A tag that can't be a word of a description by chance: letters, then a digit somewhere.
        if out.len() >= MAX_ITEMS
            || stem(tag).is_none()
            || !tag.chars().any(|c| c.is_ascii_digit())
            || out.iter().any(|c| c.tag.eq_ignore_ascii_case(tag))
        {
            continue;
        }
        let Some(circuit) = circuit_of(sp, tag, "").filter(|(_, c)| words_of(&c.description).iter().any(|w| w.eq_ignore_ascii_case(tag))) else {
            continue;
        };
        out.push(Connection { tag: tag.to_string(), row: None, circuit: Some(circuit), equipment: Some(e) });
    }
    out
}

// ---------------------------------------------------------------- the model's circuit

/// The model's conductors for a connection, from its panel schedule row's wire (`feeder 74: 1
/// set(s) of 4 4 cu, ground 8, in 1.25" conduit`, or a bare size `10`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Wiring {
    pub feeder: String,
    pub sets: u32,
    /// Conductors per set (phases and neutral), their size (`4`, `1/0`, `350 kcmil`), metal.
    pub conductors: Option<u32>,
    pub size: String,
    pub material: String,
    /// The material was said (not the schedule's copper default).
    pub material_given: bool,
    pub ground: String,
    pub conduit_in: Option<f64>,
}

/// The conductor size as the tables key it (`#4` → `4`, `350 kcmil` → `350`, `3/0`).
fn size_key(size: &str) -> String {
    let t = size.trim().trim_start_matches('#').to_ascii_lowercase();
    let t = t.trim_end_matches("kcmil").trim_end_matches("mcm").trim();
    let t = t.strip_suffix("awg").unwrap_or(t).trim();
    t.to_string()
}

fn is_size(w: &str) -> bool {
    let k = size_key(w);
    !k.is_empty() && k.chars().all(|c| c.is_ascii_digit() || c == '/') && k.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Read a panel schedule row's wire text.
pub(crate) fn wiring(wire: &str) -> Option<Wiring> {
    let t = wire.trim();
    if t.is_empty() {
        return None;
    }
    let low = t.to_ascii_lowercase();
    let mut w = Wiring { sets: 1, material: "cu".into(), ..Wiring::default() };
    if let Some(rest) = low.strip_prefix("feeder ") {
        let (tag, body) = rest.split_once(':')?;
        w.feeder = tag.trim().to_ascii_uppercase();
        let mut parts = body.split(',');
        let main = parts.next().unwrap_or_default();
        let words: Vec<&str> = main.split_whitespace().collect();
        // `S set(s) of N SIZE [kcmil] MAT`
        let of = words.iter().position(|x| *x == "of")?;
        w.sets = words.first().and_then(|x| x.parse::<u32>().ok()).filter(|n| (1..=MAX_SETS).contains(n))?;
        w.conductors = words.get(of + 1).and_then(|x| x.parse::<u32>().ok()).filter(|n| (1..=MAX_CONDUCTORS).contains(n));
        let rest: Vec<&str> = words.iter().skip(of + 2).copied().collect();
        let (mat_at, mat) = rest.iter().enumerate().find(|(_, x)| matches!(**x, "cu" | "al")).map(|(i, m)| (i, (*m).to_string())).unzip();
        let size_words: Vec<&str> = rest.iter().take(mat_at.unwrap_or(rest.len())).copied().collect();
        w.size = size_words.join(" ");
        if let Some(m) = mat {
            w.material = m;
            w.material_given = true;
        }
        for p in parts {
            let p = p.trim();
            if let Some(g) = p.strip_prefix("ground ") {
                w.ground = g.trim().to_string();
            } else if let Some(c) = p.strip_prefix("in ") {
                w.conduit_in = super::gearbuild::trade_in(c.trim().trim_end_matches("conduit").trim());
            }
        }
        return is_size(&w.size).then_some(w);
    }
    // A bare size, maybe with its metal: `10`, `#4`, `4/0 AL`.
    let words: Vec<&str> = low.split_whitespace().collect();
    let size = words.first().copied().filter(|x| is_size(x))?;
    w.size = size.to_string();
    if words.contains(&"al") {
        w.material = "al".into();
        w.material_given = true;
    } else if words.contains(&"cu") {
        w.material_given = true;
    }
    Some(w)
}

/// A number of amps in a text (`70`, `70A`, `60 AMP`, `100A/3P`).
fn amps_in(v: &str) -> Option<f64> {
    let t = v.trim().to_ascii_uppercase();
    let n: String = t.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    n.parse::<f64>().ok().filter(|a| a.is_finite() && *a > 0.0 && *a < 100_000.0)
}

/// A fault current as printed (`41,500A`, `22 kA`, `65kAIC`), amps.
fn fault_amps(v: &str) -> Option<f64> {
    let u = v.trim().to_ascii_uppercase().replace(',', "");
    let n: String = u.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let x = n.parse::<f64>().ok().filter(|a| a.is_finite() && *a > 0.0)?;
    let after: String = u.chars().skip_while(|c| !c.is_ascii_digit()).skip_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let x = if after.trim_start().starts_with('K') { x * 1000.0 } else { x };
    (x < 10_000_000.0).then_some(x)
}

/// The fuse class a text names (`RK1`, `J`).
fn fuse_class_in(t: &str) -> Option<String> {
    let u = t.to_ascii_uppercase();
    for c in ["RK1", "RK5", "CLASS J", "CLASS T", "CLASS CC", "CLASS L"] {
        if u.contains(c) {
            return Some(c.trim_start_matches("CLASS ").to_string());
        }
    }
    None
}

/// The model's disconnect for a connection: equipment or an electrical device that is a
/// disconnect and names it (its tag holds the connection's, or a property names it), or a device
/// disconnect on the same panel circuit.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModelDisconnect {
    pub tag: String,
    pub amps: Option<f64>,
    /// Fused (true), not fused (false), not said (None).
    pub fused: Option<bool>,
    pub fuse_class: Option<String>,
    pub text: String,
}

fn disconnect_from(tag: &str, kind: &str, props: &BTreeMap<String, String>) -> ModelDisconnect {
    let all = format!("{kind} {tag} {}", props.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" "));
    let up = all.to_ascii_uppercase();
    let fused = if up.contains("NON-FUSED") || up.contains("NONFUSED") || up.contains("NON FUSED") || words_of(&up).iter().any(|w| w == "NF") {
        Some(false)
    } else if up.contains("FUSED") || up.contains("FUSIBLE") || up.contains("FUSE") {
        Some(true)
    } else {
        None
    };
    let amps = props
        .iter()
        .filter(|(k, _)| {
            let k = k.to_ascii_uppercase();
            ["RATING", "AMPS", "AMP", "SIZE", "SWITCH", "FRAME"].iter().any(|w| k.contains(w))
        })
        .find_map(|(_, v)| amps_in(v));
    ModelDisconnect { tag: tag.to_string(), amps, fused, fuse_class: fuse_class_in(&up), text: quote(&all) }
}

fn model_disconnect(sp: &BuildingSpec, c: &Connection) -> Option<ModelDisconnect> {
    let me = c.tag.to_ascii_uppercase();
    let names = |tag: &str, props: &BTreeMap<String, String>| {
        words_of(tag).iter().any(|w| w.contains(&me)) || props.values().any(|v| words_of(v).contains(&me))
    };
    for e in sp.mep.equipment.iter().filter(|e| e.kind.to_ascii_lowercase().contains("disconnect")) {
        if names(&e.tag, &e.properties) {
            return Some(disconnect_from(&e.tag, &e.kind, &e.properties));
        }
    }
    let circuit = c.circuit.map(|(p, k)| (p.to_ascii_uppercase(), k.number.trim().to_string()));
    for d in sp.mep.devices.iter().filter(|d| d.kind.to_ascii_lowercase().contains("disconnect")) {
        let on_circuit = circuit.as_ref().is_some_and(|(p, n)| {
            d.panel.trim().eq_ignore_ascii_case(p) && d.circuit.split([',', ' ']).map(str::trim).any(|x| !x.is_empty() && x == n)
        });
        if names(&d.tag, &d.properties) || on_circuit {
            return Some(disconnect_from(&d.tag, &d.kind, &d.properties));
        }
    }
    None
}

/// The available fault current at a panel, from its schedule's header (`AFC`, `available fault
/// current`, `fault current`): amps, and the words it was read from.
fn source_afc(sp: &BuildingSpec, panel: &str) -> Option<(f64, String)> {
    let p = sp.mep.panel_schedules.iter().find(|p| !panel.is_empty() && p.panel.trim().eq_ignore_ascii_case(panel.trim()))?;
    p.header.iter().find_map(|(k, v)| {
        let kl = k.to_ascii_lowercase();
        (kl.contains("afc") || kl.contains("fault current")).then(|| fault_amps(v).map(|a| (a, format!("{} panel schedule {k}: {v}", p.panel))))?
    })
}

// ---------------------------------------------------------------- the requirements

/// Where the connection's submittal values come from, as Validate settles it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settled {
    pub data: Option<EquipmentElectrical>,
    /// `submittal` (its text, re-read), `agent` (a job answer citing a page), `record` (the
    /// model's record: no job on this machine).
    pub from: &'static str,
    /// The file and page(s) the values are on (`<file> p.<n>`).
    pub cite: String,
    pub status: String,
    pub revision: Option<(u32, u32)>,
    pub folder: String,
    /// Why there are no values (when `data` is None).
    pub why: String,
    pub unit_disconnect: Option<String>,
    /// Every revision looked at: (folder, revision, status, has data).
    pub history: Vec<Value>,
    /// An older revision whose text gives values the latest (a scan) supersedes.
    pub superseded: Option<String>,
}

fn approved(status: &str) -> Option<bool> {
    match status {
        "no exceptions taken" | "exceptions as noted" | "approved" => Some(true),
        "revise and resubmit" | "rejected" => Some(false),
        _ => None,
    }
}

/// A revision folder's review status from its files' stamps: approved when some file says so and
/// none sends it back; sent back when some file does and none approves; else not read (empty).
fn folder_status(files: &[&SubFile]) -> String {
    let said: Vec<&str> = files.iter().map(|f| f.status.as_str()).filter(|s| !s.is_empty()).collect();
    let yes = said.iter().find(|s| approved(s) == Some(true));
    let no = said.iter().find(|s| approved(s) == Some(false));
    match (yes, no) {
        (Some(y), None) => (*y).to_string(),
        (None, Some(n)) => (*n).to_string(),
        _ => String::new(),
    }
}

/// The page an agent's answer cites (`p.9`, `page 9`, `pg 9`, `p9`).
pub(crate) fn cited_page(source: &str) -> Option<u32> {
    let low = source.to_ascii_lowercase();
    for key in ["page ", "page", "pg. ", "pg.", "pg ", "p. ", "p.", "p "] {
        for (i, _) in low.match_indices(key) {
            // The key starts a word.
            if i > 0 && low.as_bytes().get(i - 1).is_some_and(u8::is_ascii_alphanumeric) {
                continue;
            }
            let n: String = low.get(i + key.len()..).unwrap_or("").chars().take_while(char::is_ascii_digit).collect();
            if let Some(p) = n.parse::<u32>().ok().filter(|p| *p > 0 && *p < 100_000) {
                return Some(p);
            }
        }
    }
    None
}

fn has_page_cite(source: &str) -> bool {
    cited_page(source).is_some()
}

/// Settle a connection's submittal values: the latest approved (or unstamped) revision whose text
/// gives them; else an agent's answer citing a page; else, with no job here, the model's record
/// when it cites its source; else none, with why.
pub(crate) fn settle(c: &Connection, docs: Option<&EquipDocs>) -> Settled {
    let key = c.tag.to_ascii_uppercase();
    let mut out = Settled::default();
    let files: Vec<&SubFile> = docs.and_then(|d| d.by_tag.get(&key)).map(|v| v.iter().collect()).unwrap_or_default();
    // Revision folders, the latest first.
    let mut folders: BTreeMap<(u32, u32, String), Vec<&SubFile>> = BTreeMap::new();
    for f in &files {
        folders.entry((f.revision.0, f.revision.1, f.folder.clone())).or_default().push(f);
    }
    let ordered: Vec<(&(u32, u32, String), &Vec<&SubFile>)> = folders.iter().rev().collect();
    for ((a, b, folder), fs) in &ordered {
        let status = folder_status(fs);
        out.history.push(json!({
            "folder": folder_name(folder), "revision": format!("{a}-{b}"), "status": if status.is_empty() { Value::Null } else { json!(status) },
            "files": fs.iter().map(|f| json!({"file": f.name, "data": f.data.is_some(), "scanned": f.scanned, "unread": f.unread})).collect::<Vec<_>>(),
        }));
    }
    // The latest revision not sent back wins, its text or (a scan) an agent's reading of it; an
    // older revision's values are superseded, kept as history only.
    let latest = ordered.iter().find(|(_, fs)| approved(&folder_status(fs)) != Some(false));
    if let Some(((a, b, folder), fs)) = latest {
        if let Some(f) = fs.iter().find(|f| f.data.is_some()) {
            out.data = f.data.clone();
            out.from = "submittal";
            out.cite = format!("{} p.{}", f.name, f.pages.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
            out.status = folder_status(fs);
            out.revision = Some((*a, *b));
            out.folder = folder.clone();
            out.unit_disconnect = fs.iter().find_map(|f| f.unit_disconnect.clone());
            return out;
        }
        if let Some(((oa, ob, _), _)) =
            ordered.iter().skip_while(|(k, _)| (k.0, k.1, &k.2) != (*a, *b, folder)).skip(1).find(|(_, fs)| fs.iter().any(|f| f.data.is_some()))
        {
            out.superseded = Some(format!("{oa}-{ob}"));
        }
    }
    if let Some(a) = docs.and_then(|d| d.answers.get(&key)) {
        let e = super::equipsub::from_answer(a);
        let src = a.get("source").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !super::equipsub::has_data(&e) {
            out.why = "the agent's answer gives no electrical value".into();
        } else if !has_page_cite(&src) {
            out.why = format!("an agent's answer with no page cited ({})", if src.is_empty() { "no source" } else { src.as_str() });
        } else {
            out.data = Some(e);
            out.from = "agent";
            out.cite = src;
            if let Some(((a, b, folder), fs)) = latest {
                out.status = folder_status(fs);
                out.revision = Some((*a, *b));
                out.folder = folder.clone();
            }
            return out;
        }
        return out;
    }
    if !files.is_empty() {
        let scanned = files.iter().filter(|f| f.scanned).count();
        let unread = files.iter().filter(|f| f.unread.is_some()).count();
        out.why = if ordered.iter().all(|(_, fs)| approved(&folder_status(fs)) == Some(false)) && files.iter().any(|f| f.data.is_some()) {
            "its submittals are all sent back (revise and resubmit / rejected): no approved values".into()
        } else if scanned > 0 {
            let older = out.superseded.as_ref().map(|r| format!("; revision {r}'s values are superseded")).unwrap_or_default();
            format!(
                "its latest submittal revision is scanned ({scanned} file(s) with no electrical text) and no agent has answered `equip:{}` with the page{older}",
                c.tag
            )
        } else {
            format!("its submittal files could not be read ({unread})")
        };
        return out;
    }
    if docs.is_none()
        && let Some(r) = c.row.filter(|r| super::equipsub::has_data(r))
    {
        let src = r.source.trim();
        let agent = src.to_ascii_lowercase().contains("agent");
        if src.is_empty() {
            out.why = "the model's record gives values but no source".into();
        } else if agent && !has_page_cite(src) {
            out.why = format!("an agent's answer with no page cited ({src})");
        } else {
            out.data = Some(r.clone());
            out.from = "record";
            out.cite = src.to_string();
            out.status = r.status.clone();
            return out;
        }
        return out;
    }
    out.why = if docs.is_none() {
        "no submittal: the job's submittals aren't on this machine and the model records none".into()
    } else {
        "no submittal found for it".into()
    };
    out
}

fn folder_name(f: &str) -> String {
    let p = std::path::Path::new(f);
    let last = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let up = p.parent().and_then(|x| x.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if up.is_empty() { last } else { format!("{up}/{last}") }
}

fn quote(t: &str) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > QUOTE_CHARS { format!("{}…", t.chars().take(QUOTE_CHARS).collect::<String>()) } else { t }
}

// ---------------------------------------------------------------- the check

pub(crate) struct Ctx<'a> {
    pub sp: &'a BuildingSpec,
    pub docs: Option<&'a EquipDocs>,
    pub frame: &'a super::validate::Frame,
    /// The scope's areas; `whole` (a building or level scope): every connection of their levels
    /// and those of no level, else only equipment standing in an area.
    pub areas: &'a [super::validate_packet::Area],
    pub whole: bool,
}

/// Is a connection in the scope?
fn in_scope(cx: &Ctx, c: &Connection) -> bool {
    use super::validate_packet::{inside, point_in};
    let levels: BTreeSet<String> = cx.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    match c.equipment {
        None => cx.whole,
        Some(e) if cx.whole => e.level.trim().is_empty() || levels.is_empty() || levels.contains(&e.level.to_ascii_lowercase()),
        Some(e) => {
            let mid = cadaddy_geom::Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0);
            let p = cx.frame.to_ft_on(&e.level, mid);
            cx.areas.iter().any(|a| a.level.eq_ignore_ascii_case(&e.level) && inside(a.bbox, p) && (a.outline.len() < 3 || point_in(&a.outline, p)))
        }
    }
}

/// What the rule found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, verdict, finding).
    pub findings: Vec<(u8, &'static str, Value)>,
    pub unmeasured: BTreeMap<String, u64>,
    pub connections: usize,
    /// The build's RFIs matched to a failure (confirmed), and its RFIs with none (disputed).
    pub confirmed: Vec<String>,
    pub disputed: Vec<String>,
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        "extra" | "missing" => 1,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

/// One check's outcome.
struct Outcome {
    check: &'static str,
    verdict: &'static str,
    issue: String,
    expected: String,
    found: String,
    recomputed: Value,
}

fn o(check: &'static str, verdict: &'static str, issue: String, expected: String, found: String, recomputed: Value) -> Outcome {
    Outcome { check, verdict, issue, expected, found, recomputed }
}

/// The build's equipment-connection RFI: (tag, check) from its `about` (`RTU-4 (protection)`).
fn rfi_subject(about: &str) -> Option<(String, String)> {
    let a = about.trim();
    let (tag, rest) = a.split_once('(')?;
    let check = rest.trim().strip_suffix(')')?.trim().to_ascii_lowercase();
    let tag = tag.trim();
    (!tag.is_empty() && !tag.contains(' ') && !check.is_empty()).then(|| (tag.to_ascii_uppercase(), check))
}

/// The NEC Chapter 9 check of one set's conductors in its conduit, for each raceway type the
/// tables list: (fits all, fits none, per type).
fn fill_of(w: &Wiring) -> Result<(bool, bool, Value, f64, f64), String> {
    let rl = super::conduitroute::rules();
    let n = w.conductors.ok_or("the number of conductors per set isn't given")?;
    let conduit = w.conduit_in.ok_or("the conduit size isn't given")?;
    let wire = |s: &str| rl.fill.wire_sq_in.get(&size_key(s)).copied();
    let a = wire(&w.size).ok_or_else(|| format!("no conductor area in the fill table for #{}", w.size))?;
    let (g, grounds) = if w.ground.trim().is_empty() {
        (0.0, 0u32)
    } else {
        (wire(&w.ground).ok_or_else(|| format!("no conductor area in the fill table for #{} (ground)", w.ground))?, 1)
    };
    let count = n.saturating_add(grounds);
    let area = f64::from(n) * a + f64::from(grounds) * g;
    let pct = match count {
        0 | 1 => rl.fill.one_pct,
        2 => rl.fill.two_pct,
        _ => rl.fill.over_two_pct,
    } / 100.0;
    let mut per = serde_json::Map::new();
    let (mut any_fit, mut any_short) = (false, false);
    let mut least = f64::INFINITY;
    for (kind, table) in &rl.fill.area_sq_in {
        let Some([_, inside]) = table.iter().find(|[t, _]| (t - conduit).abs() < 1e-6) else { continue };
        let allowed = inside * pct;
        least = least.min(allowed);
        let fits = area <= allowed + 1e-9;
        any_fit |= fits;
        any_short |= !fits;
        per.insert(kind.clone(), json!({"allowed_sq_in": r3(allowed), "fits": fits}));
    }
    if per.is_empty() {
        return Err(format!("no {conduit} in raceway in the fill tables"));
    }
    Ok((any_fit && !any_short, !any_fit, Value::Object(per), area, if least.is_finite() { least } else { 0.0 }))
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn amps(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 { format!("{v:.0} A") } else { format!("{v:.1} A") }
}

#[cfg(test)]
#[test]
fn numbers_read_short() {
    assert_eq!((fmt_n(0.75), fmt_n(70.0), fmt_n(1.25)), ("0.75".to_string(), "70".to_string(), "1.25".to_string()));
}

/// The checks of one connection.
fn check_one(c: &Connection, st: &Settled, rule: &super::coderules::CodeRule, sp: &BuildingSpec) -> Vec<Outcome> {
    let mut out = Vec::new();
    let tag = &c.tag;
    let cite = if st.cite.is_empty() { "its submittal".to_string() } else { st.cite.clone() };
    let source_panel = c.circuit.map(|(p, _)| p.to_string()).or_else(|| c.row.map(|r| r.source_panel.clone())).unwrap_or_default();
    // The model's protection: its schedule row's breaker, else the record's one-line protection.
    let breaker = c
        .circuit
        .and_then(|(_, k)| amps_in(&k.breaker).map(|a| (a, format!("{}A/{}P breaker at {source_panel} circuit {}", fmt_n(a), k.poles, k.number))));
    let breaker = breaker.or_else(|| {
        c.row.and_then(|r| {
            super::equipsub::ocpd_of(&r.drawing_ocpd).map(|(a, p)| (a, format!("{}A/{p}P at {} (the one-line)", fmt_n(a), r.source_panel)))
        })
    });
    let wire = c.circuit.and_then(|(_, k)| wiring(&k.wire));
    let disc = model_disconnect(sp, c);
    let unsure_all = |why: &str| why.to_string();
    let Some(data) = st.data.as_ref() else {
        let why = unsure_all(&st.why);
        for check in ["protection", "conductors", "disconnect", "sccr"] {
            out.push(o(check, "unsure", format!("{tag}: {why}"), "the submittal's value".into(), "no value to check against".into(), Value::Null));
        }
        // Conduit fill needs no submittal: the model's conductors in their conduit.
        out.push(fill_outcome(tag, wire.as_ref()));
        return out;
    };
    // ---- protection
    let class = if data.fuse_class.trim().is_empty() { None } else { Some(data.fuse_class.trim().to_ascii_uppercase()) };
    let max_s = |m: f64| format!("at most {}{} ({cite})", amps(m), class.as_ref().map(|c| format!(" Class {c} fuses")).unwrap_or_default());
    out.push(match (data.max_ocpd_a, &breaker) {
        (None, _) => o("protection", "unsure", format!("{tag}: its submittal gives no maximum protection (MOP / max fuse)"), "the submittal's maximum".into(), breaker.as_ref().map(|b| b.1.clone()).unwrap_or_default(), Value::Null),
        (Some(m), None) => o("protection", "unsure", format!("{tag}: the model has no protection for it (no panel schedule row, no one-line protection)"), max_s(m), "none".into(), json!({"max_ocpd_a": m})),
        (Some(m), Some((a, what))) if *a > m + 1e-9 => o(
            "protection",
            "fail",
            format!("{tag}: the model protects it with {what}; its submittal allows at most {}{} ({cite})", amps(m), class.as_ref().map(|c| format!(" Class {c}")).unwrap_or_default()),
            max_s(m),
            what.clone(),
            json!({"max_ocpd_a": m, "model_a": a, "fuse_class": class}),
        ),
        (Some(m), Some((a, what))) => match &class {
            None => o("protection", "pass", format!("{tag}: {what} is within the submittal's {} maximum", amps(m)), max_s(m), what.clone(), json!({"max_ocpd_a": m, "model_a": a})),
            Some(cl) => match disc.as_ref().map(|d| (d.fused, d.fuse_class.clone())) {
                Some((Some(true), Some(dc))) if dc == *cl => o("protection", "pass", format!("{tag}: {what}, and Class {cl} fuses in its disconnect, within the submittal's {} maximum", amps(m)), max_s(m), format!("{what}; disconnect fused Class {dc}"), json!({"max_ocpd_a": m, "model_a": a, "fuse_class": cl})),
                Some((Some(false), _)) => o("protection", "fail", format!("{tag}: its submittal calls for Class {cl} fuses ({cite}); the model's disconnect is not fused"), max_s(m), format!("{what}; disconnect not fused"), json!({"max_ocpd_a": m, "model_a": a, "fuse_class": cl})),
                Some((Some(true), Some(dc))) => o("protection", "fail", format!("{tag}: its submittal calls for Class {cl} fuses ({cite}); the model's disconnect has Class {dc}"), max_s(m), format!("{what}; fuses Class {dc}"), json!({"max_ocpd_a": m, "model_a": a, "fuse_class": cl})),
                _ => o("protection", "unsure", format!("{tag}: {what} is within the {} maximum, but its submittal calls for Class {cl} fuses ({cite}) and the model doesn't record them", amps(m)), max_s(m), what.clone(), json!({"max_ocpd_a": m, "model_a": a, "fuse_class": cl})),
            },
        },
    });
    // ---- conductors
    let factor = rule.conductor_factor.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(1.25);
    let need = data.mca.map(|m| (m, format!("its minimum circuit ampacity {} ({cite})", amps(m)))).or_else(|| {
        data.fla
            .map(|f| (f, "full-load"))
            .or(data.nameplate_a.map(|n| (n, "nameplate")))
            .map(|(a, what)| (a * factor, format!("{factor} x its {what} {} = {} ({cite}; {})", amps(a), amps(a * factor), rule.id)))
    });
    out.push(match (&need, &wire) {
        (None, _) => o(
            "conductors",
            "unsure",
            format!("{tag}: its submittal gives no MCA, full-load or nameplate amps"),
            "the submittal's current".into(),
            wire.as_ref().map(wire_text).unwrap_or_default(),
            Value::Null,
        ),
        (Some((n, why)), None) => o(
            "conductors",
            "unsure",
            format!("{tag}: the model's conductors for it aren't known (its panel schedule row gives no wire)"),
            format!("at least {} ({why})", amps(*n)),
            "none".into(),
            json!({"need_a": n}),
        ),
        (Some((n, why)), Some(w)) => {
            let have = rule.ampacity_75c.get(&w.material).and_then(|t| t.get(&size_key(&w.size))).map(|a| a * f64::from(w.sets.max(1)));
            match have {
                None => o(
                    "conductors",
                    "unsure",
                    format!("{tag}: no 75 C ampacity in the rules for #{} {}", w.size, w.material),
                    format!("at least {} ({why})", amps(*n)),
                    wire_text(w),
                    json!({"need_a": n}),
                ),
                Some(h) if h + 1e-9 < *n => o(
                    "conductors",
                    "fail",
                    format!("{tag}: the model's {} carry {} (75 C); it needs at least {} ({why})", wire_text(w), amps(h), amps(*n)),
                    format!("at least {}", amps(*n)),
                    format!("{} = {}", wire_text(w), amps(h)),
                    json!({"need_a": n, "have_a": h, "material_assumed": !w.material_given}),
                ),
                Some(h) => o(
                    "conductors",
                    "pass",
                    format!(
                        "{tag}: the model's {} carry {} (75 C), at least the {} it needs ({why}){}",
                        wire_text(w),
                        amps(h),
                        amps(*n),
                        if w.material_given { "" } else { "; copper, the schedule's default" }
                    ),
                    format!("at least {}", amps(*n)),
                    format!("{} = {}", wire_text(w), amps(h)),
                    json!({"need_a": n, "have_a": h, "material_assumed": !w.material_given}),
                ),
            }
        }
    });
    // ---- conduit fill
    out.push(fill_outcome(tag, wire.as_ref()));
    // ---- disconnect
    let dfactor = rule.disconnect_factor.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(1.15);
    let base = data
        .fla
        .map(|a| (a, "full-load"))
        .or(data.nameplate_a.map(|a| (a, "nameplate")))
        .or(data.mca.map(|a| (a / factor, "minimum circuit amps / factor")));
    let fused_needed = class.is_some();
    let mut need_a = base.map(|(a, _)| a * dfactor);
    if fused_needed && let (Some(n), Some(m)) = (need_a, data.max_ocpd_a) {
        need_a = Some(n.max(m));
    }
    let std_size = need_a.and_then(|n| rule.disconnect_sizes_a.iter().copied().filter(|s| s.is_finite()).find(|s| *s + 1e-9 >= n));
    let dcite = if rule.disconnect_cite.is_empty() { "NEC 430.110, 440.12, 620.51".to_string() } else { rule.disconnect_cite.clone() };
    let expect_d = match (need_a, std_size) {
        (Some(n), Some(s)) => {
            format!("a {}disconnect of at least {} (the next standard size {}; {dcite})", if fused_needed { "fused " } else { "" }, amps(n), amps(s))
        }
        (Some(n), None) => format!("a {}disconnect of at least {} ({dcite})", if fused_needed { "fused " } else { "" }, amps(n)),
        _ => format!("a disconnect sized to its full-load current ({dcite})"),
    };
    let drec = json!({"need_a": need_a, "standard_a": std_size, "fused": fused_needed, "base": base.map(|(a, w)| json!({"amps": a, "of": w})), "factor": dfactor});
    out.push(match (&disc, &st.unit_disconnect) {
        (None, Some(words)) => o(
            "disconnect",
            "pass",
            format!("{tag}: its submittal includes its disconnect (\"{words}\"), sized by its maker"),
            expect_d,
            format!("unit-mounted per the submittal ({cite})"),
            drec,
        ),
        (None, None) => o(
            "disconnect",
            "unsure",
            format!("{tag}: the model has no disconnect for it (none names it, none on its circuit)"),
            expect_d,
            "none".into(),
            drec,
        ),
        (Some(d), _) => {
            let what = format!("disconnect {} ({})", d.tag, d.text);
            if fused_needed && d.fused == Some(false) {
                o(
                    "disconnect",
                    "fail",
                    format!("{tag}: its submittal calls for fuses (Class {}); the model's {} is not fused", class.clone().unwrap_or_default(), d.tag),
                    expect_d,
                    what,
                    drec,
                )
            } else {
                match (d.amps, need_a) {
                    (Some(have), Some(n)) if have + 1e-9 < n => o(
                        "disconnect",
                        "fail",
                        format!("{tag}: the model's disconnect {} is {}; it needs at least {} ({dcite})", d.tag, amps(have), amps(n)),
                        expect_d,
                        what,
                        drec,
                    ),
                    (Some(_), Some(_)) if fused_needed && d.fused.is_none() => o(
                        "disconnect",
                        "unsure",
                        format!(
                            "{tag}: the model's disconnect {} is large enough but isn't said to be fused (its submittal calls for Class {} fuses)",
                            d.tag,
                            class.clone().unwrap_or_default()
                        ),
                        expect_d,
                        what,
                        drec,
                    ),
                    (Some(have), Some(n)) => o(
                        "disconnect",
                        "pass",
                        format!("{tag}: the model's disconnect {} is {}, at least the {} it needs", d.tag, amps(have), amps(n)),
                        expect_d,
                        what,
                        drec,
                    ),
                    (None, _) => o("disconnect", "unsure", format!("{tag}: the model's disconnect {} has no rating", d.tag), expect_d, what, drec),
                    (Some(_), None) => o(
                        "disconnect",
                        "unsure",
                        format!("{tag}: its submittal gives no full-load or nameplate amps to size the disconnect"),
                        expect_d,
                        what,
                        drec,
                    ),
                }
            }
        }
    });
    // ---- sccr
    let afc = source_afc(sp, &source_panel);
    out.push(match (data.sccr_a, &afc) {
        (None, _) => o(
            "sccr",
            "unsure",
            format!("{tag}: its submittal gives no short-circuit current rating"),
            "the submittal's SCCR".into(),
            afc.as_ref().map(|a| a.1.clone()).unwrap_or_default(),
            Value::Null,
        ),
        (Some(s), None) => o(
            "sccr",
            "unsure",
            format!(
                "{tag}: no available fault current is known at its source {}",
                if source_panel.is_empty() { "(no source panel)".to_string() } else { source_panel.clone() }
            ),
            format!("an SCCR of at least the available fault current ({})", rule.id),
            format!("SCCR {} ({cite})", amps(s)),
            json!({"sccr_a": s}),
        ),
        (Some(s), Some((a, from))) if s + 1e-9 < *a => o(
            "sccr",
            "fail",
            format!(
                "{tag}: its short-circuit current rating is {} ({cite}); {source_panel} has {} available fault current ({from})",
                amps(s),
                amps(*a)
            ),
            format!("an SCCR of at least {}", amps(*a)),
            format!("SCCR {}", amps(s)),
            json!({"sccr_a": s, "afc_a": a, "afc_from": from}),
        ),
        (Some(s), Some((a, from))) => o(
            "sccr",
            "pass",
            format!("{tag}: its short-circuit current rating {} is at least the {} available at {source_panel}", amps(s), amps(*a)),
            format!("an SCCR of at least {}", amps(*a)),
            format!("SCCR {}", amps(s)),
            json!({"sccr_a": s, "afc_a": a, "afc_from": from}),
        ),
    });
    out
}

/// A number as short as it reads (`70`, `1.25`, `0.75`).
fn fmt_n(v: f64) -> String {
    let t = format!("{v:.3}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() || t == "-" { "0".into() } else { t.to_string() }
}

fn wire_text(w: &Wiring) -> String {
    let n = w.conductors.map(|n| format!("{n} ")).unwrap_or_default();
    let feeder = if w.feeder.is_empty() { String::new() } else { format!("feeder {}: ", w.feeder) };
    let conduit = w.conduit_in.map(|c| format!(" in {}\" conduit", fmt_n(c))).unwrap_or_default();
    let ground = if w.ground.is_empty() { String::new() } else { format!(", #{} ground", w.ground) };
    format!("{feeder}{} set(s) of {n}#{} {}{ground}{conduit}", w.sets, w.size, w.material.to_ascii_uppercase())
}

fn fill_outcome(tag: &str, wire: Option<&Wiring>) -> Outcome {
    let rl = super::conduitroute::rules();
    let src = rl.fill.source.clone();
    let Some(w) = wire else {
        return o(
            "conduit_fill",
            "unsure",
            format!("{tag}: the model's conductors and conduit for it aren't known"),
            format!("the conductors within the fill ({src})"),
            "none".into(),
            Value::Null,
        );
    };
    match fill_of(w) {
        Err(why) => {
            o("conduit_fill", "unsure", format!("{tag}: {why}"), format!("the conductors within the fill ({src})"), wire_text(w), Value::Null)
        }
        Ok((all, none, per, area, least)) => {
            let rec = json!({"conductors_sq_in": r3(area), "by_raceway": per});
            if all {
                o(
                    "conduit_fill",
                    "pass",
                    format!("{tag}: {} sq in of conductors fit its conduit in every raceway the tables list", fmt3(area)),
                    format!("the conductors within the fill ({src})"),
                    wire_text(w),
                    rec,
                )
            } else if none {
                o(
                    "conduit_fill",
                    "fail",
                    format!(
                        "{tag}: {} sq in of conductors overfill its conduit (at most {} sq in in any raceway the tables list)",
                        fmt3(area),
                        fmt3(least)
                    ),
                    format!("the conductors within the fill ({src})"),
                    wire_text(w),
                    rec,
                )
            } else {
                o(
                    "conduit_fill",
                    "unsure",
                    format!(
                        "{tag}: {} sq in of conductors fit its conduit in some raceways and not others; the raceway type isn't given",
                        fmt3(area)
                    ),
                    format!("the conductors within the fill ({src})"),
                    wire_text(w),
                    rec,
                )
            }
        }
    }
}

fn fmt3(v: f64) -> String {
    format!("{:.3}", r3(v))
}

/// The revision check: the submittal the model's record names is the latest approved one.
fn revision_outcome(c: &Connection, st: &Settled, docs: Option<&EquipDocs>) -> Option<Outcome> {
    let row = c.row?;
    let tag = &c.tag;
    let used = row.source.trim();
    if approved(row.status.trim()) == Some(false) {
        return Some(o(
            "revision",
            "fail",
            format!("{tag}: the model follows a submittal sent back ({}: {})", if used.is_empty() { "its record" } else { used }, row.status),
            "the latest approved revision".into(),
            format!("{used} ({})", row.status),
            Value::Null,
        ));
    }
    let name = used.strip_prefix("submittal ").map(str::trim).filter(|n| n.to_ascii_lowercase().ends_with(".pdf"))?;
    let files = docs.and_then(|d| d.by_tag.get(&tag.to_ascii_uppercase()))?;
    let mine: Vec<&SubFile> = files.iter().filter(|f| f.name.eq_ignore_ascii_case(name)).collect();
    if mine.is_empty() {
        return Some(o(
            "revision",
            "unsure",
            format!("{tag}: the submittal the model names ({name}) isn't among the job's"),
            "the latest approved revision".into(),
            name.to_string(),
            Value::Null,
        ));
    }
    let winner = st.revision.map(|r| (r, st.folder.clone()));
    let in_winner = winner.as_ref().is_some_and(|(_, f)| mine.iter().any(|m| m.folder == *f));
    let folder_files = |folder: &str| files.iter().filter(|f| f.folder == folder).collect::<Vec<_>>();
    let theirs_status = mine.first().map(|m| folder_status(&folder_files(&m.folder))).unwrap_or_default();
    Some(if in_winner {
        o(
            "revision",
            "pass",
            format!("{tag}: the model follows {name}, the latest approved revision"),
            "the latest approved revision".into(),
            name.to_string(),
            json!({"revision": winner.map(|(r, _)| format!("{}-{}", r.0, r.1))}),
        )
    } else if approved(&theirs_status) == Some(false) {
        o(
            "revision",
            "fail",
            format!("{tag}: the model follows {name}, a revision sent back ({theirs_status})"),
            "the latest approved revision".into(),
            name.to_string(),
            Value::Null,
        )
    } else if let Some((r, f)) = winner {
        o(
            "revision",
            "fail",
            format!(
                "{tag}: the model follows {name} (rev {}-{}); the latest approved is rev {}-{} ({})",
                mine.first().map_or(0, |m| m.revision.0),
                mine.first().map_or(0, |m| m.revision.1),
                r.0,
                r.1,
                st.cite
            ),
            format!("{} ({})", st.cite, folder_name(&f)),
            name.to_string(),
            json!({"latest": format!("{}-{}", r.0, r.1)}),
        )
    } else {
        o(
            "revision",
            "unsure",
            format!("{tag}: no approved revision of its submittal with electrical data was found"),
            "the latest approved revision".into(),
            name.to_string(),
            Value::Null,
        )
    })
}

/// Check every connection in scope, then match the build's RFIs against the verdicts.
pub(crate) fn check(cx: &Ctx, r: &Effective) -> Checked {
    let mut out = Checked::default();
    let rule = super::coderules::rules().into_iter().find(|x| x.id == RULE).unwrap_or_default();
    let sev = r.rule.severity.clone();
    let conns = connections(cx.sp);
    let mut verdicts: BTreeMap<(String, String), (&'static str, String)> = BTreeMap::new();
    let mut fails: Vec<(String, String, String, usize)> = Vec::new();
    for c in &conns {
        let level = c.equipment.map(|e| e.level.clone()).unwrap_or_default();
        if !in_scope(cx, c) {
            continue;
        }
        out.connections += 1;
        let st = settle(c, cx.docs);
        if st.data.is_none() {
            *out.unmeasured.entry(if st.why.is_empty() { "no submittal values".into() } else { generic_why(&st.why) }).or_default() += 1;
        }
        let mut results = check_one(c, &st, &rule, cx.sp);
        if let Some(rv) = revision_outcome(c, &st, cx.docs) {
            results.push(rv);
        } else if c.row.is_some_and(|r| r.source.trim().is_empty()) {
            *out.unmeasured.entry("revision: the model records no submittal for it".into()).or_default() += 1;
        }
        let submittal = json!({
            "from": if st.from.is_empty() { Value::Null } else { json!(st.from) },
            "cite": st.cite, "status": if st.status.is_empty() { Value::Null } else { json!(st.status) },
            "revision": st.revision.map(|r| format!("{}-{}", r.0, r.1)), "page": cited_page(&st.cite),
            "why": if st.why.is_empty() { Value::Null } else { json!(st.why) },
            "values": st.data.as_ref().map(values_json), "revisions": st.history,
        });
        let model = json!({
            "panel": c.circuit.map(|(p, _)| p), "circuit": c.circuit.map(|(_, k)| json!({"number": k.number, "description": k.description, "breaker": k.breaker, "poles": k.poles, "wire": k.wire})),
            "record": c.row.map(|r| json!({"source_panel": r.source_panel, "drawing_ocpd": r.drawing_ocpd, "feeder": r.feeder, "source": r.source, "status": r.status})),
            "disconnect": model_disconnect(cx.sp, c).map(|d| json!({"tag": d.tag, "amps": d.amps, "fused": d.fused, "fuse_class": d.fuse_class})),
        });
        for res in results {
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|{}|{}|{}", c.tag, res.check, res.verdict)));
            let f = json!({
                "id": id, "rule": RULE, "handle": Value::Null, "kind": "equipment_connection", "tag": c.tag, "verdict": res.verdict,
                "issue": res.issue, "expected": res.expected, "found": res.found,
                "measured": {"check": res.check, "tag": c.tag, "description": c.row.map(|r| r.description.clone()).or_else(|| c.circuit.map(|(_, k)| k.description.clone())), "submittal": submittal, "model": model, "recomputed": res.recomputed},
                "evidence": [], "confidence": if res.verdict == "unsure" { 0.5 } else { 1.0 }, "severity": sev, "area": Value::Null, "level": level, "at": Value::Null,
                "source": "prescreen",
            });
            verdicts.insert((c.tag.to_ascii_uppercase(), res.check.to_string()), (res.verdict, res.issue.clone()));
            if res.verdict == "fail" {
                fails.push((c.tag.to_ascii_uppercase(), res.check.to_string(), res.issue.clone(), out.findings.len()));
            }
            out.findings.push((rank(res.verdict, &sev), res.verdict, f));
        }
    }
    // The build's RFIs against the verdicts.
    let tags: BTreeSet<String> = conns.iter().map(|c| c.tag.to_ascii_uppercase()).collect();
    let mut raised: BTreeSet<(String, String)> = BTreeSet::new();
    for item in cx.sp.rfis.iter().take(MAX_RFIS).filter(|i| i.kind_name() == RFI_KIND) {
        if !matches!(item.status, cadaddy_walk3d::rfi::RfiStatus::Open | cadaddy_walk3d::rfi::RfiStatus::Sent) {
            continue;
        }
        let Some((tag, check)) = rfi_subject(&item.about).filter(|(t, _)| tags.contains(t)) else {
            *out.unmeasured.entry("an equipment_electrical RFI about no equipment connection (not matched)".into()).or_default() += 1;
            continue;
        };
        if !cx.whole && !out.findings.iter().any(|(_, _, f)| f["tag"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(&tag))) {
            continue;
        }
        raised.insert((tag.clone(), check.clone()));
        match verdicts.get(&(tag.clone(), check.clone())) {
            Some(("fail", _)) => {
                out.confirmed.push(item.id.clone());
                if let Some((_, _, _, at)) = fails.iter().find(|(t, k, _, _)| *t == tag && *k == check)
                    && let Some((_, _, f)) = out.findings.get_mut(*at)
                {
                    f["measured"]["rfi"] = json!(item.id);
                }
            }
            other => {
                out.disputed.push(item.id.clone());
                let (verdict, why) = match other {
                    Some((v, why)) => (*v, why.clone()),
                    None => ("none", format!("Validate has no `{check}` check")),
                };
                let issue =
                    format!("the build's RFI {} ({tag} {check}) has no matching failure: Validate's {check} check is {verdict}: {why}", item.id);
                out.findings.push((
                    rank("extra", &sev),
                    "extra",
                    json!({
                        "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|rfi|{}", item.id))), "rule": RULE, "handle": Value::Null,
                        "kind": "equipment_connection", "tag": tag, "verdict": "extra", "issue": issue,
                        "expected": "an RFI only where Validate's recomputation fails", "found": quote(&item.issue),
                        "measured": {"check": check, "tag": tag, "rfi": item.id, "validate": verdict, "working": why, "rfi_issue": quote(&item.issue), "rfi_proposed": item.proposed},
                        "evidence": [], "confidence": if verdict == "unsure" { 0.5 } else { 1.0 }, "severity": sev, "area": Value::Null, "level": item.level, "at": Value::Null, "source": "prescreen",
                    }),
                ));
            }
        }
    }
    for (tag, check, issue, _) in &fails {
        if raised.contains(&(tag.clone(), check.clone())) {
            continue;
        }
        out.findings.push((
            rank("missing", &sev),
            "missing",
            json!({
                "id": format!("PS-{}", super::validate_run::hash8(&format!("{RULE}|nofri|{tag}|{check}"))), "rule": RULE, "handle": Value::Null,
                "kind": "equipment_connection", "tag": tag, "verdict": "missing",
                "issue": format!("Validate fails {tag}'s {check} and the build raised no RFI for it: {issue}"),
                "expected": format!("an `{RFI_KIND}` RFI about {tag} ({check})"), "found": "none",
                "measured": {"check": check, "tag": tag, "rfi": Value::Null},
                "evidence": [], "confidence": 1.0, "severity": sev, "area": Value::Null, "level": "", "at": Value::Null, "source": "prescreen",
            }),
        ));
    }
    out
}

/// A why with its tag taken out, to count alike reasons together.
fn generic_why(why: &str) -> String {
    let w = why.split('`').enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, s)| s).collect::<Vec<_>>().join("<tag>");
    w.split(" (").next().unwrap_or(&w).trim().to_string()
}

fn values_json(e: &EquipmentElectrical) -> Value {
    json!({
        "volts": e.volts, "phase": e.phase, "nameplate_a": e.nameplate_a, "fla": e.fla, "mca": e.mca, "max_ocpd_a": e.max_ocpd_a,
        "fuse_class": if e.fuse_class.is_empty() { Value::Null } else { json!(e.fuse_class) }, "accel_a": e.accel_a, "hp": e.hp, "kw": e.kw, "sccr_a": e.sccr_a,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_rows_wire_reads_as_feeder_or_bare_size() {
        let w = wiring("feeder 74: 1 set(s) of 4 4 cu, ground 8, in 1.25\" conduit").unwrap();
        assert_eq!(
            (w.feeder.as_str(), w.sets, w.conductors, w.size.as_str(), w.material.as_str(), w.ground.as_str()),
            ("74", 1, Some(4), "4", "cu", "8")
        );
        assert_eq!(w.conduit_in, Some(1.25));
        let w = wiring("feeder 31: 2 set(s) of 4 350 kcmil al, ground 1/0, in 3\" conduit").unwrap();
        assert_eq!((w.sets, w.size.as_str(), w.material.as_str(), w.material_given), (2, "350 kcmil", "al", true));
        assert_eq!(size_key(&w.size), "350");
        let w = wiring("10").unwrap();
        assert_eq!((w.size.as_str(), w.material.as_str(), w.material_given, w.conductors), ("10", "cu", false, None));
        assert!(wiring("").is_none() && wiring("SEE ONE-LINE").is_none());
        assert!(wiring("feeder 9: 4000000000 set(s) of 4 4 cu").is_none(), "hostile counts are refused");
    }

    #[test]
    fn rfis_name_their_connection_and_check() {
        assert_eq!(rfi_subject("PMP-2 (protection)"), Some(("PMP-2".into(), "protection".into())));
        assert_eq!(rfi_subject("Fire smoke damper power"), None);
        assert_eq!(rfi_subject("two words (sccr)"), None);
    }

    #[test]
    fn an_answer_counts_only_with_its_page() {
        assert_eq!(cited_page("Pump submittal.pdf p.9"), Some(9));
        assert_eq!(cited_page("data sheet, page 12"), Some(12));
        assert_eq!(cited_page("pg 3 of the scan"), Some(3));
        assert_eq!(cited_page("an agent's reading of the submittal"), None);
        assert_eq!(cited_page("the shop drawings"), None);
    }

    #[test]
    fn fault_currents_and_amps_read_as_printed() {
        assert_eq!(fault_amps("41,500A"), Some(41_500.0));
        assert_eq!(fault_amps("22 kA"), Some(22_000.0));
        assert_eq!(amps_in("70A/3P"), Some(70.0));
        assert_eq!(amps_in("none"), None);
    }

    #[test]
    fn a_folder_is_approved_only_when_no_stamp_sends_it_back() {
        let f = |s: &str| SubFile { status: s.into(), ..SubFile::default() };
        let (a, b, c) = (f("exceptions as noted"), f(""), f("revise and resubmit"));
        assert_eq!(folder_status(&[&a, &b]), "exceptions as noted");
        assert_eq!(folder_status(&[&c, &b]), "revise and resubmit");
        assert_eq!(folder_status(&[&a, &c]), "");
    }

    #[test]
    fn conduit_fill_fits_every_raceway_or_none() {
        let w = wiring("feeder 1: 1 set(s) of 4 4 cu, ground 8, in 1.25\" conduit").unwrap();
        let (all, none, ..) = fill_of(&w).unwrap();
        assert!(all && !none);
        let w = wiring("feeder 1: 1 set(s) of 4 4/0 cu, ground 4, in 1\" conduit").unwrap();
        let (all, none, ..) = fill_of(&w).unwrap();
        assert!(!all && none);
        assert!(fill_of(&wiring("12").unwrap()).is_err(), "no conduit: not measured");
    }
}
