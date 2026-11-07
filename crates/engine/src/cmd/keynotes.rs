//! Keynotes as the sheets print them: a `KEYNOTES` heading (`ELECTRICAL KEYNOTES:`), numbered
//! notes under it (`3. BDA SYSTEM CEILING MOUNTED ANTENNA LOCATION. PROVIDE 8"X8"X6" JUNCTION
//! BOX …`), and on the plans each note's number in a small box (a square, or a hexagon) where it
//! applies. What a note asks to be built is read from its words ([`work`]): a box of a size, and
//! conduit of a size between each of its boxes (a riser), stubbed into a space, or run to its
//! system's riser.

use std::sync::OnceLock;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{PanelCircuit, PanelSchedule};
use serde::Deserialize;

/// The systems keynotes build and the parts of each the documents must place
/// ([`SystemRules`]).
const RULES: &str = include_str!("keynotes.json");

/// A system keynotes build (`BDA`): its names, and the parts of it the job looks for in the
/// documents (a head-end and its power, a donor antenna) and leaves `undecided` when no
/// document places them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SystemRules {
    pub name: String,
    pub also: Vec<String>,
    pub title: String,
    pub source: String,
    pub parts: Vec<PartRule>,
}

/// One part of a system: what it is, the words a note or schedule names it by, and whether it
/// takes power (a branch circuit of its own).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PartRule {
    pub part: String,
    pub what: String,
    pub words: Vec<String>,
    pub powered: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    systems: Vec<SystemRules>,
    equipment_receptacle: ReceptacleRules,
    device_requirements: DeviceRules,
    unpowered_boxes: UnpoweredBoxes,
}

/// The roles of a keynote's box that no branch circuit feeds (`antenna`), and why.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct UnpoweredBoxes {
    pub roles: Vec<String>,
    pub source: String,
}

/// The rules data for the boxes no branch circuit feeds.
pub(crate) fn unpowered_boxes() -> &'static UnpoweredBoxes {
    &rules().unpowered_boxes
}

/// How a keynote asking something of the device at its marker is read
/// ([`device_requirements`]): each kind of requirement's words.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DeviceRules {
    pub source: String,
    pub features: Vec<FeatureRule>,
    pub gang_words: Vec<String>,
    pub gang_kinds: Vec<String>,
    pub gang_source: String,
    pub mounts: Vec<MountRule>,
    pub lv_words: Vec<String>,
    pub lv_kinds: Vec<String>,
    pub lv_source: String,
    pub scope_words: Vec<String>,
    pub scope_kinds: Vec<ScopeKind>,
    pub scope_skip: Vec<String>,
    pub scope_reach_words: usize,
}

/// A device feature a note asks for (`tamper-resistant`, `USB-C`): how it is said, its words,
/// the kinds of device it is a feature of.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FeatureRule {
    pub say: String,
    pub words: Vec<String>,
    pub kinds: Vec<String>,
}

/// A mounting a note puts a device in (`ceiling`, `floor`), its words, the kinds it applies to
/// (none: any), and what the device then is (`floor box`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct MountRule {
    pub mount: String,
    pub words: Vec<String>,
    pub kinds: Vec<String>,
    pub device_type: String,
}

/// A kind of device a note names for a set of rooms (`ALL … RECEPTACLES`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ScopeKind {
    pub kind: String,
    pub words: Vec<String>,
}

/// How an equipment receptacle's keynote is read ([`equipment_receptacle`]): its words, the
/// sub-base's height when the note puts it there, and the NEMA configuration of a rating.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ReceptacleRules {
    pub receptacle_words: Vec<String>,
    pub horizontal_words: Vec<String>,
    pub under_unit_words: Vec<String>,
    pub sub_base_words: Vec<String>,
    pub sub_base_in: f64,
    pub sub_base_source: String,
    pub nema_source: String,
    pub nema: Vec<NemaFamily>,
}

/// A receptacle family by its poles: its voltage and its configuration by amperes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct NemaFamily {
    pub poles: u32,
    pub volts: u32,
    pub by_amps: Vec<(u32, String)>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The rules data's systems.
pub(crate) fn systems() -> &'static [SystemRules] {
    &rules().systems
}

/// The rules data for equipment receptacles' keynotes.
pub(crate) fn receptacle_rules() -> &'static ReceptacleRules {
    &rules().equipment_receptacle
}

/// The rules data for keynotes that ask something of a device.
pub(crate) fn device_rules() -> &'static DeviceRules {
    &rules().device_requirements
}

/// What a keynote asks of the device at its marker ([`device_requirements`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DeviceReq {
    /// Features, as the rules data says them (`tamper-resistant`, `USB`, `USB-C`), each with the
    /// kinds of device it is a feature of.
    pub features: Vec<(String, Vec<String>)>,
    /// The devices at its marker share one box and plate.
    pub ganged: bool,
    /// Where it is mounted (`ceiling`, `floor`), the kinds that applies to (none: any), and what
    /// the device then is (`floor box`; empty: what it was).
    pub mount: Option<(String, Vec<String>, String)>,
    /// The low-voltage conduit the note gives its outlet, trade size inches.
    pub lv_conduit_in: Option<f64>,
    /// A note over a kind of device in some rooms (`ALL GUEST ROOM AND LOBBY 120V
    /// RECEPTACLES …`): the kind and the rooms' words (empty: every room).
    pub scope: Option<(String, Vec<String>)>,
    /// A box note's box ([`work`]: a system's antenna or riser box): its role, and whether no
    /// branch circuit feeds it (rules `unpowered_boxes`). Its box is at the ceiling.
    pub box_role: Option<String>,
    pub unpowered: bool,
}

impl DeviceReq {
    /// The features it asks of a device of `kind` (its kind's words: `receptacle`, `gfci
    /// receptacle`).
    pub fn features_for(&self, kind: &str) -> Vec<String> {
        let k = kind.replace('_', " ").to_ascii_lowercase();
        self.features.iter().filter(|(_, kinds)| kinds.is_empty() || kinds.iter().any(|x| k.contains(x.as_str()))).map(|(f, _)| f.clone()).collect()
    }

    /// The mounting it gives a device of `kind`, and what that device then is.
    pub fn mount_for(&self, kind: &str) -> Option<(String, String)> {
        let k = kind.replace('_', " ").to_ascii_lowercase();
        self.mount
            .as_ref()
            .filter(|(_, kinds, _)| kinds.is_empty() || kinds.iter().any(|x| k.contains(x.as_str())))
            .map(|(m, _, t)| (m.clone(), t.clone()))
    }

    /// Whether it asks anything of the one device at its marker (not only of a set of rooms).
    pub fn at_marker(&self) -> bool {
        !self.features.is_empty() || self.ganged || self.mount.is_some() || self.lv_conduit_in.is_some() || self.box_role.is_some()
    }
}

/// What a keynote asks of the device at its marker: its features (`TAMPER-RESISTANT`, `USB AND
/// USB-C`), that the devices there be ganged, how it is mounted (`CEILING MOUNTED`, a `FLOOR
/// BOX`), the size of its low-voltage conduit (`1" LOW-VOLTAGE CONDUIT`); and a note over a kind
/// of device in some rooms, those rooms. All its words are the rules data's
/// ([`device_rules`]). None: it asks none of these.
pub(crate) fn device_requirements(text: &str) -> Option<DeviceReq> {
    let r = device_rules();
    let text: String = text.chars().take(MAX_NOTE_CHARS).collect();
    let norm = |s: &str| {
        let s: String = s.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
        format!(" {} ", s.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let mut rest = norm(&text);
    let mut q = DeviceReq::default();
    // Features, each phrase taken once and longest first (`USB-C` is not also a `USB`).
    let mut phrases: Vec<(usize, String, &FeatureRule)> =
        r.features.iter().flat_map(|f| f.words.iter().map(move |w| (norm(w).len(), norm(w), f))).filter(|(n, _, _)| *n > 2).collect();
    phrases.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p, f) in &phrases {
        if rest.contains(p.as_str()) {
            rest = rest.replace(p.as_str(), " | ");
            if !q.features.iter().any(|(s, _)| *s == f.say) {
                q.features.push((f.say.clone(), f.kinds.clone()));
            }
        }
    }
    // Keep the rules data's order (a record lists them so).
    q.features.sort_by_key(|(s, _)| r.features.iter().position(|f| f.say == *s).unwrap_or(usize::MAX));
    q.ganged = r.gang_words.iter().any(|w| has_phrase(&text, w));
    q.mount =
        r.mounts.iter().find(|m| m.words.iter().any(|w| has_phrase(&text, w))).map(|m| (m.mount.clone(), m.kinds.clone(), m.device_type.clone()));
    // A low-voltage conduit's size: the trade size before `CONDUIT` (past the low-voltage words
    // between), in a note about low-voltage work, not in the ground.
    let up = text.to_ascii_uppercase();
    if r.lv_words.iter().any(|w| has_phrase(&text, w)) && !buried(&up) {
        let words: Vec<&str> = up.split_whitespace().collect();
        let lv_word = |w: &str| {
            let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            r.lv_words.iter().any(|x| x.split_whitespace().any(|p| p.trim_matches(|c: char| !c.is_ascii_alphanumeric()) == w)) || w == "VOLTAGE"
        };
        q.lv_conduit_in = words.iter().enumerate().filter(|(_, w)| w.starts_with("CONDUIT")).find_map(|(i, _)| {
            (1..=3).filter_map(|k| i.checked_sub(k)).take_while(|j| *j + 1 == i || words.get(*j + 1).is_some_and(|w| lv_word(w))).find_map(|j| {
                let w = words.get(j)?;
                super::gearbuild::trade_in(w.trim_end_matches([',', '.']))
            })
        });
    }
    // A note over a kind of device in some rooms: `ALL <room words> <kind words>`.
    let toks: Vec<String> = up.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|t| !t.is_empty()).map(str::to_string).collect();
    let reach = if r.scope_reach_words == 0 { 12 } else { r.scope_reach_words };
    'scope: for (i, t) in toks.iter().enumerate() {
        if !r.scope_words.iter().any(|w| w == t) {
            continue;
        }
        for j in (i + 1)..toks.len().min(i + 1 + reach) {
            let Some(tj) = toks.get(j) else { break };
            let Some(sk) = r.scope_kinds.iter().find(|k| k.words.iter().any(|w| w.split_whitespace().last() == Some(tj.as_str()))) else {
                continue;
            };
            let rooms: Vec<String> = toks
                .get(i + 1..j)
                .unwrap_or_default()
                .iter()
                .filter(|w| !w.chars().any(|c| c.is_ascii_digit()) && w.len() > 1)
                .filter(|w| !r.scope_skip.iter().any(|x| x == *w))
                .filter(|w| !sk.words.iter().any(|x| x.split_whitespace().any(|p| p == w.as_str())))
                .cloned()
                .collect();
            q.scope = Some((sk.kind.clone(), rooms));
            break 'scope;
        }
    }
    // A box note's box (a system's antenna or riser box): at the ceiling, as the note builds it.
    if let Some(w) = work(&text).filter(|w| w.box_in.is_some() && !w.role().is_empty()) {
        q.box_role = Some(w.role().to_string());
        q.unpowered = w.unpowered();
        q.mount = Some(("ceiling".to_string(), Vec::new(), String::new()));
    }
    (q.at_marker() || q.scope.is_some()).then_some(q)
}

/// What the keynotes at a device asked of it and it took ([`apply_to_tag`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Applied {
    pub features: Vec<String>,
    pub mount: Option<String>,
    pub device_type: Option<String>,
    pub lv_conduit_in: Option<f64>,
    pub ganged: bool,
    /// The circuit read by it that its keynote's box can't take (taken off).
    pub uncircuited: Option<String>,
}

/// The note's text of a keynote as the job cites it (`E-9 keynote 4: <the note>` → the note).
pub(crate) fn cited_text(cite: &str) -> &str {
    cite.split_once(": ").map_or(cite, |(_, t)| t)
}

/// Lays what `cites` (keynotes as the job cites them, `E-9 keynote 4: <the note>`) ask of a
/// device of `kind` onto its tag, each with the keynote as its source (`note`): its features
/// (`keynote_features`), mounting (`mount`) and what that makes it (`device_type`), its
/// low-voltage conduit (`lv_conduit_in`), the devices it is ganged with (`gang_with`, the key
/// they share), and the keynotes themselves (`keynotes`; `keynote` when it had none). What it
/// took.
pub(crate) fn apply_to_tag(t: &mut cadaddy_walk3d::tags::ElementTag, kind: &str, cites: &[String], gang: &str) -> Applied {
    let mut out = Applied::default();
    let mut taken: Vec<&String> = Vec::new();
    for c in cites.iter().take(MAX_NOTES) {
        let Some(q) = device_requirements(cited_text(c)) else { continue };
        let mut any = false;
        for f in q.features_for(kind) {
            any = true;
            if !out.features.contains(&f) {
                out.features.push(f);
            }
        }
        if let Some((m, ty)) = q.mount_for(kind) {
            any = true;
            out.mount = Some(m);
            if !ty.is_empty() {
                out.device_type = Some(ty);
            }
        }
        if let Some(v) = q.lv_conduit_in.filter(|_| lv_kind(kind)) {
            any = true;
            out.lv_conduit_in = Some(v);
        }
        if q.ganged && !gang.is_empty() {
            any = true;
            out.ganged = true;
        }
        // A box no branch circuit feeds: a circuit label read by its symbol is not its own. Taken
        // off, the read value kept beside it, with why.
        if q.unpowered
            && let Some(c) = t.fields.get("circuit").map(|f| f.value.clone()).filter(|v| !v.trim().is_empty())
        {
            any = true;
            t.fields.remove("circuit");
            t.set("circuit_read", c.clone(), "plan");
            t.set(
                "correction",
                format!(
                    "circuit {c} read by its symbol is not built: its keynote makes it a {} box, and {}",
                    q.box_role.as_deref().unwrap_or_default(),
                    unpowered_boxes().source
                ),
                "rule",
            );
            out.uncircuited = Some(c);
        }
        if any && !taken.contains(&c) {
            taken.push(c);
        }
    }
    if taken.is_empty() {
        return out;
    }
    let field = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| t.fields.get(k).map(|f| f.value.clone()).unwrap_or_default();
    let mut all: Vec<String> = field(t, "keynotes").split(" | ").filter(|x| !x.trim().is_empty()).map(str::to_string).collect();
    for c in &taken {
        if !all.iter().any(|x| x == *c) {
            all.push((*c).clone());
        }
    }
    t.set("keynotes", all.join(" | "), "note");
    if field(t, "keynote").is_empty()
        && let Some(c) = taken.first()
    {
        t.set("keynote", (*c).clone(), "note");
    }
    if !out.features.is_empty() {
        let mut f: Vec<String> = field(t, "keynote_features").split(", ").filter(|x| !x.trim().is_empty()).map(str::to_string).collect();
        for x in &out.features {
            if !f.contains(x) {
                f.push(x.clone());
            }
        }
        t.set("keynote_features", f.join(", "), "note");
        // What it is, in words, says them (`duplex receptacle, USB, USB-C`).
        let mut ty = field(t, "device_type");
        for x in &out.features {
            if !ty.to_ascii_lowercase().contains(&x.to_ascii_lowercase()) {
                ty = if ty.is_empty() { x.clone() } else { format!("{ty}, {x}") };
            }
        }
        t.set("device_type", ty, "note");
    }
    if let Some(m) = &out.mount {
        t.set("mount", m.clone(), "note");
    }
    if let Some(ty) = &out.device_type {
        let was = field(t, "device_type");
        if !was.to_ascii_lowercase().contains(&ty.to_ascii_lowercase()) {
            t.set("device_type", if was.is_empty() { ty.clone() } else { format!("{ty} {was}") }, "note");
        }
    }
    if let Some(v) = out.lv_conduit_in {
        t.set("lv_conduit_in", format!("{v}"), "note");
    }
    if out.ganged {
        t.set("gang_with", gang, "note");
    }
    out
}

/// Whether a device of `kind` is a low-voltage outlet (its pathway is low-voltage conduit).
pub(crate) fn lv_kind(kind: &str) -> bool {
    let k = format!(" {} ", kind.replace('_', " ").to_ascii_lowercase());
    // A kind's word at a word's start (`data` in `data outlet`, not in `metadata`).
    device_rules().lv_kinds.iter().any(|w| k.contains(&format!(" {}", w.trim().to_ascii_lowercase())))
}

/// What a keynote asks of a receptacle for a piece of equipment (`30A/2P RECEPTACLE FOR <unit>,
/// SET HORIZONTALLY IN ITS SUB-BASE`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EquipmentReceptacle {
    /// The equipment's kind (`equipment.room_unit_tags` / `equipment.typical_load_words`) and the
    /// words the note names it by.
    pub kind: String,
    pub named: String,
    /// Its rating as the note gives it: amperes, poles, volts (`30A/2P`, `208V`).
    pub amps: Option<u32>,
    pub poles: Option<u32>,
    pub volts: Option<u32>,
    /// A NEMA configuration the note prints (`6-20R`).
    pub nema: Option<String>,
    /// Set horizontally; under the unit; in its sub-base.
    pub horizontal: bool,
    pub under_unit: bool,
    pub sub_base: bool,
}

impl EquipmentReceptacle {
    /// Its configuration: the note's, else the family its poles and amperes make (rules data).
    pub fn configuration(&self) -> Option<String> {
        if self.nema.is_some() {
            return self.nema.clone();
        }
        let (poles, amps) = (self.poles?, self.amps?);
        let fam = receptacle_rules().nema.iter().find(|f| f.poles == poles)?;
        fam.by_amps.iter().find(|(a, _)| *a == amps).map(|(_, c)| c.clone())
    }

    /// Its voltage: the note's, else its family's.
    pub fn voltage(&self) -> Option<u32> {
        if self.volts.is_some() {
            return self.volts;
        }
        let from_nema = self.nema.as_deref().and_then(|n| match n.split('-').next() {
            Some("5") => Some(1),
            Some("6") => Some(2),
            _ => None,
        });
        let poles = self.poles.or(from_nema)?;
        receptacle_rules().nema.iter().find(|f| f.poles == poles).map(|f| f.volts).filter(|v| *v > 0)
    }

    /// Its rating as a record says it (`20 A, 250 V, 2-pole (NEMA 6-20R)`); none when the note
    /// gives no rating.
    pub fn rating(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(a) = self.amps {
            parts.push(format!("{a} A"));
        }
        if let Some(v) = self.voltage() {
            parts.push(format!("{v} V"));
        }
        if let Some(p) = self.poles {
            parts.push(format!("{p}-pole"));
        }
        if parts.is_empty() && self.nema.is_none() {
            return None;
        }
        let mut s = parts.join(", ");
        if let Some(c) = self.configuration() {
            s = if s.is_empty() { format!("NEMA {c}") } else { format!("{s} (NEMA {c})") };
        }
        Some(s)
    }

    /// What it is, in words (`receptacle 20 A, 250 V, 2-pole (NEMA 6-20R), horizontal, under
    /// the unit in its sub-base, for <tag>`).
    pub fn describe(&self, unit: &str) -> String {
        let mut s = String::from("receptacle");
        if let Some(r) = self.rating() {
            s.push_str(&format!(" {r}"));
        }
        if self.horizontal {
            s.push_str(", horizontal");
        }
        if self.sub_base {
            s.push_str(", under the unit in its sub-base");
        } else if self.under_unit {
            s.push_str(", under the unit");
        }
        s.push_str(&format!(", for {}", if unit.trim().is_empty() { self.named.as_str() } else { unit.trim() }));
        s
    }
}

/// A keynote calling for a receptacle for a piece of equipment the rules data names: what it
/// asks of that receptacle. None: no receptacle, or no equipment the data knows.
pub(crate) fn equipment_receptacle(text: &str) -> Option<EquipmentReceptacle> {
    let r = receptacle_rules();
    let text: String = text.chars().take(MAX_NOTE_CHARS).collect();
    if !r.receptacle_words.iter().any(|w| has_phrase(&text, w)) {
        return None;
    }
    // The equipment: a room unit's printed tag (`PTAC`, `EF`), else a load's words.
    let tags = cadaddy_mepfix::wordrules::pairs("equipment.room_unit_tags");
    let loads = cadaddy_mepfix::wordrules::groups("equipment.typical_load_words");
    let (kind, named) = tags.iter().find(|(t, _)| has_phrase(&text, t)).map(|(t, k)| (k.to_string(), t.to_string())).or_else(|| {
        loads.iter().find_map(|(value, words)| words.iter().find(|w| has_phrase(&text, w)).map(|w| (value.to_string(), w.to_string())))
    })?;
    let up = text.to_ascii_uppercase();
    let (amps, poles) = amps_poles(&up);
    let has = |words: &[String]| words.iter().any(|w| has_phrase(&text, w));
    Some(EquipmentReceptacle {
        kind,
        named,
        amps,
        poles,
        volts: volts(&up),
        nema: nema(&up),
        horizontal: has(&r.horizontal_words),
        under_unit: has(&r.under_unit_words),
        sub_base: has(&r.sub_base_words),
    })
}

/// A note's characters read at most (hostile text).
const MAX_NOTE_CHARS: usize = 2000;

/// `30A/2P`, `30A-2P`, `20 AMP 2 POLE`, `15A 1P`: amperes and poles (each when printed; poles
/// only beside amperes: a `2P` alone may be anything).
fn amps_poles(up: &str) -> (Option<u32>, Option<u32>) {
    let toks: Vec<&str> = up
        .split(|c: char| c.is_whitespace() || c == '/' || c == ',' || c == '-')
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|t| !t.is_empty())
        .collect();
    // A number with its unit on it (`20A`), or the unit's word next (`20 AMP`).
    let read = |i: usize, attached: &[&str], apart: &[&str]| -> Option<u32> {
        let t = toks.get(i)?;
        let d: String = t.chars().take_while(char::is_ascii_digit).collect();
        if d.is_empty() || d.len() > 3 {
            return None;
        }
        let rest = t.get(d.len()..).unwrap_or("");
        let unit = if rest.is_empty() { toks.get(i + 1).copied().filter(|n| apart.contains(n))? } else { rest };
        (attached.contains(&unit) || apart.contains(&unit)).then(|| d.parse().ok()).flatten()
    };
    let amps = (0..toks.len()).find_map(|i| read(i, &["A"], &["AMP", "AMPS", "AMPERE", "AMPERES"]).filter(|a| (1..=400).contains(a)));
    let poles = (0..toks.len()).find_map(|i| read(i, &["P"], &["POLE", "POLES"]).filter(|p| (1..=3).contains(p)));
    (amps, if amps.is_some() { poles } else { None })
}

/// `208V`, `250 V`: the volts a note prints.
fn volts(up: &str) -> Option<u32> {
    let toks: Vec<&str> = up.split(|c: char| c.is_whitespace() || c == '/' || c == ',').filter(|t| !t.is_empty()).collect();
    toks.iter().enumerate().find_map(|(i, t)| {
        let t = t.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let d: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = t.get(d.len()..).unwrap_or("");
        let unit = if rest.is_empty() { toks.get(i + 1).map(|n| n.trim_matches(|c: char| !c.is_ascii_alphanumeric())).unwrap_or("") } else { rest };
        (d.len() == 3 && matches!(unit, "V" | "VOLT" | "VOLTS")).then(|| d.parse().ok()).flatten()
    })
}

/// `6-20R`, `NEMA 5-20R`: a configuration the note prints.
fn nema(up: &str) -> Option<String> {
    up.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')').find_map(|t| {
        let t = t.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let (a, b) = t.split_once('-')?;
        let n = b.strip_suffix('R')?;
        let a_ok = !a.is_empty() && a.len() <= 2 && a.chars().all(|c| c.is_ascii_digit());
        let n_ok = (2..=3).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit());
        (a_ok && n_ok).then(|| t.to_string())
    })
}

/// The rules for a system a note names (`BDA`); a system the data doesn't know has no parts.
pub(crate) fn system_rules(name: &str) -> SystemRules {
    systems()
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name) || s.also.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .cloned()
        .unwrap_or_else(|| SystemRules { name: name.to_ascii_uppercase(), ..SystemRules::default() })
}

/// Whether `text` holds `phrase` as whole words (`BDA` in `BDA SYSTEM`, not in `ABDAL`).
fn has_phrase(text: &str, phrase: &str) -> bool {
    let norm = |s: &str| {
        let s: String = s.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
        format!(" {} ", s.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let p = norm(phrase);
    !p.trim().is_empty() && norm(text).contains(&p)
}

impl SystemRules {
    /// Whether a text names the system.
    pub fn named_in(&self, text: &str) -> bool {
        !self.name.is_empty() && (has_phrase(text, &self.name) || self.also.iter().any(|a| has_phrase(text, a)))
    }
}

impl PartRule {
    /// Whether a text names the part.
    pub fn named_in(&self, text: &str) -> bool {
        self.words.iter().any(|w| has_phrase(text, w))
    }
}

/// The panel schedules' circuits that feed a system: those whose description names it
/// (`BDA SYSTEM`), with their panel.
pub(crate) fn system_circuits<'a>(sys: &SystemRules, schedules: &'a [PanelSchedule]) -> Vec<(&'a PanelSchedule, &'a PanelCircuit)> {
    schedules.iter().flat_map(|p| p.circuits.iter().map(move |c| (p, c))).filter(|(_, c)| sys.named_in(&c.description)).take(MAX_TAGS).collect()
}

/// Most notes a list holds, lists a sheet holds, tags a sheet holds (hostile files).
const MAX_NOTES: usize = 60;
const MAX_LISTS: usize = 12;
const MAX_TAGS: usize = 2000;
/// How far below its heading a list's notes run, in the heading's text heights.
const LIST_REACH: f64 = 120.0;
/// A note's lines start this close to its number's column, in its text heights (text further
/// right on its line is something else: a title block, a logo's address).
const NOTE_INDENT: f64 = 4.0;

/// A list's subject: its heading's first word (`ELEVATOR` of `ELEVATOR KEYNOTES:`).
pub(crate) fn subject(l: &List) -> String {
    l.heading.split_whitespace().next().unwrap_or_default().trim_end_matches(':').to_ascii_uppercase()
}

/// One numbered note.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Note {
    pub number: String,
    pub text: String,
}

/// A keynote list: its heading, notes, where it is (to tell its own numbers from the plan's
/// tags) and its numbers' text height (a tag's number is printed about as tall).
#[derive(Clone, Debug, Default)]
pub(crate) struct List {
    pub heading: String,
    pub notes: Vec<Note>,
    pub area: [f64; 4],
    pub number_h: f64,
}

/// `3.` / `3. BDA SYSTEM…` → (`3`, the rest).
fn numbered(v: &str) -> Option<(String, String)> {
    let t = v.trim();
    let n: String = t.chars().take_while(char::is_ascii_digit).collect();
    let rest = t.get(n.len()..)?.strip_prefix('.')?;
    (!n.is_empty() && n.len() <= 2 && (rest.is_empty() || rest.starts_with(' ') || rest.starts_with(|c: char| c.is_ascii_uppercase())))
        .then(|| (n, rest.trim().to_string()))
}

/// The keynote lists on a sheet's texts.
pub(crate) fn lists(texts: &[TextItem]) -> Vec<List> {
    lists_under(texts, |u| u.contains("KEYNOTES") || u.contains("KEYED NOTES"))
}

/// The numbered lists under the headings `heading` takes (its upper-case text): a sheet's
/// general notes as well as its keynotes.
pub(crate) fn lists_under(texts: &[TextItem], heading: impl Fn(&str) -> bool) -> Vec<List> {
    let flat: Vec<&TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    let mut out = Vec::new();
    for head in flat.iter().filter(|t| t.value.len() <= 80 && heading(&t.value.to_ascii_uppercase())) {
        if out.len() >= MAX_LISTS {
            break;
        }
        let h = head.height;
        let below = |t: &&&TextItem| t.at.y < head.at.y - 0.3 * h && t.at.y > head.at.y - LIST_REACH * h;
        // The numbers' column: the left-most `N.` under the heading, near its left edge.
        let firsts: Vec<&&TextItem> = flat
            .iter()
            .filter(below)
            .filter(|t| numbered(&t.value).is_some() && t.at.x > head.at.x - 3.0 * h && t.at.x < head.at.x + 10.0 * h)
            .collect();
        let Some(col) = firsts.iter().map(|t| t.at.x).min_by(f64::total_cmp) else { continue };
        let nh = firsts.iter().map(|t| t.height).fold(f64::INFINITY, f64::min);
        if !nh.is_finite() {
            continue;
        }
        let mut lines: Vec<&&TextItem> = flat.iter().filter(below).filter(|t| t.at.x >= col - nh && t.at.x <= col + NOTE_INDENT * nh).collect();
        lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        let mut notes: Vec<Note> = Vec::new();
        let mut last_y = head.at.y;
        let mut y_min = head.at.y;
        for t in lines {
            // A gap of several lines (or another heading) ends the list.
            if !notes.is_empty() && last_y - t.at.y > 4.0 * nh {
                break;
            }
            if t.value.to_ascii_uppercase().contains("NOTES") && t.height > 1.2 * nh {
                break;
            }
            match numbered(&t.value).filter(|_| (t.at.x - col).abs() < 1.5 * nh) {
                Some((n, rest)) => {
                    if notes.len() >= MAX_NOTES {
                        break;
                    }
                    notes.push(Note { number: n, text: rest });
                }
                None => {
                    // The next note's number run into its text (`7.30A/2P …` after note 6).
                    let next = notes.last().and_then(|n| n.number.parse::<u32>().ok()).map(|k| (k + 1).to_string());
                    let run_in = next
                        .as_deref()
                        .filter(|_| (t.at.x - col).abs() < 1.5 * nh)
                        .and_then(|k| t.value.trim().strip_prefix(k)?.strip_prefix('.').filter(|r| r.starts_with(|c: char| c.is_ascii_digit())));
                    if let (Some(k), Some(rest)) = (next.as_deref(), run_in) {
                        if notes.len() >= MAX_NOTES {
                            break;
                        }
                        notes.push(Note { number: k.to_string(), text: rest.trim().to_string() });
                        last_y = t.at.y;
                        y_min = y_min.min(t.at.y);
                        continue;
                    }
                    let Some(n) = notes.last_mut() else { continue };
                    if !n.text.is_empty() {
                        n.text.push(' ');
                    }
                    n.text.push_str(t.value.trim());
                }
            }
            last_y = t.at.y;
            y_min = y_min.min(t.at.y);
        }
        if notes.is_empty() {
            continue;
        }
        let x1 = col + 80.0 * nh;
        out.push(List { heading: head.value.trim().to_string(), notes, area: [col - nh, y_min - nh, x1, head.at.y + h], number_h: nh });
    }
    out
}

/// Where a list's numbers are tagged on its sheet: a text that is one of its numbers, about as
/// tall, outside the list, in a box — four sides of `segs` around it, or one of `boxes` (a
/// keynote symbol the import made a block) holding it — or alone: as tall as the list's numbers,
/// no other text on its line beside it (a box the import didn't keep; a grid bubble's number is
/// taller, a room's has three digits, a circuit's is part of its label).
///
/// Not a number in a small circle (`rounds`: a callout bubble's; a radius below 0 is a symbol's
/// middle, the number right over it its own), nor one in a view whose title
/// names another list's subject (`others`: an `ELEVATOR` view's numbers are the elevator
/// keynotes').
pub(crate) fn tags(
    texts: &[TextItem],
    segs: &[(Vec2, Vec2)],
    boxes: &[[f64; 4]],
    rounds: &[(Vec2, f64)],
    others: &[String],
    list: &List,
) -> Vec<(String, [f64; 2])> {
    let [ax0, ay0, ax1, ay1] = list.area;
    let mut out = Vec::new();
    for t in texts.iter().filter(|t| t.rotation.abs() < 0.05) {
        if out.len() >= MAX_TAGS {
            break;
        }
        let v = t.value.trim();
        let Some(n) = list.notes.iter().find(|n| n.number == v) else { continue };
        if t.height < 0.6 * list.number_h || t.height > 1.6 * list.number_h {
            continue;
        }
        if t.at.x >= ax0 && t.at.x <= ax1 && t.at.y >= ay0 && t.at.y <= ay1 {
            continue;
        }
        let h = t.height;
        // The number's middle (its text anchors at its baseline's start).
        let c = Vec2::new(t.at.x + 0.3 * h * v.len() as f64, t.at.y + 0.5 * h);
        let owned = |(o, r): &(Vec2, f64)| {
            if *r < 0.0 {
                // A symbol's middle right under it: the upper half of a callout bubble.
                (o.x - c.x).abs() < 0.6 * h && c.y - o.y > 0.5 * h && c.y - o.y < 2.2 * h
            } else {
                (*o - c).len() < *r && *r < 4.0 * h
            }
        };
        if rounds.iter().any(owned) {
            continue;
        }
        // A callout's number: a sheet's number (`E-6`, `A501`) right under it.
        let sheet_no = |v: &str| {
            let v = v.trim();
            let letters = v.chars().take_while(char::is_ascii_alphabetic).count();
            (1..=2).contains(&letters)
                && v.len() <= 8
                && v.chars().any(|c| c.is_ascii_digit())
                && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        };
        if texts.iter().any(|o| o.at.y < t.at.y && t.at.y - o.at.y < 3.0 * h && (o.at.x - t.at.x).abs() < 2.0 * h && sheet_no(&o.value)) {
            continue;
        }
        // The view it is in: the nearest title under it (a `… PLAN` larger than its notes).
        let title = view_title(texts, t.at, h);
        if title.is_some_and(|ti| {
            let u = ti.value.to_ascii_uppercase();
            others.iter().any(|w| u.contains(w.as_str()))
        }) {
            continue;
        }
        let in_box = boxes.iter().any(|b| c.x > b[0] && c.x < b[2] && c.y > b[1] && c.y < b[3] && b[2] - b[0] < 5.0 * h && b[3] - b[1] < 5.0 * h);
        let as_tall = (h - list.number_h).abs() <= LONE_HEIGHT * list.number_h;
        // A real text (mostly letters and digits: a dashed line's glyphs read as text aren't)
        // right by it: in its box (an equipment tag's `FCU` over its `4`).
        let crowded = texts.iter().any(|o| {
            let ov = o.value.trim();
            let alnum = ov.chars().filter(char::is_ascii_alphanumeric).count();
            !std::ptr::eq(o, t)
                && alnum > 0
                && 2 * alnum >= ov.chars().count()
                && (o.at.x - t.at.x).abs() < 1.5 * h
                && (o.at.y - t.at.y).abs() < 1.6 * h
        });
        // Beside it on its line, or right above or below it (a callout bubble's `3` over `E-6`):
        // a real text.
        let lone = as_tall
            && !texts.iter().any(|o| {
                let ov = o.value.trim();
                let alnum = ov.chars().filter(char::is_ascii_alphanumeric).count();
                let real = alnum > 0 && 2 * alnum >= ov.chars().count();
                let oh = o.height.min(2.0 * h);
                !std::ptr::eq(o, t)
                    && real
                    && (o.at.y - t.at.y).abs() < 1.8 * h
                    && o.at.x < t.at.x + (0.7 * v.len() as f64 + 1.0) * h
                    && o.at.x + 0.7 * ov.len() as f64 * oh > t.at.x - 1.0 * h
            });
        if lone || (!crowded && (in_box || boxed(c, h, segs))) {
            out.push((n.number.clone(), [c.x, c.y]));
        }
    }
    out
}

/// The sheet's main keynote list: its electrical one, else its first.
pub(crate) fn main_list(lists: &[List]) -> usize {
    lists.iter().position(|l| l.heading.to_ascii_uppercase().contains("ELECTRICAL")).unwrap_or(0)
}

/// The title of the view a point of a sheet is in: the nearest text under it naming a `PLAN`,
/// larger than `h` (its notes' height), within [`VIEW_REACH`] of it.
fn view_title(texts: &[TextItem], at: Vec2, h: f64) -> Option<&TextItem> {
    texts
        .iter()
        .filter(|o| o.at.y < at.y && o.height >= 1.2 * h && (o.at.x - at.x).abs() < VIEW_REACH * h && at.y - o.at.y < VIEW_REACH * h)
        .filter(|o| o.value.to_ascii_uppercase().contains("PLAN"))
        .min_by(|a, b| (at.y - a.at.y).total_cmp(&(at.y - b.at.y)))
}

/// The markers of a sheet's list `li` ([`tags`]) that are its own where the sheet has several
/// lists with the same numbers: the main list's ([`main_list`]) unless its view's title names
/// another list's subject (`ELEVATOR` of an `ELEVATOR PIT … PLAN`); another list's only in a view
/// whose title names its subject.
pub(crate) fn owned_tags(
    texts: &[TextItem],
    segs: &[(Vec2, Vec2)],
    boxes: &[[f64; 4]],
    rounds: &[(Vec2, f64)],
    lists: &[List],
    li: usize,
) -> Vec<(String, [f64; 2])> {
    let Some(list) = lists.get(li) else { return Vec::new() };
    let main = main_list(lists);
    // The other lists' subjects (the main list's is in most views' titles: `… ELECTRICAL PLAN`).
    let side: Vec<String> = lists.iter().enumerate().filter(|(i, _)| *i != main).map(|(_, l)| subject(l)).filter(|w| !w.is_empty()).collect();
    let me = subject(list);
    let title_of =
        |at: &[f64; 2]| view_title(texts, Vec2::new(at[0], at[1]), list.number_h).map(|t| t.value.to_ascii_uppercase()).unwrap_or_default();
    tags(texts, segs, boxes, rounds, &[], list)
        .into_iter()
        .filter(|(_, at)| {
            let title = title_of(at);
            if li == main { !side.iter().any(|w| title.contains(w.as_str())) } else { !me.is_empty() && title.contains(me.as_str()) }
        })
        .collect()
}

/// Whether straight sides close around `c` a box's width away (a text height or two): one above,
/// one below, crossing over it, and one at a side.
fn boxed(c: Vec2, h: f64, segs: &[(Vec2, Vec2)]) -> bool {
    let near = |s: &&(Vec2, Vec2)| (s.0.x - c.x).abs() < 3.0 * h && (s.0.y - c.y).abs() < 3.0 * h;
    let mut sides = [false; 4];
    for (a, b) in segs.iter().filter(near) {
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        let len = dx.hypot(dy);
        if len < 0.5 * h || len > 6.0 * h {
            continue;
        }
        let y = (a.y + b.y) / 2.0;
        let x = (a.x + b.x) / 2.0;
        let crosses_x = a.x.min(b.x) < c.x && a.x.max(b.x) > c.x;
        let crosses_y = a.y.min(b.y) < c.y && a.y.max(b.y) > c.y;
        if dy < 0.1 * len && crosses_x {
            if y > c.y + 0.4 * h && y < c.y + 2.0 * h {
                sides[0] = true;
            } else if y < c.y - 0.4 * h && y > c.y - 2.0 * h {
                sides[1] = true;
            }
        } else if dx < 0.1 * len && crosses_y && (x - c.x).abs() > 0.4 * h && (x - c.x).abs() < 2.5 * h {
            sides[if x < c.x { 2 } else { 3 }] = true;
        }
    }
    sides[0] && sides[1] && (sides[2] || sides[3])
}

/// Most entities of a block a tag's box may be drawn in.
const MAX_BLOCK_ENTITIES: usize = 400;

/// A sheet's straight pieces in `rect` (drawing units): its lines and polylines, and those of
/// the blocks inserted on it (a tag's box is often drawn in a symbol's block).
pub(crate) fn sheet_segments(d: &cadaddy_doc::Drawing, rect: [f64; 4]) -> Vec<(Vec2, Vec2)> {
    use cadaddy_doc::EntityKind;
    let inside = |p: Vec2| p.x >= rect[0] && p.x <= rect[2] && p.y >= rect[1] && p.y <= rect[3];
    let mut out: Vec<(Vec2, Vec2)> = super::gridtable::segments(d).into_iter().filter(|(a, _)| inside(*a)).collect();
    let pieces = |kind: &EntityKind, m: &cadaddy_geom::Mat3, out: &mut Vec<(Vec2, Vec2)>| match kind {
        EntityKind::Line(l) => out.push((m.apply(l.a.xy()), m.apply(l.b.xy()))),
        EntityKind::LwPolyline(p) => {
            let n = p.vertices.len();
            let last = if p.closed { n } else { n.saturating_sub(1) };
            for i in 0..last.min(MAX_BLOCK_ENTITIES) {
                if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1)))
                    && a.bulge.abs() < 1e-9
                {
                    out.push((m.apply(a.p), m.apply(b.p)));
                }
            }
        }
        _ => {}
    };
    for e in d.model.iter() {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        if !inside(ins.insert.xy()) {
            continue;
        }
        let Some(b) = d.block(&ins.block) else { continue };
        if b.entities.len() > MAX_BLOCK_ENTITIES {
            continue;
        }
        let m = ins.transform(b.base.xy());
        for be in b.entities.iter() {
            pieces(&be.kind, &m, &mut out);
        }
    }
    out
}

/// How far a view's title may be from a tag in it (below it), in the tag's text heights.
const VIEW_REACH: f64 = 60.0;
/// A lone tag's number is the list's numbers' height within this share.
const LONE_HEIGHT: f64 = 0.12;

/// How a note's conduit runs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Route {
    /// Between each of the note's boxes, floor to floor.
    Riser,
    /// From each box into the nearest room so named (`CORRIDOR`), above its ceiling.
    Stub(String),
    /// From each box to its system's riser box on its level.
    ToRiser,
    /// Under the slab from what its tag points at (a floor box) to the nearest wall of its room,
    /// stubbed up there.
    UnderSlab,
}

/// What a note asks to be built.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Work {
    /// The system it is part of (`BDA`), from `<NAME> SYSTEM`.
    pub system: String,
    /// Its box: height, width, depth, inches.
    pub box_in: Option<[f64; 3]>,
    /// Its conduit: trade size (inches) and how it runs.
    pub conduits: Vec<(f64, Route)>,
    /// The note puts it in the ground or under the slab (`UNDER SLAB`, `BELOW GRADE`).
    pub buried: bool,
    /// The note gives no conduit size: the smallest trade size is assumed.
    pub size_assumed: bool,
    /// Its box is an antenna's (`… ANTENNA LOCATION`) or the system's riser box (`RISER
    /// JUNCTION BOX`, or a box its conduit rises between).
    pub antenna: bool,
    pub riser: bool,
    /// The conduits (of [`Work::conduits`]) the note gives a pull tape (`WITH PULL TAPE`).
    pub pull_tape: Vec<(f64, Route)>,
    /// The space its conduit to the riser runs in or above (`ABOVE HARD LID PORTION OF
    /// CORRIDOR` → `CORRIDOR`), and whether over a hard (gypsum) ceiling there.
    pub via: Option<String>,
    pub hard_lid: bool,
    /// The fire rating the note keeps where its work passes (`MAINTAIN 2-HOUR FIRE RATING OF
    /// FLOORS AND WALLS`).
    pub rating: Option<Rating>,
}

/// A fire rating a note says to keep: its hours, and whether of floors, of walls (neither said:
/// both).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Rating {
    pub hours: f64,
    pub floors: bool,
    pub walls: bool,
}

impl Rating {
    /// `2-hour`.
    pub fn words(&self) -> String {
        format!("{}-hour", self.hours)
    }
}

impl Work {
    /// Its box's role (`antenna`, `riser`; empty: a plain box).
    pub fn role(&self) -> &'static str {
        if self.antenna {
            "antenna"
        } else if self.riser {
            "riser"
        } else {
            ""
        }
    }

    /// Whether no branch circuit feeds its box (its role is one the rules data's
    /// `unpowered_boxes` names).
    pub fn unpowered(&self) -> bool {
        let role = self.role();
        !role.is_empty() && unpowered_boxes().roles.iter().any(|r| r.eq_ignore_ascii_case(role))
    }

    /// The kind its box is built as: `<system>_antenna_box`, `<system>_riser_box`, else a plain
    /// `junction_box` (a note that names no system, or no role for its box).
    pub fn box_kind(&self) -> String {
        let sys = self.system.to_ascii_lowercase();
        let role = if self.antenna {
            "antenna"
        } else if self.riser {
            "riser"
        } else {
            ""
        };
        if sys.is_empty() || role.is_empty() { "junction_box".to_string() } else { format!("{sys}_{role}_box") }
    }
}

/// The words before the space a run follows (`… PORTION OF CORRIDOR`, `ALONG THE CORRIDOR`).
const VIA_WORDS: [&str; 5] = ["OF", "IN", "ALONG", "THROUGH", "VIA"];

/// `2-HOUR`, `2 HOUR`, `TWO-HOUR`, `2 HR` → 2.
fn hours(sentence: &str) -> Option<f64> {
    let w: Vec<&str> = sentence.split(|c: char| c.is_whitespace() || c == '-').filter(|x| !x.is_empty()).collect();
    let i = w.iter().position(|x| matches!(x.trim_end_matches(['.', ',']), "HOUR" | "HOURS" | "HR" | "HRS"))?;
    let n = w.get(i.checked_sub(1)?)?;
    let v = match *n {
        "ONE" => 1.0,
        "TWO" => 2.0,
        "THREE" => 3.0,
        "FOUR" => 4.0,
        _ => n.trim_start_matches('(').trim_end_matches(')').parse::<f64>().ok()?,
    };
    (v > 0.0 && v <= 4.0).then_some(v)
}

/// The trade size assumed for buried conduit a note doesn't size, inches.
pub(crate) const BURIED_MIN_IN: f64 = 0.75;

/// Whether a text calls for work in the ground or under the slab.
pub(crate) fn buried(text: &str) -> bool {
    let up = text.to_ascii_uppercase();
    cadaddy_mepfix::wordrules::words("keynotes.buried_words").iter().any(|w| up.contains(w))
}

/// `12"X12"X6"` → [12, 12, 6].
fn dims(word: &str) -> Option<[f64; 3]> {
    let parts: Vec<f64> =
        word.split(['X', 'x']).map(|p| p.trim().trim_end_matches(['"', ',', '.', ';']).trim()).filter_map(|p| p.parse::<f64>().ok()).collect();
    match parts.as_slice() {
        [a, b, c] if word.contains('"') && [*a, *b, *c].iter().all(|v| *v > 0.0 && *v <= 96.0) => Some([*a, *b, *c]),
        _ => None,
    }
}

/// Lines a note of buried work is joined from, at most.
const MAX_NOTE_LINES: usize = 12;

/// The notes of a sheet that call for buried work, each whole: its numbered lists' notes (with
/// their numbers: `21`, `PROVIDE UNDER SLAB CONDUIT…`) and the paragraphs outside the lists, each
/// joined from the lines left-aligned with the one naming the work, one under the next (a site
/// plan's callout: `TWO (2) 5" SCH. 40 PVC CONDUIT … MIN. 48" BELOW GRADE …`).
pub(crate) fn buried_notes(texts: &[TextItem]) -> Vec<(Option<String>, String)> {
    notes_matching(texts, buried)
}

/// The notes of a sheet with a line `hit` says yes to, each whole (as [`buried_notes`] joins
/// them): its numbered lists' notes whose text is a hit, and the paragraphs outside the lists
/// joined round each line that is.
pub(crate) fn notes_matching(texts: &[TextItem], hit: impl Fn(&str) -> bool) -> Vec<(Option<String>, String)> {
    let ls = lists(texts);
    let mut out: Vec<(Option<String>, String)> =
        ls.iter().flat_map(|l| &l.notes).filter(|n| hit(&n.text)).map(|n| (Some(n.number.clone()), n.text.clone())).collect();
    let in_list = |p: Vec2| ls.iter().any(|l| p.x >= l.area[0] && p.x <= l.area[2] && p.y >= l.area[1] && p.y <= l.area[3]);
    let mut used = vec![false; texts.len()];
    for (i, t) in texts.iter().enumerate() {
        if used.get(i).copied().unwrap_or(true)
            || !hit(&t.value)
            || in_list(t.at)
            || t.rotation.abs() > 0.01
            || !t.height.is_finite()
            || t.height <= 0.0
        {
            continue;
        }
        let h = t.height;
        // The line next above (`up`) or below a line, left-aligned with it and as tall.
        let step = |cur: Vec2, up: bool, used: &[bool]| {
            texts
                .iter()
                .enumerate()
                .filter(|(j, o)| {
                    let dy = if up { o.at.y - cur.y } else { cur.y - o.at.y };
                    !used.get(*j).copied().unwrap_or(true)
                        && (o.at.x - cur.x).abs() < h
                        && (o.height - h).abs() < 0.25 * h
                        && dy > 0.5 * h
                        && dy < 2.2 * h
                        && o.rotation.abs() <= 0.01
                })
                .min_by(|a, b| (a.1.at.y - cur.y).abs().total_cmp(&(b.1.at.y - cur.y).abs()))
                .map(|(j, _)| j)
        };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        let mut lines: Vec<usize> = vec![i];
        // Up to the paragraph's first line (a numbered line starts one).
        while lines.len() < MAX_NOTE_LINES {
            let Some(&top) = lines.first() else { break };
            let Some(cur) = texts.get(top) else { break };
            if numbered(&cur.value).is_some() {
                break;
            }
            let Some(j) = step(cur.at, true, &used) else { break };
            if texts.get(j).is_some_and(|o| o.value.trim_end().ends_with('.') && numbered(&o.value).is_none()) {
                break;
            }
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
            lines.insert(0, j);
        }
        // Down to its last (the next numbered line starts another).
        while lines.len() < MAX_NOTE_LINES {
            let Some(&bottom) = lines.last() else { break };
            let Some(cur) = texts.get(bottom) else { break };
            let Some(j) = step(cur.at, false, &used) else { break };
            if texts.get(j).is_some_and(|o| numbered(&o.value).is_some()) {
                break;
            }
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
            lines.push(j);
        }
        let text = lines.iter().filter_map(|j| texts.get(*j)).map(|o| o.value.trim()).collect::<Vec<_>>().join(" ");
        if !out.iter().any(|(_, o)| *o == text) {
            out.push((None, text));
        }
    }
    out
}

/// What a note's words ask to be built (none: a note that builds nothing it can read).
pub(crate) fn work(text: &str) -> Option<Work> {
    let up = text.to_ascii_uppercase();
    let words: Vec<&str> = up.split_whitespace().collect();
    let mut w = Work::default();
    if let Some(i) = words.iter().position(|x| x.trim_end_matches(['.', ',']) == "SYSTEM").filter(|i| *i > 0) {
        w.system = words.get(i - 1).map(|s| s.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string()).unwrap_or_default();
    }
    let boxy = cadaddy_mepfix::wordrules::words("keynotes.box_words").iter().any(|b| up.contains(b));
    if boxy {
        w.box_in = words.iter().find_map(|x| dims(x));
    }
    // Each sentence naming conduit: its size (the word before `CONDUIT`) and how it runs.
    for sentence in up.split(". ") {
        let sw: Vec<&str> = sentence.split_whitespace().collect();
        for (i, x) in sw.iter().enumerate() {
            if !x.starts_with("CONDUIT") || i == 0 {
                continue;
            }
            let in_ground = buried(sentence);
            let size = match sw.get(i - 1).and_then(|s| super::gearbuild::trade_in(s.trim_end_matches(','))) {
                Some(v) => v,
                None if in_ground => {
                    w.size_assumed = true;
                    BURIED_MIN_IN
                }
                None => continue,
            };
            let route = if in_ground {
                w.buried = true;
                Route::UnderSlab
            } else if sentence.contains("BETWEEN") {
                Route::Riser
            } else if let Some(j) = sw.iter().position(|y| y.starts_with("STUB")) {
                // `STUBBED INTO CORRIDOR`: the room named after it.
                let room = sw
                    .iter()
                    .skip(j + 1)
                    .find(|y| !matches!(**y, "INTO" | "TO" | "IN" | "THE" | "UP" | "OUT"))
                    .map(|y| y.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string());
                Route::Stub(room.unwrap_or_default())
            } else {
                Route::ToRiser
            };
            if cadaddy_mepfix::wordrules::words("keynotes.pull_words").iter().any(|p| sentence.contains(p))
                && !w.pull_tape.contains(&(size, route.clone()))
            {
                w.pull_tape.push((size, route.clone()));
            }
            if route == Route::ToRiser {
                w.hard_lid |= cadaddy_mepfix::wordrules::words("keynotes.hard_lid_words").iter().any(|p| sentence.contains(p));
                // The space named last after `OF` / `IN` / `ALONG`: `… PORTION OF CORRIDOR`.
                let named = sw
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .filter(|(_, y)| VIA_WORDS.contains(&y.trim_end_matches([',', '.'])))
                    .filter_map(|(j, _)| sw.iter().skip(j + 1).find(|y| !matches!(**y, "THE" | "A" | "AN" | "EACH")))
                    .map(|y| y.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string())
                    .rfind(|y| !y.is_empty() && !y.chars().any(|c| c.is_ascii_digit()));
                if named.is_some() {
                    w.via = named;
                }
            }
            if route == Route::Riser {
                w.riser = true;
            }
            if !w.conduits.contains(&(size, route.clone())) {
                w.conduits.push((size, route));
            }
        }
    }
    // Its box's role: the first sentence's words (`… CEILING MOUNTED ANTENNA LOCATION`, `… RISER
    // JUNCTION BOX`).
    let first = up.split(". ").next().unwrap_or_default();
    w.antenna = first.contains("ANTENNA");
    w.riser |= !w.antenna && first.contains("RISER");
    // The rating it keeps: `MAINTAIN 2-HOUR FIRE RATING OF FLOORS AND WALLS`.
    w.rating = up.split(". ").filter(|s| s.contains("RATING") || s.contains("RATED")).find_map(|s| {
        let hours = hours(s)?;
        let (floors, walls) = (s.contains("FLOOR"), s.contains("WALL"));
        Some(Rating { hours, floors: floors || !walls, walls: walls || !floors })
    });
    (w.box_in.is_some() || !w.conduits.is_empty()).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q6249: a keynote that sets a device's features, ganging, mounting or pathway is read for
    /// the device at its marker, however the note words it; a note over a kind of device in some
    /// rooms gives those rooms; a note that asks none of these is none.
    #[test]
    fn a_keynotes_device_requirements_are_read_however_worded() {
        // Features: USB-C is not also a plain USB; the rules data's order.
        let q = device_requirements("THE COUNTER RECEPTACLE SHALL INCLUDE USB-A AND USB TYPE-C CHARGING PORTS.").unwrap();
        assert_eq!(q.features_for("receptacle"), ["USB", "USB-C"]);
        assert!(q.features_for("switch").is_empty(), "a receptacle's feature is no switch's");
        let q = device_requirements("Desk outlet with USB-C only").unwrap();
        assert_eq!(q.features_for("duplex receptacle"), ["USB-C"]);
        // Ganging, in other words.
        for t in ["SWITCHES TO BE GANGED TOGETHER UNDER ONE PLATE.", "Install these devices in a common box.", "DIMMER AND SWITCH GANGED."] {
            let q = device_requirements(t).unwrap_or_else(|| panic!("{t}"));
            assert!(q.ganged && q.at_marker(), "{t}");
        }
        // Mounting: ceiling for any kind; a floor box makes a receptacle one.
        let q = device_requirements("Ceiling-mounted speaker location.").unwrap();
        assert_eq!(q.mount_for("speaker"), Some(("ceiling".to_string(), String::new())));
        let q = device_requirements("RUN 3/4\" PVC BELOW THE SLAB TO THE FLOOR BOX RECEPTACLE (FLUSH, BRASS COVER).").unwrap();
        assert_eq!(q.mount_for("receptacle"), Some(("floor".to_string(), "floor box".to_string())));
        assert_eq!(q.mount_for("switch"), None, "a floor box is a receptacle's or an outlet's");
        assert_eq!(q.lv_conduit_in, None, "power in the ground, not a low-voltage pathway");
        // A low-voltage conduit's size, the words between the size and CONDUIT skipped.
        for (t, v) in [
            ("PROVIDE 1\" LOW-VOLTAGE CONDUIT WITH PULL STRING FROM THE OUTLET BOX TO THE HALL.", 1.0),
            ("1-1/4\" LOW VOLTAGE CONDUIT FROM WALL PHONE TO ACCESSIBLE CEILING", 1.25),
            ("Data outlet: 3/4\" conduit stubbed above the ceiling.", 0.75),
        ] {
            let q = device_requirements(t).unwrap_or_else(|| panic!("{t}"));
            assert_eq!(q.lv_conduit_in, Some(v), "{t}");
        }
        assert!(lv_kind("data_outlet") && lv_kind("low voltage outlet") && !lv_kind("receptacle") && !lv_kind("metadata box"));
        // A box note's box (a system's antenna box): at the ceiling, fed by no branch circuit.
        let q =
            device_requirements("PAGING SYSTEM CEILING ANTENNA LOCATION. PROVIDE 6\"X6\"X4\" JUNCTION BOX. PROVIDE 3/4\" CONDUIT TO RISER.").unwrap();
        assert_eq!((q.box_role.as_deref(), q.unpowered), (Some("antenna"), true));
        assert_eq!(q.mount_for("junction box").map(|m| m.0), Some("ceiling".to_string()));
        // A note over a kind of device in some rooms: the rooms' words, numbers and joins dropped.
        let q = device_requirements("ALL SUITE, LIBRARY AND FITNESS 125V RECEPTACLES SHALL BE TAMPER RESISTANT.").unwrap();
        assert_eq!(q.scope, Some(("receptacle".to_string(), vec!["SUITE".to_string(), "LIBRARY".to_string(), "FITNESS".to_string()])));
        assert_eq!(q.features_for("receptacle"), ["tamper-resistant"]);
        let q = device_requirements("Every receptacle shall be weather-resistant.").unwrap();
        assert_eq!(
            (q.scope.clone(), q.features_for("receptacle")),
            (Some(("receptacle".to_string(), Vec::new())), vec!["weather-resistant".to_string()])
        );
        // None of these: a reference, an equipment receptacle's rating, a dimension.
        for t in ["REFER TO ARCHITECTURAL ELEVATIONS FOR HEIGHTS.", "30A/2P RECEPTACLE FOR PTAC.", "COORDINATE WITH OWNER PRIOR TO ROUGH-IN."] {
            assert_eq!(device_requirements(t), None, "{t}");
        }
        // Hostile: long text, odd characters.
        assert!(device_requirements(&"ALL \u{0}USB RECEPTACLES GANGED ".repeat(400)).is_some());
    }

    /// Q6249: a sheet with two lists numbered alike (its electrical keynotes and an elevator's):
    /// a marker is the electrical list's unless its view's title names the other's subject.
    #[test]
    fn a_marker_is_its_own_lists_where_two_lists_share_numbers() {
        let t = |v: &str, x: f64, y: f64, h: f64| TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 };
        let texts = vec![
            t("ELECTRICAL KEYNOTES:", 1000.0, 800.0, 4.0),
            t("5.", 1000.0, 790.0, 4.0),
            t("ALL RECEPTACLES SHALL BE TAMPER-RESISTANT.", 1012.0, 790.0, 4.0),
            t("ELEVATOR KEYNOTES:", 1400.0, 800.0, 4.0),
            t("5.", 1400.0, 790.0, 4.0),
            t("PROVIDE CEILING MOUNTED SMOKE DETECTOR IN THE PIT.", 1412.0, 790.0, 4.0),
            // A unit's view and its marker 5; the elevator pit's view and its marker 5.
            t("5", 100.0, 300.0, 4.0),
            t("ENLARGED UNIT PLAN", 80.0, 200.0, 7.0),
            t("5", 600.0, 300.0, 4.0),
            t("ELEVATOR PIT ELECTRICAL PLAN", 580.0, 200.0, 7.0),
        ];
        let lists = lists(&texts);
        assert_eq!(lists.len(), 2);
        assert_eq!(main_list(&lists), 0);
        let at = |li: usize| owned_tags(&texts, &[], &[], &[], &lists, li).into_iter().map(|(_, a)| a[0].round()).collect::<Vec<_>>();
        let (el, ev) = (at(0), at(1));
        assert!(el.len() == 1 && el[0] < 200.0, "the unit's marker is the electrical list's: {el:?}");
        assert!(ev.len() == 1 && ev[0] > 500.0, "the pit's marker is the elevator list's: {ev:?}");
    }

    /// Q6249: a device's tag takes what its keynotes ask of its kind, each with the keynote as
    /// its source; a gang note needs the key its marker gave; an antenna box's circuit read by
    /// its symbol is taken off, the read value kept beside with why.
    #[test]
    fn a_devices_tag_takes_its_keynotes_requirements() {
        use cadaddy_walk3d::tags::ElementTag;
        let mut rec = ElementTag { id: "device:L1:1,1".into(), kind: "receptacle".into(), ..Default::default() };
        rec.set("device_type", "duplex receptacle", "plan");
        let cites = ["Q-9 keynote 3: DESK RECEPTACLE WITH USB AND USB-C.".to_string(), "Q-9 keynote 5: DEVICES GANGED TOGETHER.".to_string()];
        let got = apply_to_tag(&mut rec, "receptacle", &cites, "");
        assert_eq!((got.features.clone(), got.ganged), (vec!["USB".to_string(), "USB-C".to_string()], false), "no key: no gang");
        let f = |t: &ElementTag, k: &str| t.fields.get(k).map(|f| (f.value.clone(), f.source.clone()));
        assert_eq!(f(&rec, "keynote_features"), Some(("USB, USB-C".into(), "note".into())));
        assert_eq!(f(&rec, "device_type"), Some(("duplex receptacle, USB, USB-C".into(), "note".into())));
        assert_eq!(f(&rec, "keynotes").map(|v| v.0), Some(cites[0].clone()), "only the note it took");
        assert_eq!(f(&rec, "keynote").map(|v| v.0), Some(cites[0].clone()));
        let mut sw = ElementTag { kind: "switch".into(), ..Default::default() };
        assert!(apply_to_tag(&mut sw, "switch", &cites, "Q-9 keynote 5 at 1,2").ganged);
        assert_eq!(f(&sw, "gang_with").map(|v| v.0), Some("Q-9 keynote 5 at 1,2".to_string()));
        assert_eq!(f(&sw, "keynote_features"), None, "a receptacle's features are no switch's");
        // A floor box; a ceiling antenna box on a circuit it can't take.
        let mut fb = ElementTag { kind: "receptacle".into(), ..Default::default() };
        apply_to_tag(&mut fb, "receptacle", &["Q-9 keynote 7: FLOOR BOX RECEPTACLE.".to_string()], "");
        assert_eq!((f(&fb, "mount").map(|v| v.0), f(&fb, "device_type").map(|v| v.0)), (Some("floor".into()), Some("floor box".into())));
        let mut jb = ElementTag { kind: "junction_box".into(), ..Default::default() };
        jb.set("circuit", "Q-3", "plan");
        let got = apply_to_tag(
            &mut jb,
            "junction box",
            &["Q-9 keynote 2: CEILING MOUNTED ANTENNA LOCATION. PROVIDE 8\"X8\"X6\" JUNCTION BOX.".to_string()],
            "",
        );
        assert_eq!((got.mount.as_deref(), got.uncircuited.as_deref()), (Some("ceiling"), Some("Q-3")));
        assert_eq!((f(&jb, "circuit"), f(&jb, "circuit_read").map(|v| v.0)), (None, Some("Q-3".into())));
        assert!(f(&jb, "correction").is_some_and(|c| c.0.contains("antenna box") && c.0.contains("no branch circuit")), "{:?}", f(&jb, "correction"));
        // A low-voltage outlet's conduit; a receptacle doesn't take it.
        let lv = ["Q-9 keynote 8: PROVIDE 1\" LOW-VOLTAGE CONDUIT FROM THE DATA BOX TO THE HALL.".to_string()];
        let mut d = ElementTag { kind: "data_outlet".into(), ..Default::default() };
        assert_eq!(apply_to_tag(&mut d, "data outlet", &lv, "").lv_conduit_in, Some(1.0));
        assert_eq!(f(&d, "lv_conduit_in").map(|v| v.0), Some("1".to_string()));
        let mut r2 = ElementTag { kind: "receptacle".into(), ..Default::default() };
        assert_eq!(apply_to_tag(&mut r2, "receptacle", &lv, ""), Applied::default());
        assert!(r2.fields.is_empty(), "a note it doesn't take leaves it as it was");
    }

    /// Q6247: a keynote calling for a unit's receptacle gives its rating, poles, orientation and
    /// place, the unit's kind from the rules data; a note with no receptacle or no unit the data
    /// knows is none.
    #[test]
    fn an_equipment_receptacles_keynote_gives_its_rating_place_and_unit() {
        let r = equipment_receptacle("PROVIDE 30A/2P RECEPTACLE FOR PTAC. SET IT HORIZONTALLY UNDER THE UNIT, IN ITS SUB BASE.").unwrap();
        assert_eq!((r.kind.as_str(), r.amps, r.poles, r.horizontal, r.under_unit, r.sub_base), ("ptac", Some(30), Some(2), true, true, true));
        assert_eq!((r.configuration().as_deref(), r.voltage()), (Some("6-30R"), Some(250)));
        assert_eq!(r.rating().as_deref(), Some("30 A, 250 V, 2-pole (NEMA 6-30R)"));
        assert_eq!(r.describe("PTAC-2"), "receptacle 30 A, 250 V, 2-pole (NEMA 6-30R), horizontal, under the unit in its sub-base, for PTAC-2");
        // A load's words, one pole spelt out, the volts and configuration printed.
        let r = equipment_receptacle("20 AMP 1 POLE RECEPTACLE (NEMA 5-20R) 120V FOR BATH FAN ABOVE CEILING").unwrap();
        assert_eq!((r.kind.as_str(), r.amps, r.poles, r.volts, r.under_unit), ("exhaust_fan", Some(20), Some(1), Some(120), false));
        assert_eq!((r.configuration().as_deref(), r.voltage()), (Some("5-20R"), Some(120)));
        // No rating: the unit's receptacle all the same, its rating the documents' elsewhere.
        let r = equipment_receptacle("RECEPTACLE FOR EF-3").unwrap();
        assert_eq!((r.kind.as_str(), r.rating()), ("exhaust_fan", None));
        // Not one: no receptacle; a rule over all receptacles; a tag's letters inside a word; no
        // unit the data knows.
        for t in [
            "PROVIDE CONDENSATE PIPING FOR PTAC",
            "ALL 120V RECEPTACLES SHALL BE TAMPER-RESISTANT",
            "RECEPTACLE, REFER TO DETAIL 3",
            "QUAD RECEPTACLE FOR TV",
        ] {
            assert!(equipment_receptacle(t).is_none(), "{t}");
        }
        // Poles alone are no rating; a note's article is no ampere.
        assert_eq!(amps_poles("SEE 2P DETAIL"), (None, None));
        assert_eq!(amps_poles("PROVIDE 1 A RECEPTACLE"), (None, None));
        // Hostile text: long, odd characters.
        assert!(equipment_receptacle(&"RECEPTACLE FOR PTAC 9999A/9P ".repeat(500)).is_some());
    }

    /// Q6247: a note's number printed against its text (`7.30A/2P …` under note 6) starts the
    /// next note; a line that isn't the next number stays the note's.
    #[test]
    fn a_notes_number_run_into_its_text_starts_the_next_note() {
        let texts = vec![
            t(0.0, 100.0, "ELECTRICAL KEYNOTES:", 4.0),
            t(0.0, 94.0, "1.", 3.0),
            t(5.0, 94.0, "PROVIDE A BLANK PLATE AT EACH SPARE BOX.", 3.0),
            t(0.0, 90.0, "2.30A/2P RECEPTACLE FOR PTAC BY ITS SLEEVE.", 3.0),
            t(5.0, 86.0, "SET IT HORIZONTALLY BELOW THE UNIT.", 3.0),
            t(0.0, 82.0, "4.5 FT ABOVE THE FLOOR", 3.0),
        ];
        let l = lists(&texts);
        let notes: Vec<(&str, &str)> = l[0].notes.iter().map(|n| (n.number.as_str(), n.text.as_str())).collect();
        assert_eq!(
            notes,
            [
                ("1", "PROVIDE A BLANK PLATE AT EACH SPARE BOX."),
                ("2", "30A/2P RECEPTACLE FOR PTAC BY ITS SLEEVE. SET IT HORIZONTALLY BELOW THE UNIT. 4.5 FT ABOVE THE FLOOR")
            ]
        );
        assert!(equipment_receptacle(&l[0].notes[1].text).is_some() && equipment_receptacle(&l[0].notes[0].text).is_none());
    }

    fn t(x: f64, y: f64, v: &str, h: f64) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    /// A sheet's keynotes (as E-3 prints them, its lines one text each) and the plan's tags.
    fn sheet() -> (Vec<TextItem>, Vec<(Vec2, Vec2)>) {
        let mut v = vec![
            t(1000.0, 1000.0, "ELECTRICAL KEYNOTES: (     ,     ,     )", 15.0),
            t(1000.0, 970.0, "3.", 9.0),
            t(1020.0, 970.0, "BDA SYSTEM CEILING MOUNTED ANTENNA LOCATION.", 9.0),
            t(1020.0, 958.0, "PROVIDE 8\"X8\"X6\" JUNCTION BOX AT LOCATION AS DIRECTED", 9.0),
            t(1020.0, 946.0, "BY THE BDA SYSTEM VENDOR. PROVIDE 1\" CONDUIT WITH", 9.0),
            t(1020.0, 934.0, "PULL TAPE ABOVE HARD LID PORTION OF CORRIDOR.", 9.0),
            t(1000.0, 922.0, "6.", 9.0),
            t(1020.0, 922.0, "PROVIDE POWER TO FIRE SMOKE DAMPER.", 9.0),
            t(1000.0, 910.0, "7.", 9.0),
            t(1020.0, 910.0, "BDA SYSTEM RISER JUNCTION BOX. JUNCTION BOX SHALL BE", 9.0),
            t(1020.0, 898.0, "12\"X12\"X6\". CONFIRM ALL REQUIREMENTS WITH BDA SYSTEM", 9.0),
            t(1020.0, 886.0, "VENDOR. PROVIDE 2\" CONDUIT BETWEEN EACH RISER", 9.0),
            t(1020.0, 874.0, "JUNCTION BOX. PROVIDE 1\" CONDUIT STUBBED INTO", 9.0),
            t(1020.0, 862.0, "CORRIDOR ABOVE CEILING.", 9.0),
            t(1000.0, 840.0, "8.", 9.0),
            t(1020.0, 840.0, "CIRCUIT TO FACTORY MOUNTED DISCONNECT SWITCH.", 9.0),
            // Beside the list on its lines: a title block's text.
            t(1700.0, 958.0, "300 Sample Street N, Ste 100", 9.0),
            // On the plan: 7 in a box, a 7 that is a circuit number (no box), 3 in a symbol box.
            t(200.0, 200.0, "7", 9.0),
            t(400.0, 200.0, "7", 9.0),
            t(410.0, 200.0, "A-4", 9.0),
            // A lone 8 as tall as the list's numbers (its box not kept); a taller 8 (a grid bubble's).
            t(900.0, 100.0, "8", 9.0),
            t(950.0, 100.0, "8", 14.0),
            // An equipment tag: FCU over 7 in a hexagon (its box test below).
            t(1500.0, 312.0, "FCU", 9.0),
            t(1503.0, 300.0, "7", 9.0),
            // A callout bubble's 3 (in its circle) over its sheet, 2.2 heights down.
            t(1200.0, 100.0, "3", 9.0),
            t(1196.0, 80.0, "E-6", 9.0),
            // A 6 in an elevator view (another list's subject), a 6 in a plan.
            t(2000.0, 500.0, "6", 9.0),
            t(1990.0, 400.0, "ELEVATOR SHAFT ELECTRICAL PLAN", 12.0),
            t(3000.0, 500.0, "6", 9.0),
            t(2990.0, 400.0, "ELECTRICAL - 2ND FLOOR PLAN", 12.0),
            // Dashed line glyphs read as text on the lone 8's line.
            t(700.0, 101.0, "'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'", 9.0),
            t(600.0, 300.0, "3", 9.0),
        ];
        v.push(t(800.0, 800.0, "SOME OTHER NOTE", 9.0));
        // The box around the first 7 (its middle at 202.7, 204.5).
        let (x0, y0, x1, y1) = (194.0, 195.0, 212.0, 214.0);
        let segs = vec![
            (Vec2::new(x0, y1), Vec2::new(x1, y1)),
            (Vec2::new(x0, y0), Vec2::new(x1, y0)),
            (Vec2::new(x0, y0), Vec2::new(x0, y1)),
            (Vec2::new(x1, y0), Vec2::new(x1, y1)),
            // The hexagon around FCU / 7: flat top and bottom, slanted sides.
            (Vec2::new(1495.0, 325.0), Vec2::new(1520.0, 325.0)),
            (Vec2::new(1495.0, 296.0), Vec2::new(1520.0, 296.0)),
            (Vec2::new(1495.0, 325.0), Vec2::new(1488.0, 310.0)),
            (Vec2::new(1488.0, 310.0), Vec2::new(1495.0, 296.0)),
            (Vec2::new(1520.0, 325.0), Vec2::new(1527.0, 310.0)),
            (Vec2::new(1527.0, 310.0), Vec2::new(1520.0, 296.0)),
        ];
        (v, segs)
    }

    #[test]
    fn a_keynote_list_gives_its_numbered_notes_whole() {
        let (texts, _) = sheet();
        let l = lists(&texts);
        assert_eq!(l.len(), 1);
        let n: Vec<&str> = l[0].notes.iter().map(|n| n.number.as_str()).collect();
        assert_eq!(n, ["3", "6", "7", "8"]);
        assert!(l[0].notes[2].text.ends_with("CORRIDOR ABOVE CEILING."), "{:?}", l[0].notes[1]);
        assert!(!l[0].notes[0].text.contains("Street"), "a title block's text beside it is no note's");
    }

    #[test]
    fn a_tag_is_its_number_in_a_box_and_a_bare_number_is_not() {
        let (texts, segs) = sheet();
        let l = lists(&texts).remove(0);
        let boxes = [[596.0, 295.0, 614.0, 313.0]];
        let got = tags(&texts, &segs, &boxes, &[], &["ELEVATOR".to_string()], &l);
        let mut numbers: Vec<(&str, i64)> = got.iter().map(|(n, at)| (n.as_str(), at[0] as i64)).collect();
        numbers.sort();
        assert_eq!(numbers, [("3", 602), ("6", 3002), ("7", 202), ("8", 902)], "{got:?}");
    }

    #[test]
    fn a_notes_words_say_its_box_and_its_conduit() {
        let (texts, _) = sheet();
        let l = lists(&texts).remove(0);
        let riser = work(&l.notes[2].text).expect("riser");
        assert_eq!(riser.system, "BDA");
        assert_eq!(riser.box_in, Some([12.0, 12.0, 6.0]));
        assert_eq!(riser.conduits, [(2.0, Route::Riser), (1.0, Route::Stub("CORRIDOR".into()))]);
        let antenna = work(&l.notes[0].text).expect("antenna");
        assert_eq!((antenna.box_in, antenna.conduits.as_slice()), (Some([8.0, 8.0, 6.0]), [(1.0, Route::ToRiser)].as_slice()));
        assert_eq!(work(&l.notes[3].text), None, "a note that builds nothing it can read");
        assert!(!riser.buried);
        // Conduit under the slab, its size not given: the smallest trade size, assumed.
        let floor_box = work("PROVIDE UNDER SLAB CONDUIT FOR FLOOR BOX RECEPTACLE (HUBBLE OR EQUAL). VERIFY PARTS.").expect("floor box");
        assert_eq!(floor_box.conduits, [(BURIED_MIN_IN, Route::UnderSlab)]);
        assert!(floor_box.buried && floor_box.size_assumed);
        let sized = work("PROVIDE 1\" CONDUIT BELOW GRADE TO THE SIGN.").expect("sized");
        assert_eq!((sized.conduits.as_slice(), sized.size_assumed), ([(1.0, Route::UnderSlab)].as_slice(), false));
        assert!(buried("MIN. 48\" BELOW GRADE") && !buried("MICROWAVE BELOW"));
    }

    #[test]
    fn a_notes_words_say_its_box_kind_pull_tape_route_and_rating() {
        // The list's own wording: pull tape above the corridor's hard lid, the wall's rating kept.
        let (texts, _) = sheet();
        let l = lists(&texts).remove(0);
        let antenna = work(&l.notes[0].text).expect("antenna");
        assert!(antenna.antenna && !antenna.riser && antenna.hard_lid, "{antenna:?}");
        assert_eq!((antenna.box_kind().as_str(), antenna.via.as_deref()), ("bda_antenna_box", Some("CORRIDOR")));
        assert_eq!(antenna.pull_tape, [(1.0, Route::ToRiser)]);
        let riser = work(&l.notes[2].text).expect("riser");
        assert_eq!((riser.box_kind().as_str(), riser.pull_tape.len(), riser.rating.clone()), ("bda_riser_box", 0, None));
        // Other wordings: a pull string along a gypsum-lidded corridor, a 1-hour wall rating
        // in words; a riser box of another system, its floors 2 HR; a plain box, no system.
        let a = work("BDA SYSTEM CEILING ANTENNA. PROVIDE 6\"X6\"X4\" JUNCTION BOX WHERE THE SYSTEM VENDOR DIRECTS. RUN 3/4\" CONDUIT WITH PULL STRING ABOVE THE GYPSUM CEILING ALONG THE CORRIDOR. KEEP THE ONE-HOUR RATING OF WALLS.").expect("a");
        assert_eq!(
            (a.box_in, a.pull_tape.as_slice(), a.via.as_deref(), a.hard_lid),
            (Some([6.0, 6.0, 4.0]), [(0.75, Route::ToRiser)].as_slice(), Some("CORRIDOR"), true)
        );
        assert_eq!(a.rating, Some(Rating { hours: 1.0, floors: false, walls: true }));
        let r =
            work("DAS SYSTEM RISER JUNCTION BOX 10\"X10\"X4\". PROVIDE 1-1/4\" CONDUIT BETWEEN EACH RISER BOX. MAINTAIN 2 HR FIRE RATING OF FLOORS.")
                .expect("r");
        assert_eq!((r.system.as_str(), r.box_kind().as_str(), r.conduits.as_slice()), ("DAS", "das_riser_box", [(1.25, Route::Riser)].as_slice()));
        assert_eq!(r.rating, Some(Rating { hours: 2.0, floors: true, walls: false }));
        let plain = work("PROVIDE 4\"X4\"X2\" JUNCTION BOX FOR THE DOOR OPERATOR. KEEP THE (2) HOUR RATED ASSEMBLY.").expect("plain");
        assert_eq!((plain.box_kind().as_str(), plain.rating.clone()), ("junction_box", Some(Rating { hours: 2.0, floors: true, walls: true })));
        assert_eq!(Rating { hours: 2.0, floors: true, walls: true }.words(), "2-hour");
    }

    #[test]
    fn a_systems_power_and_parts_are_named_by_whole_words() {
        let bda = system_rules("bda");
        let parts: Vec<&str> = bda.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(parts, ["head_end", "donor_antenna"]);
        assert!(!bda.source.is_empty(), "rules data cite their source");
        assert!(bda.named_in("BDA SYSTEM") && bda.named_in("ERRCS PANEL") && !bda.named_in("ABDAL PUMP"));
        assert!(bda.parts[1].named_in("RUN IT TO THE ROOF.") && !bda.parts[1].named_in("ROOFTOP UNIT"));
        let c = |n: &str, d: &str| PanelCircuit { number: n.into(), description: d.into(), ..Default::default() };
        let schedules = vec![
            PanelSchedule { panel: "L1".into(), circuits: vec![c("1", "ABDAL PUMP"), c("3", "RCPT - LOBBY")], ..Default::default() },
            PanelSchedule { panel: "H2".into(), circuits: vec![c("12", "ERRCS / BDA HEAD-END")], ..Default::default() },
        ];
        let fed: Vec<(&str, &str)> = system_circuits(&bda, &schedules).into_iter().map(|(p, c)| (p.panel.as_str(), c.number.as_str())).collect();
        assert_eq!(fed, [("H2", "12")]);
        // A system the rules data doesn't know: its name, no parts to look for.
        let other = system_rules("Paging");
        assert_eq!((other.name.as_str(), other.parts.len()), ("PAGING", 0));
    }

    #[test]
    fn a_sheets_buried_work_notes_are_read_whole() {
        // A site plan's callout, its lines one text each, beside another callout; nothing buried in that one.
        let texts = vec![
            t(100.0, 500.0, "TWO (2) 5\" SCH. 40 PVC CONDUIT", 6.0),
            t(100.0, 492.0, "WITH STEEL ELLS AND RISERS,", 6.0),
            t(100.0, 484.0, "MIN. 48\" BELOW GRADE WITH", 6.0),
            t(100.0, 476.0, "PULLWIRE FOR PRIMARY", 6.0),
            t(100.0, 468.0, "CONDUCTORS.", 6.0),
            t(300.0, 500.0, "UTILITY CO. PAD MOUNTED", 6.0),
            t(300.0, 492.0, "TRANSFORMER.", 6.0),
        ];
        let notes = buried_notes(&texts);
        assert_eq!(
            notes,
            vec![(
                None,
                "TWO (2) 5\" SCH. 40 PVC CONDUIT WITH STEEL ELLS AND RISERS, MIN. 48\" BELOW GRADE WITH PULLWIRE FOR PRIMARY CONDUCTORS.".to_string()
            )]
        );
    }
}
