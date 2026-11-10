//! Fire alarm wiring (Q6503): the fire alarm system's circuits routed through the model, the way
//! the field finishes a system the drawings show as devices only.
//!
//! - **Equipment** ([`place_equipment`], run by `model.build` once the devices stand, and by the
//!   reroute): the control panel where no document places one, and the NAC power supplies each
//!   storey's notification load needs (the control panel's own NACs serve its storey when they
//!   carry it), each in a service room nearest the riser over the panel. Each is inferred, with
//!   its basis; the documents' own panels and power supplies are kept and counted first.
//! - **Notification appliance circuits**: each NAC chained appliance to appliance from its power
//!   supply (or the panel). The shop drawings' circuits and addresses ( `nac_circuit`,
//!   `address`) give the circuits and their order; where the documents give none, each storey's
//!   appliances go to their nearest source and are chained nearest neighbour, a circuit closed
//!   where its load passes the output's rating or its voltage drop the appliances' minimum
//!   (inferred, `fawiring.json`). A circuit whose drop #14 can't carry is #12, with the reason.
//! - **Signaling line circuits**: each SLC loop from the panel through the initiating devices
//!   and modules, storey by storey up the riser (by the documents' loops and addresses, else
//!   nearest neighbour), a loop closed at its device capacity; the annunciator's bus and each
//!   power supply's start circuit from the panel.
//! - **Pieces**: each circuit's legs go into the spec as [`NAC_SYSTEM`] / [`SLC_SYSTEM`] runs,
//!   one tag per circuit ([`TAG_KIND`]); the build cuts each leg into its cable from the element
//!   it leaves to the one it reaches (and the raceway the documents call for below a height), so
//!   `bim.chain` walks device → circuit → power supply → panel and the takeoff counts the cable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Drawing, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, Made, MepEquipment, MepRun};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{GearEl, ROUTED, from_box, manhattan, mesh_box, penetrate, r1, r2, rise_point, route_leg, run_ft, same_level, to_box};
use crate::cmd::scope;

/// The systems the fire alarm circuits go into the spec as.
pub(crate) const NAC_SYSTEM: &str = "FA-NAC";
pub(crate) const SLC_SYSTEM: &str = "FA-SLC";
/// The kind of the spec tag each fire alarm circuit carries (its source, cable, members).
pub(crate) const TAG_KIND: &str = "fa_circuit";
/// The id prefix of those tags and their runs (`fa:<circuit>#<leg>.<piece>`).
pub(crate) const TAG_PREFIX: &str = "fa:";
/// What the job read of the documents' fire alarm wiring statements (specifications,
/// submittals, sheets), among the drawing's embedded files.
pub(crate) const DOCS_FILE: &str = "route/fawiring.json";
/// The property marking equipment this step placed (rebuilt from what stands each time).
const PLACED_BY: &str = "PLACED_BY";
/// Its value.
const PLACED_BY_US: &str = "fire alarm wiring rule";
/// The trade practice rules each piece's provenance cites.
pub(crate) const RULE_NAC: &str = "FA-NAC-CHAIN";
pub(crate) const RULE_SLC: &str = "FA-SLC-LOOP";
pub(crate) const RULE_SUPPLY: &str = "FA-POWER-SUPPLY";
pub(crate) const RULE_PANEL: &str = "FA-CONTROL-PANEL";

const RULES: &str = include_str!("fawiring.json");
/// Most devices, circuits, legs and report rows handled (hostile models).
const MAX_DEVICES: usize = 50_000;
const MAX_CIRCUITS: usize = 5_000;
const MAX_LEGS: usize = 100_000;
const MAX_ROWS: usize = 2_000;
const MAX_PLACED: usize = 200;

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Cable {
    #[serde(rename = "type")]
    pub ty: String,
    pub size: String,
    pub awg: u32,
    pub item: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Cables {
    pub nac: Cable,
    pub slc: Cable,
    pub annunciator: Cable,
    pub trigger: Cable,
    pub body_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct NacRules {
    pub circuit_amps: f64,
    pub use_fraction: f64,
    pub supply_volts: f64,
    pub end_volts_min: f64,
    pub max_devices: usize,
    pub class: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SlcRules {
    pub devices_per_loop: usize,
    pub max_ft: f64,
    pub class: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Placement {
    pub nac_circuits: usize,
    pub kind: String,
    pub tag: String,
    pub title: String,
    pub size_in: [f64; 3],
    pub top_in: f64,
    /// Rooms it goes in, by their names' words, most wanted first.
    pub room_words: Vec<Vec<String>>,
    /// Rooms it never goes in, by their names' words.
    pub never_words: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DocWords {
    pub require_words: Vec<String>,
    pub class_b_words: Vec<String>,
    /// A wire legend's circuit names, by the circuit they are (`slc`, `nac`, `annunciator`).
    pub legend: BTreeMap<String, Vec<String>>,
    pub legend_source: String,
    pub raceway_words: Vec<String>,
    pub wiring_words: Vec<String>,
    pub subject_words: Vec<String>,
    pub below_words: Vec<String>,
    pub not_words: Vec<String>,
    pub plenum_words: Vec<String>,
    pub class_a_words: Vec<String>,
    pub raceway: String,
    pub raceway_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FaRules {
    pub notification_kinds: Vec<String>,
    pub notification_source: String,
    pub equipment_words: Vec<String>,
    pub candela_amps: Vec<[f64; 2]>,
    pub candela_default: f64,
    pub horn_amps: f64,
    pub amps_source: String,
    pub nac: NacRules,
    pub slc: SlcRules,
    pub cables: Cables,
    pub ohms_per_kft: BTreeMap<String, f64>,
    pub ohms_source: String,
    pub power_supply: Placement,
    pub control_panel: Placement,
    pub documents: DocWords,
    pub riser_source: String,
}

pub(crate) fn rules() -> &'static FaRules {
    static R: OnceLock<FaRules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

impl FaRules {
    /// What one circuit output carries, amperes (its rating at the rules' use).
    fn circuit_cap(&self) -> f64 {
        (self.nac.circuit_amps * self.nac.use_fraction.clamp(0.1, 1.0)).max(0.1)
    }

    /// The voltage a NAC may lose to its last appliance.
    fn drop_allowed(&self) -> f64 {
        (self.nac.supply_volts - self.nac.end_volts_min).max(0.1)
    }

    /// Ohms per foot of one conductor of `awg`.
    fn ohms_ft(&self, awg: u32) -> f64 {
        self.ohms_per_kft.get(&awg.to_string()).copied().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(3.07) / 1000.0
    }

    /// The drop of `amps` lumped at the end of `ft` of two conductors of `awg`, volts.
    fn drop(&self, amps: f64, ft: f64, awg: u32) -> f64 {
        amps * 2.0 * ft * self.ohms_ft(awg)
    }

    /// An appliance's current by its kind and candela, and why.
    fn amps_of(&self, fa_kind: &str, candela: Option<f64>) -> (f64, String) {
        let k = fa_kind.to_ascii_lowercase();
        if !k.contains("strobe") && (k.contains("horn") || k.contains("chime") || k.contains("bell") || k.contains("sounder")) {
            return (self.horn_amps, format!("{}: an audible appliance", self.amps_source));
        }
        let cd = candela.filter(|c| c.is_finite() && *c > 0.0).unwrap_or(self.candela_default);
        let row = self.candela_amps.iter().find(|r| r[0] + 1e-9 >= cd).or(self.candela_amps.last());
        let a = row.map_or(0.2, |r| r[1]).max(0.0);
        let said = if candela.is_some() { format!("{cd} cd") } else { format!("no candela read: taken at {cd} cd") };
        (a, format!("{said}: {}", self.amps_source))
    }
}

// ─── Words ───────────────────────────────────────────────────────────────────

/// Words as whole words: upper case, anything but letters and digits a space, a space each side.
fn spaced(s: &str) -> String {
    let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

fn has_any(hay_spaced: &str, words: &[String]) -> bool {
    words.iter().map(|w| spaced(w)).any(|w| !w.trim().is_empty() && hay_spaced.contains(&w))
}

/// The numbers in a text (`15/75cd`: 15 and 75).
fn numbers(t: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in t.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty() && !cur.contains('.')) {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(v) = cur.trim_end_matches('.').parse::<f64>() {
                out.push(v);
            }
            cur.clear();
        }
        if out.len() >= 16 {
            break;
        }
    }
    out
}

/// A device address's number on its circuit (`BP7.N1.01`: 1, `L7.42`: 42).
fn address_number(addr: &str) -> Option<u32> {
    let last = addr.trim().rsplit(['.', '-', ':', ' ']).find(|p| !p.is_empty())?;
    let digits: String = last.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 6 { None } else { digits.parse().ok() }
}

// ─── What each element is ────────────────────────────────────────────────────

/// What a fire alarm element is on the wiring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Panel,
    Supply,
    Annunciator,
    Notification,
    Initiating,
}

impl Role {
    fn of_kind(fa_kind: &str) -> Role {
        let k = fa_kind.trim().to_ascii_lowercase();
        let fr = rules();
        match k.as_str() {
            "control panel" => Role::Panel,
            "power supply" => Role::Supply,
            "annunciator" => Role::Annunciator,
            _ if fr.notification_kinds.iter().any(|n| n.eq_ignore_ascii_case(&k)) => Role::Notification,
            _ => Role::Initiating,
        }
    }
}

/// A fire alarm device as the wiring takes it.
#[derive(Clone, Debug)]
pub(crate) struct FaDevice {
    pub handle: Handle,
    pub level: String,
    /// Its plan point (its storey's stacking taken off).
    pub at: Vec2,
    pub fa_kind: String,
    pub role: Role,
    pub amps: f64,
    pub amps_basis: String,
    /// The circuit, loop and address the documents give it (the shop drawings').
    pub nac: String,
    pub slc: String,
    pub address: String,
}

/// The spec's fire alarm equipment's role (its kind, title, tag, `FA_KIND`): none when it is not
/// fire alarm equipment.
pub(crate) fn equipment_role(e: &MepEquipment) -> Option<Role> {
    let fa_prop = e.properties.get("FA_KIND").map(String::as_str).unwrap_or_default();
    let words = format!("{} {} {} {}", e.kind.replace('_', " "), e.title, e.tag, fa_prop);
    let sr = scope::rules();
    let fa = !fa_prop.trim().is_empty()
        || sr.class_of(&e.kind, &words).is_some_and(|c| c.party == "fire alarm")
        || has_any(&spaced(&words), &rules().equipment_words);
    if !fa {
        return None;
    }
    let kind = Some(fa_prop.trim().to_string()).filter(|k| !k.is_empty()).or_else(|| sr.fa_kind_of(&words));
    Some(match kind {
        Some(k) => Role::of_kind(&k),
        None if spaced(&words).contains(" PANEL ") => Role::Panel,
        None => Role::Initiating,
    })
}

/// The built model's fire alarm devices: each device element of the fire alarm class (by its
/// kind, its legend row, its spec tag's fields), with its kind, current and documented circuit.
pub(crate) fn devices(d: &Drawing, sp: &BuildingSpec) -> Vec<FaDevice> {
    let fr = rules();
    let sr = scope::rules();
    let by_id: HashMap<&str, &ElementTag> = sp.tags.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("device")) {
        if out.len() >= MAX_DEVICES {
            break;
        }
        let ps = props::read(&e.common.xdata);
        if ps.get("misplaced").is_some() {
            continue;
        }
        let tag = super::super::bimlook::made_tag(e, "devid").and_then(|id| by_id.get(id).copied());
        let g = |k: &str| {
            ps.get(k)
                .map(Prop::text)
                .filter(|v| !v.trim().is_empty())
                .or_else(|| tag.and_then(|t| t.fields.get(k)).map(|f| f.value.clone()))
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let kind = Some(g("kind")).filter(|k| !k.is_empty()).or_else(|| tag.map(|t| t.kind.clone())).unwrap_or_default();
        let device_type = g("device_type");
        let words = format!(
            "{} {} {device_type} {} {} {} {}",
            kind.replace('_', " "),
            g("type"),
            g("description"),
            g("legend"),
            g("fixture"),
            g("kind_source")
        );
        let documented = !g("fa_kind").is_empty() || !g("nac_circuit").is_empty() || !g("slc_circuit").is_empty();
        let fa =
            documented || sr.class_of(&kind, if device_type.is_empty() { &words } else { &device_type }).is_some_and(|c| c.party == "fire alarm");
        if !fa {
            continue;
        }
        let fa_kind = Some(g("fa_kind")).filter(|k| !k.is_empty()).or_else(|| sr.fa_kind_of(&words)).unwrap_or_else(|| "fire alarm device".into());
        let role = Role::of_kind(&fa_kind);
        let Some((lo, hi)) = mesh_box(e) else { continue };
        let off = super::super::bspec::stacked_by(e);
        let at = Vec2::new((lo.x + hi.x) / 2.0 - off.x, (lo.y + hi.y) / 2.0 - off.y);
        let candela = numbers(&g("candela")).into_iter().fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
        let (amps, amps_basis) = if role == Role::Notification { fr.amps_of(&fa_kind, candela) } else { (0.0, String::new()) };
        out.push(FaDevice {
            handle: e.handle,
            level: super::super::bspec::made_level(e).unwrap_or_default(),
            at,
            fa_kind,
            role,
            amps,
            amps_basis,
            nac: g("nac_circuit"),
            slc: g("slc_circuit"),
            address: g("address"),
        });
    }
    out
}

/// Whether the built model has a fire alarm device (something to wire).
pub(crate) fn any_device(d: &Drawing) -> bool {
    let sp = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok()).unwrap_or_default();
    !devices(d, &sp).is_empty()
}

/// Whether a system is one of the fire alarm circuits'.
pub(crate) fn is_system(system: &str) -> bool {
    system == NAC_SYSTEM || system == SLC_SYSTEM
}

// ─── The documents ───────────────────────────────────────────────────────────

/// A cable a wire legend gives a circuit: its size, conductors and type, and the words.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DocCable {
    pub awg: u32,
    pub conductors: u32,
    #[serde(rename = "type")]
    pub ty: String,
    pub words: String,
}

/// What the documents say of the fire alarm wiring method.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WiringDocs {
    /// The raceway the wiring goes in, and below what height above the floor (feet; none: all
    /// of it), and the words.
    pub raceway: Option<(String, Option<f64>, String)>,
    /// A plenum cable the documents call for, and the words.
    pub plenum: Option<String>,
    /// Class A circuits, and the words.
    pub class_a: Option<String>,
    /// The cable a wire legend gives each circuit (`slc`, `nac`, `annunciator`).
    pub cables: BTreeMap<String, DocCable>,
}

/// One statement to read: where it is (its document and place), its words, and whether its
/// document is the fire alarm system's own (its subject is that system already).
pub(crate) struct Statement {
    pub cite: String,
    pub text: String,
    pub fa_doc: bool,
}

/// Up to `before` and `after` bytes of `flat` round byte `at`, on character boundaries.
fn window(flat: &str, at: usize, before: usize, after: usize) -> String {
    let start = at.saturating_sub(before);
    let end = at.saturating_add(after).min(flat.len());
    let s = (0..=start).rev().find(|i| flat.is_char_boundary(*i)).unwrap_or(0);
    let e = (end..=flat.len()).find(|i| flat.is_char_boundary(*i)).unwrap_or(flat.len());
    let mut q = flat.get(s..e).unwrap_or_default().trim().to_string();
    if s > 0 {
        q = format!("…{q}");
    }
    if e < flat.len() {
        q.push('…');
    }
    q
}

/// Where in `up` (upper case) the first of `words` is (as whole words).
fn first_at(up: &str, words: &[String]) -> Option<usize> {
    words
        .iter()
        .filter(|w| !w.trim().is_empty())
        .filter_map(|w| {
            let w = w.to_ascii_uppercase();
            up.match_indices(w.as_str())
                .find(|(i, _)| {
                    let before = up.get(..*i).and_then(|b| b.chars().next_back());
                    let after = up.get(i + w.len()..).and_then(|a| a.chars().next());
                    before.is_none_or(|c| !c.is_ascii_alphanumeric()) && after.is_none_or(|c| !c.is_ascii_alphanumeric())
                })
                .map(|(i, _)| i)
        })
        .min()
}

/// The cable a wire legend gives each circuit: the circuit's name, then (before the next
/// circuit's name) its `N AWG M CONDUCTOR` and its cable type.
fn legend_cables(flat: &str, cite: &str, w: &DocWords, out: &mut BTreeMap<String, DocCable>) {
    let up = flat.to_ascii_uppercase();
    let mut names: Vec<(usize, usize, &str)> = Vec::new();
    for (role, phrases) in &w.legend {
        for ph in phrases.iter().filter(|p| !p.trim().is_empty()) {
            let ph = ph.to_ascii_uppercase();
            for (i, _) in up.match_indices(ph.as_str()).take(64) {
                names.push((i, i + ph.len(), role.as_str()));
            }
        }
    }
    names.sort_unstable();
    for (k, (start, end, role)) in names.iter().enumerate() {
        if out.contains_key(*role) {
            continue;
        }
        let stop = names.iter().skip(k + 1).map(|n| n.0).find(|n| *n >= *end).unwrap_or(up.len()).min(end + 160);
        let Some(seg) = up.get(*end..stop) else { continue };
        let toks: Vec<&str> = seg.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')').filter(|t| !t.is_empty()).collect();
        let num = |t: &str| t.trim_start_matches('#').parse::<u32>().ok();
        let awg = toks.windows(2).find(|p| matches!(p, [_, b] if *b == "AWG")).and_then(|p| p.first().and_then(|a| num(a)));
        let conductors = toks
            .windows(2)
            .find(|p| matches!(p, [_, b] if b.starts_with("CONDUCTOR") || *b == "COND." || *b == "COND"))
            .and_then(|p| p.first().and_then(|a| num(a)));
        let ty = toks.iter().find(|t| matches!(**t, "FPL" | "FPLR" | "FPLP")).map(|t| t.to_string()).unwrap_or_default();
        if let (Some(awg), Some(n)) = (awg, conductors)
            && (10..=24).contains(&awg)
            && (1..=12).contains(&n)
        {
            out.insert(
                (*role).to_string(),
                DocCable { awg, conductors: n, ty, words: format!("{cite}: {}", window(flat, *start, 0, (stop - start).min(200))) },
            );
        }
    }
}

/// The statements of the fire alarm wiring method among `texts`.
pub(crate) fn read_docs(texts: &[Statement]) -> WiringDocs {
    let w = &rules().documents;
    let mut out = WiringDocs::default();
    for st in texts.iter().take(400_000) {
        let flat: String = st.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let s = spaced(&flat);
        if !(st.fa_doc || has_any(&s, &w.subject_words) || s.contains(" 760 ")) {
            continue;
        }
        let up = flat.to_ascii_uppercase();
        let quote =
            |at: usize| if st.cite.is_empty() { window(&flat, at, 120, 180) } else { format!("{}: {}", st.cite, window(&flat, at, 120, 180)) };
        if out.raceway.is_none() && has_any(&s, &w.raceway_words) && has_any(&s, &w.wiring_words) && !has_any(&s, &w.not_words) {
            // `BELOW 7'`: the height the raceway goes to; none named, all of it.
            let below = w.below_words.iter().filter_map(|b| up.find(b.as_str()).map(|i| i + b.len())).min().and_then(|i| {
                let rest = up.get(i..)?;
                let first = numbers(rest.get(..rest.len().min(16))?).into_iter().next()?;
                let inches = rest.trim_start().chars().skip_while(|c| c.is_ascii_digit() || *c == '.' || *c == ' ').take(2).collect::<String>();
                let ft = if inches.starts_with('"') || inches.starts_with("IN") { first / 12.0 } else { first };
                (ft.is_finite() && ft > 0.0 && ft < 40.0).then_some(ft)
            });
            let at = first_at(&up, &w.raceway_words).unwrap_or(0);
            out.raceway = Some((w.raceway.clone(), below, quote(at)));
        }
        if out.plenum.is_none()
            && let Some(at) = first_at(&up, &w.plenum_words)
            && (has_any(&s, &w.wiring_words) || s.contains(" FPLP "))
        {
            out.plenum = Some(quote(at));
        }
        // Class A where a statement requires it (not product data listing what a panel takes,
        // nor a choice of A or B).
        if out.class_a.is_none()
            && let Some(at) = first_at(&up, &w.class_a_words)
            && has_any(&s, &w.require_words)
            && !w.class_b_words.iter().any(|b| up.contains(&b.to_ascii_uppercase()))
        {
            out.class_a = Some(quote(at));
        }
        legend_cables(&flat, &st.cite, w, &mut out.cables);
    }
    out
}

impl WiringDocs {
    pub(crate) fn stored(d: &Drawing) -> Self {
        d.embedded.get(DOCS_FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
    }

    pub(crate) fn store(&self, d: &mut Drawing) {
        if let Ok(b) = serde_json::to_vec(self) {
            d.embedded.insert(DOCS_FILE.into(), std::sync::Arc::new(b));
        }
    }

    /// Each statement this reading has, else the other's.
    pub(crate) fn or(self, other: WiringDocs) -> WiringDocs {
        let mut cables = other.cables;
        cables.extend(self.cables);
        WiringDocs { raceway: self.raceway.or(other.raceway), plenum: self.plenum.or(other.plenum), class_a: self.class_a.or(other.class_a), cables }
    }

    /// The cable of circuits of `role` (`nac`, `slc`, `annunciator`): the wire legend's, else
    /// `base`; a plenum cable where the documents call for one. And whether a document gave it.
    fn cable(&self, role: &str, base: &Cable) -> (Cable, bool) {
        let mut c = base.clone();
        let doc = self.cables.get(role);
        if let Some(dc) = doc {
            c.awg = dc.awg;
            c.size = format!("{}/{}", dc.awg, dc.conductors);
            if !dc.ty.is_empty() {
                c.ty = dc.ty.clone();
            }
        }
        if self.plenum.is_some() && doc.is_none_or(|d| d.ty.is_empty()) {
            c.ty = "FPLP".into();
        }
        (c, doc.is_some())
    }
}

/// The fire alarm wiring statements of the job's documents (the sheets' notes, specification
/// sections, submittals): each paragraph's sentences, a fire alarm document's own on its subject
/// already; each cited by its document and place.
pub(crate) fn read_documents(docs: &[scope::Doc]) -> WiringDocs {
    let mut texts: Vec<Statement> = Vec::new();
    for doc in docs.iter().take(5_000) {
        let fa_doc = doc.trade.as_deref() == Some("fire alarm");
        for p in doc.paras.iter().take(20_000) {
            let cite = format!("{} {}", doc.name, p.at).trim().to_string();
            for sentence in super::lv::sentences(&p.text).into_iter().take(200) {
                if texts.len() < 400_000 {
                    texts.push(Statement { cite: cite.clone(), text: sentence, fa_doc });
                }
            }
        }
    }
    read_docs(&texts)
}

/// The drawing's texts, each marked when it lies on a fire alarm sheet (`FA` discipline).
fn drawing_texts(d: &Drawing, sp: &BuildingSpec) -> Vec<Statement> {
    let fa_rects: Vec<[f64; 4]> =
        sp.sheets.iter().filter(|s| s.discipline.as_deref().is_some_and(|x| x.eq_ignore_ascii_case("FA"))).filter_map(|s| s.rect).take(500).collect();
    cadaddy_mepfix::sheet_facts::texts(d)
        .into_iter()
        .map(|t| {
            let within = |r: &[f64; 4]| t.at.x >= r[0].min(r[2]) && t.at.x <= r[0].max(r[2]) && t.at.y >= r[1].min(r[3]) && t.at.y <= r[1].max(r[3]);
            let fa_doc = fa_rects.iter().any(within);
            let cite =
                sp.sheets.iter().find(|s| s.rect.as_ref().is_some_and(within)).map_or_else(|| "the drawing's note".to_string(), |s| s.number.clone());
            Statement { cite, text: t.value, fa_doc }
        })
        .collect()
}

// ─── The equipment ───────────────────────────────────────────────────────────

/// The storey a room box is of, its plan outline (its stacking taken off).
fn plan_outline(r: &super::super::gearbuild::RoomBox) -> Vec<Vec2> {
    r.outline.iter().map(|p| *p - r.offset).collect()
}

fn centre(pts: &[Vec2]) -> Vec2 {
    if pts.is_empty() {
        return Vec2::new(0.0, 0.0);
    }
    pts.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / pts.len() as f64)
}

/// A place along a wall of the best room of `level` for a box of `rule`'s size: the rooms whose
/// names have the rule's words (its first group first), each group's nearest `near` first.
fn place_in_room(
    rooms: &[super::super::gearbuild::RoomBox],
    level: &str,
    near: Vec2,
    rule: &Placement,
    n: usize,
    upf: f64,
    taken: &[(String, [f64; 2], [f64; 2])],
    units: &BTreeSet<(String, String)>,
) -> Vec<(super::super::gearbuild::Placed, String)> {
    let [w, dp, _] = rule.size_in;
    let (w, dp) = (w.max(1.0) / 12.0 * upf, dp.max(1.0) / 12.0 * upf);
    // Never a dwelling or guest unit (the spec's unit instances), a stair, a shaft, a toilet room.
    let unit = |r: &super::super::gearbuild::RoomBox| units.contains(&(r.level.to_ascii_lowercase(), r.number.trim().to_ascii_uppercase()));
    let on: Vec<&super::super::gearbuild::RoomBox> = rooms
        .iter()
        .filter(|r| same_level(&r.level, level) && r.outline.len() >= 3 && !r.building && !unit(r) && !has_any(&spaced(&r.name), &rule.never_words))
        .collect();
    let mut groups: Vec<Vec<&super::super::gearbuild::RoomBox>> =
        rule.room_words.iter().map(|ws| on.iter().copied().filter(|r| has_any(&spaced(&r.name), ws)).collect()).collect();
    // No room the rules name: the room nearest the riser (never a unit's).
    groups.push(on.clone());
    let mut out = Vec::new();
    let mut seen: BTreeSet<(String, String, i64, i64)> = BTreeSet::new();
    for mut g in groups {
        g.sort_by(|a, b| manhattan(centre(&plan_outline(a)), near).total_cmp(&manhattan(centre(&plan_outline(b)), near)));
        for r in g {
            let c = centre(&plan_outline(r));
            if !seen.insert((r.number.clone(), r.name.clone(), c.x.round() as i64, c.y.round() as i64)) {
                continue;
            }
            let outline = plan_outline(r);
            let want = n.saturating_sub(out.len());
            if want == 0 {
                return out;
            }
            // Clear of what is placed in it already.
            let held = taken
                .iter()
                .filter(|(l, a, b)| same_level(l, level) && super::inside(Vec2::new((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0), &outline))
                .count();
            let sizes = vec![(w, dp); want + held];
            let got = super::super::gearbuild::place_along_walls(&outline, &sizes, w, 3.0 * upf, 0.5 * upf);
            let name = if r.number.trim().is_empty() { r.name.trim().to_string() } else { format!("{} ({})", r.number.trim(), r.name.trim()) };
            for p in got.into_iter().skip(held).flatten() {
                if out.len() < n {
                    out.push((p, name.clone()));
                }
            }
            if out.len() >= n {
                return out;
            }
        }
    }
    out
}

/// Whether this step placed a piece of equipment (by rule): where the documents put one of its
/// tag, theirs replaces it.
pub(crate) fn placed_by_rule(e: &MepEquipment) -> bool {
    e.properties.get(PLACED_BY).is_some_and(|v| v == PLACED_BY_US)
}

/// Remove the built equipment this step placed whose tags are `tags` (the documents placed them
/// since). Returns how many went.
pub(crate) fn remove_placed(d: &mut Drawing, tags: &[String]) -> usize {
    let want: BTreeSet<String> = tags.iter().map(|t| t.trim().to_ascii_uppercase()).collect();
    let gone: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| super::super::bimlook::made_prop(e, PLACED_BY).is_some_and(|v| v == PLACED_BY_US))
        .filter(|e| super::super::bimlook::made_tag(e, "tag").is_some_and(|t| want.contains(&t.to_ascii_uppercase())))
        .map(|e| e.handle)
        .collect();
    for h in &gone {
        d.model.remove(*h);
    }
    gone.len()
}

/// The control panel and the NAC power supplies the fire alarm system needs where the documents
/// place none (inferred, `fawiring.json`), into the spec (the drawing's too) and built; those an
/// earlier build placed are placed again from what stands now. Returns what it did.
pub(crate) fn place_equipment(d: &mut Drawing, sp: &mut BuildingSpec) -> Value {
    let fr = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let devs = devices(d, sp);
    let ours = placed_by_rule;
    // What an earlier build placed goes (its elements too): placed again below.
    let old: BTreeSet<String> = sp.mep.equipment.iter().filter(|e| ours(e)).map(|e| e.tag.to_ascii_uppercase()).collect();
    if devs.is_empty() && old.is_empty() {
        return json!({"devices": 0});
    }
    sp.mep.equipment.retain(|e| !ours(e));
    let gone: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| super::super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment")))
        .filter(|e| super::super::bimlook::made_prop(e, PLACED_BY).is_some_and(|v| v == PLACED_BY_US))
        .map(|e| e.handle)
        .collect();
    for h in &gone {
        d.model.remove(*h);
    }
    let rooms = scope::room_boxes(d);
    // The dwelling and guest units by storey and number (a unit instance naming no storey: its
    // number on every storey).
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let instances: Vec<(String, String)> = sp
        .unit_instances
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_DEVICES)
        .map(|u| (text(u, "level").to_ascii_lowercase(), text(u, "number").to_ascii_uppercase()))
        .filter(|(_, n)| !n.is_empty())
        .collect();
    let any_level: BTreeSet<&str> = instances.iter().filter(|(l, _)| l.is_empty()).map(|(_, n)| n.as_str()).collect();
    let units: BTreeSet<(String, String)> = instances
        .iter()
        .filter(|(l, _)| !l.is_empty())
        .cloned()
        .chain(
            rooms
                .iter()
                .filter(|r| any_level.contains(r.number.trim().to_ascii_uppercase().as_str()))
                .map(|r| (r.level.to_ascii_lowercase(), r.number.trim().to_ascii_uppercase())),
        )
        .collect();
    let moves = super::level_moves(d, sp);
    let move_of = |l: &str| moves.get(&l.to_ascii_lowercase()).copied().unwrap_or(Vec2::new(0.0, 0.0));
    let carry = |p: Vec2, from: &str, to: &str| p + move_of(from) - move_of(to);
    let elev = |l: &str| sp.level(l).map_or(0.0, |x| x.elevation_ft);
    let eq_centre = |e: &MepEquipment| Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0);
    let mut placed: Vec<MepEquipment> = Vec::new();
    let mut not_placed: Vec<Value> = Vec::new();
    let mut taken: Vec<(String, [f64; 2], [f64; 2])> = sp.mep.equipment.iter().map(|e| (e.level.clone(), e.min, e.max)).collect();
    let mut tags: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.to_ascii_uppercase()).collect();
    // A tag of its own: the rules' tag whole when `whole` and free, else numbered.
    let new_tag = |base: &str, whole: bool, tags: &mut BTreeSet<String>| -> String {
        if whole && tags.insert(base.to_ascii_uppercase()) {
            return base.to_string();
        }
        (1..10_000).map(|n| format!("{base}-{n}")).find(|t| tags.insert(t.to_ascii_uppercase())).unwrap_or_else(|| base.to_string())
    };
    let make = |rule: &Placement, tag: String, level: &str, p: &super::super::gearbuild::Placed, fa_kind: &str, why: String, rule_id: &str| {
        let h = rule.size_in[2].max(1.0);
        let mut properties = BTreeMap::new();
        properties.insert("FA_KIND".to_string(), fa_kind.to_string());
        properties.insert("STATUS".to_string(), Made::Inferred.as_str().to_string());
        // Its own reason first (an element's properties are kept to 250 characters), the rule's
        // whole basis beside it.
        properties.insert("BASIS".to_string(), format!("{rule_id}: {why}"));
        properties.insert("RULE".to_string(), super::super::provenance::rule(rule_id).basis_for(""));
        properties.insert("PLACEMENT".to_string(), format!("inferred: {why}"));
        properties.insert(PLACED_BY.to_string(), PLACED_BY_US.to_string());
        let mut sources = BTreeMap::new();
        sources.insert("position".to_string(), why);
        sources.insert("size".to_string(), rule.source.clone());
        sources.insert("status".to_string(), "inferred".to_string());
        MepEquipment {
            tag,
            kind: rule.kind.clone(),
            level: level.to_string(),
            min: p.min,
            max: p.max,
            bottom_ft: ((rule.top_in - h) / 12.0).max(0.0),
            height_ft: h / 12.0,
            title: rule.title.clone(),
            properties,
            sources,
            confidence: 0.4,
            ..MepEquipment::default()
        }
    };
    // -- The control panel: the documents', else one by rule on the lowest storey wired. --
    let mut panel = sp.mep.equipment.iter().find(|e| equipment_role(e) == Some(Role::Panel)).map(|e| (e.tag.clone(), e.level.clone(), eq_centre(e)));
    let mut levels: Vec<String> = devs.iter().map(|x| x.level.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    levels.sort_by(|a, b| elev(a).total_cmp(&elev(b)));
    if panel.is_none()
        && let Some(lowest) = levels.first()
    {
        let near = centre(&devs.iter().filter(|x| same_level(&x.level, lowest)).map(|x| x.at).collect::<Vec<_>>());
        match place_in_room(&rooms, lowest, near, &fr.control_panel, 1, upf, &taken, &units).into_iter().next() {
            Some((p, room)) => {
                let tag = new_tag(&fr.control_panel.tag, true, &mut tags);
                let why = format!(
                    "no document places the fire alarm control panel: along a wall of room {room}, the lowest storey with fire alarm devices"
                );
                let e = make(&fr.control_panel, tag.clone(), lowest, &p, "control panel", why, RULE_PANEL);
                taken.push((lowest.clone(), e.min, e.max));
                panel = Some((tag, lowest.clone(), eq_centre(&e)));
                placed.push(e);
            }
            None => not_placed.push(json!({"what": "control panel", "level": lowest, "why": "no room of the storey holds it"})),
        }
    }
    // -- The NAC power supplies each storey's load needs. --
    let cap = fr.circuit_cap();
    let supply_cap = cap * fr.power_supply.nac_circuits.max(1) as f64;
    let panel_cap = cap * fr.control_panel.nac_circuits as f64;
    let mut by_level: Vec<Value> = Vec::new();
    for level in levels.iter().take(500) {
        let load: f64 = devs.iter().filter(|x| same_level(&x.level, level) && x.role == Role::Notification).map(|x| x.amps).sum();
        let appliances = devs.iter().filter(|x| same_level(&x.level, level) && x.role == Role::Notification).count();
        if appliances == 0 {
            continue;
        }
        let have = sp.mep.equipment.iter().filter(|e| same_level(&e.level, level) && equipment_role(e) == Some(Role::Supply)).count();
        let panel_here = panel.as_ref().is_some_and(|(_, l, _)| same_level(l, level));
        let need = if panel_here && load <= panel_cap + 1e-9 { 0 } else { ((load / supply_cap).ceil() as usize).max(1) };
        let add = need.saturating_sub(have).min(MAX_PLACED);
        let near = panel
            .as_ref()
            .map(|(_, l, c)| carry(*c, l, level))
            .unwrap_or_else(|| centre(&devs.iter().filter(|x| same_level(&x.level, level)).map(|x| x.at).collect::<Vec<_>>()));
        let mut got = 0usize;
        if add > 0 {
            for (p, room) in place_in_room(&rooms, level, near, &fr.power_supply, add, upf, &taken, &units) {
                if placed.len() >= MAX_PLACED {
                    break;
                }
                let tag = new_tag(&fr.power_supply.tag, false, &mut tags);
                let why = format!(
                    "{appliances} notification appliances on {level} draw {:.2} A{}: {need} NAC power supply(ies) of {:.1} A each; along a wall of room {room}, nearest the riser",
                    load,
                    if panel_here { format!(", past the control panel's own {:.1} A", panel_cap) } else { String::new() },
                    supply_cap
                );
                let e = make(&fr.power_supply, tag, level, &p, "power supply", why, RULE_SUPPLY);
                taken.push((level.clone(), e.min, e.max));
                placed.push(e);
                got += 1;
            }
            if got < add {
                not_placed
                    .push(json!({"what": "power supply", "level": level, "wanted": add, "placed": got, "why": "no room of the storey holds it"}));
            }
        }
        by_level.push(json!({"level": level, "appliances": appliances, "amps": r2(load), "documented": have, "needed": need, "placed": got}));
    }
    // -- Into the spec (the drawing's too) and built. --
    let new_tags: Vec<String> = placed.iter().map(|e| e.tag.clone()).collect();
    sp.mep.equipment.extend(placed.iter().cloned());
    if let Some(mut stored) = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok()) {
        let before = stored.mep.equipment.clone();
        stored.mep.equipment.retain(|e| !ours(e));
        stored.mep.equipment.extend(placed.iter().cloned());
        if stored.mep.equipment != before {
            d.building_spec = Some(stored.to_json()).into();
        }
    }
    let built = super::super::scope::build_panels(d, sp, &new_tags, &rooms);
    json!({
        "devices": devs.len(),
        "control_panel": panel.as_ref().map(|(t, l, _)| json!({"tag": t, "level": l})),
        "placed": placed.iter().map(|e| json!({"tag": e.tag, "kind": e.kind, "level": e.level, "why": e.sources.get("position")})).collect::<Vec<_>>(),
        "built": built,
        "not_placed": not_placed,
        "by_level": by_level,
        "source": fr.power_supply.source,
    })
}

// ─── Routing ─────────────────────────────────────────────────────────────────

/// What the router gives the fire alarm wiring: the low-voltage pathways' context (the model,
/// its fields, tiers, ceilings and rated walls) and the gear it found.
pub(super) struct Ctx<'a, 'b> {
    pub base: &'a super::lv::Ctx<'b>,
    pub gear: &'a [GearEl],
}

/// What routing the fire alarm circuits made.
#[derive(Default)]
pub(super) struct Out {
    pub runs: Vec<MepRun>,
    pub tags: Vec<ElementTag>,
    pub report: Value,
}

/// An end of a leg: a device or a piece of gear, where its wiring enters.
#[derive(Clone, Debug)]
struct Node {
    handle: Handle,
    level: String,
    at: Vec2,
    top: f64,
    face: Option<Vec2>,
    tag: String,
}

/// One circuit as planned and routed.
struct Circuit {
    name: String,
    system: &'static str,
    role: &'static str,
    source: Node,
    /// Its legs: from, to, the pieces (runs), feet.
    legs: Vec<(Node, Node, Vec<MepRun>, f64)>,
    members: Vec<Handle>,
    cable: Cable,
    amps: f64,
    order: &'static str,
    rule: &'static str,
    class_a: bool,
    /// Its design cable size and why it was upsized.
    upsized: Option<(String, String)>,
    blocked: usize,
    /// What its appliances' currents were taken as (`177 cd`, `no candela read: …`).
    amps_why: BTreeSet<String>,
}

impl Circuit {
    fn add(&mut self, m: &FaDevice) {
        self.amps += m.amps;
        self.members.push(m.handle);
        if let Some(w) = m.amps_basis.split(':').next().filter(|w| !w.trim().is_empty())
            && self.amps_why.len() < 16
        {
            self.amps_why.insert(w.trim().to_string());
        }
    }

    fn ft(&self) -> f64 {
        self.legs.iter().map(|l| l.3).sum()
    }
}

/// The nearest-neighbour order of `pts` from `start` (a square distance: the way a crew pulls
/// cable box to box).
fn nn_order(start: Vec2, pts: &[(usize, Vec2)]) -> Vec<usize> {
    let mut left: Vec<(usize, Vec2)> = pts.iter().take(MAX_DEVICES).copied().collect();
    let mut out = Vec::with_capacity(left.len());
    let mut at = start;
    while !left.is_empty() {
        let Some((k, _)) = left.iter().enumerate().min_by(|a, b| manhattan(at, a.1.1).total_cmp(&manhattan(at, b.1.1))) else { break };
        let (i, p) = left.swap_remove(k);
        out.push(i);
        at = p;
    }
    out
}

pub(super) fn route(cx: &Ctx) -> Out {
    let fr = rules();
    let b = cx.base;
    let (d, sp, upf, rl) = (b.d, b.sp, b.upf, b.rl);
    let fa_devs = devices(d, sp);
    let mut out = Out::default();
    if fa_devs.is_empty() {
        out.report = json!({"devices": 0, "note": "no fire alarm device in the model"});
        return out;
    }
    // The job's documents first (specifications, submittals: stored as read), then the drawing's
    // own notes.
    let docs = WiringDocs::stored(d).or(read_docs(&drawing_texts(d, sp)));
    let elev = |l: &str| sp.level(l).map_or(0.0, |x| x.elevation_ft);
    // -- The nodes: the devices where the router has them, the fire alarm gear. --
    let dev_at: HashMap<Handle, &super::Dev> = b.devs.iter().map(|x| (x.handle, x)).collect();
    let node_of_dev = |f: &FaDevice| -> Node {
        match dev_at.get(&f.handle) {
            Some(x) => Node { handle: x.handle, level: x.level.clone(), at: x.at, top: x.top, face: x.face, tag: String::new() },
            None => Node { handle: f.handle, level: f.level.clone(), at: f.at, top: elev(&f.level) * upf, face: None, tag: String::new() },
        }
    };
    let eq_of = |tag: &str| sp.mep.equipment.iter().find(|e| !tag.is_empty() && e.tag.eq_ignore_ascii_case(tag));
    let mut panels: Vec<Node> = Vec::new();
    let mut supplies: Vec<Node> = Vec::new();
    let mut annunciators: Vec<Node> = Vec::new();
    let mut modules: Vec<Node> = Vec::new();
    for g in cx.gear.iter().take(MAX_DEVICES) {
        let Some(role) = eq_of(&g.tag).and_then(equipment_role) else { continue };
        let n = Node {
            handle: g.handle,
            level: g.level.clone(),
            at: Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0),
            top: g.hi.z,
            face: None,
            tag: g.tag.clone(),
        };
        match role {
            Role::Panel => panels.push(n),
            Role::Supply => supplies.push(n),
            Role::Annunciator => annunciators.push(n),
            Role::Notification | Role::Initiating => modules.push(n),
        }
    }
    // The control panel: the one tagged as the rules' panel first.
    panels.sort_by_key(|n| !n.tag.eq_ignore_ascii_case(&fr.control_panel.tag));
    let Some(facp) = panels.first().cloned() else {
        out.report = json!({
            "devices": fa_devs.len(),
            "circuits": 0,
            "unrouted": fa_devs.len(),
            "note": "no fire alarm control panel in the model (no document places one and none could be placed): the circuits are not routed",
        });
        return out;
    };
    let riser = |level: &str| (b.carry)(facp.at, &facp.level, level);
    let mut penetrations: Vec<Value> = Vec::new();
    // -- One leg, `a` to `b`: up into the ceiling space and across square; between storeys by
    // the riser over the panel (its pieces: across to the riser, up or down it, across to `b`). --
    let leg = |a: &Node, bn: &Node, size_in: f64, pens: &mut Vec<Value>| -> (Vec<MepRun>, f64, usize) {
        let r = size_in.max(0.1) / 24.0 * upf;
        let skip = [a.handle, bn.handle];
        let field = |l: &str| b.fields.get(&l.to_ascii_lowercase()).unwrap_or(b.empty);
        let mk = |level: &str, pts: Vec<[f64; 2]>, bot: Vec<f64>| -> MepRun {
            let mut run = super::super::printcond::run(NAC_SYSTEM, level, "fire alarm", pts, size_in, 0.0);
            run.bottoms_ft = bot;
            run.height_source = ROUTED.into();
            run.objects = vec![a.handle.hex(), bn.handle.hex()];
            run
        };
        let mut runs = Vec::new();
        let mut blocked = 0usize;
        if same_level(&a.level, &bn.level) {
            let over = [(b.box_ceiling)(&a.level, a.at, a.face), (b.box_ceiling)(&a.level, bn.at, bn.face)]
                .into_iter()
                .flatten()
                .fold(None, |m: Option<f64>, z| Some(m.map_or(z, |m| m.max(z))));
            let (floor, tiers) = (b.tiers_of)(&a.level, over);
            let z0 = tiers.first().copied().unwrap_or(floor + rl.route.default_ceiling_ft * upf);
            let f = field(&a.level);
            let (qa, ja, _) = rise_point(a.at, a.top, a.face, z0, r, f, upf, &skip, rl);
            let (qb, jb, _) = rise_point(bn.at, bn.top, bn.face, z0, r, f, upf, &skip, rl);
            let l = route_leg(qa, qb, &tiers, r, f, upf, &skip, rl);
            blocked += l.blocked.len();
            penetrate(pens, b.rated, &a.level, &l, "fire alarm", size_in, upf, floor, rl);
            let (mut pts, mut bot) = from_box(a.at, a.top, (qa, ja), &l, floor, upf);
            to_box(&mut pts, &mut bot, bn.at, bn.top, (qb, jb), floor, upf);
            runs.push(mk(&a.level, pts, bot));
        } else {
            let (ra, rb) = (riser(&a.level), riser(&bn.level));
            let (floor_a, tiers_a) = (b.tiers_of)(&a.level, (b.box_ceiling)(&a.level, a.at, a.face));
            let (floor_b, tiers_b) = (b.tiers_of)(&bn.level, (b.box_ceiling)(&bn.level, bn.at, bn.face));
            let za = tiers_a.first().copied().unwrap_or(floor_a + rl.route.default_ceiling_ft * upf);
            let zb = tiers_b.first().copied().unwrap_or(floor_b + rl.route.default_ceiling_ft * upf);
            let (fa_, fb_) = (field(&a.level), field(&bn.level));
            let (qa, ja, _) = rise_point(a.at, a.top, a.face, za, r, fa_, upf, &skip, rl);
            let la = route_leg(qa, ra, &tiers_a, r, fa_, upf, &skip, rl);
            blocked += la.blocked.len();
            penetrate(pens, b.rated, &a.level, &la, "fire alarm", size_in, upf, floor_a, rl);
            let (pts, bot) = from_box(a.at, a.top, (qa, ja), &la, floor_a, upf);
            let last = bot.last().copied().unwrap_or((za - floor_a) / upf);
            runs.push(mk(&a.level, pts, bot));
            // Up or down the riser, from this storey's ceiling space to the other's.
            let other = (zb - floor_a) / upf;
            let mut up = mk(&a.level, vec![[ra.x, ra.y]], vec![last.min(other)]);
            up.top_ft = Some(last.max(other));
            if (other - last).abs() > 1e-6 {
                runs.push(up);
            }
            let (z_lo, z_hi) = (floor_a.min(zb) / upf, (za.max(zb)) / upf);
            for l in sp.levels.iter().filter(|l| l.sheet.is_none() && l.elevation_ft > z_lo + 1e-6 && l.elevation_ft <= z_hi).take(64) {
                if pens.len() < MAX_ROWS {
                    pens.push(json!({
                        "floor": l.name, "rating": "not given", "level": a.level, "circuit": "fire alarm riser", "at": [r2(ra.x), r2(ra.y)],
                        "firestop": format!("{} for fire alarm cable through the {} floor", rl.firestop.item, l.name), "source": fr.riser_source,
                    }));
                }
            }
            let (qb, jb, _) = rise_point(bn.at, bn.top, bn.face, zb, r, fb_, upf, &skip, rl);
            let lb = route_leg(rb, qb, &tiers_b, r, fb_, upf, &skip, rl);
            blocked += lb.blocked.len();
            penetrate(pens, b.rated, &bn.level, &lb, "fire alarm", size_in, upf, floor_b, rl);
            let mut pts: Vec<[f64; 2]> = lb.pts.iter().map(|p| [p.x, p.y]).collect();
            let mut bot: Vec<f64> = lb.z.iter().map(|z| (z - floor_b) / upf).collect();
            if pts.len() < 2 {
                pts = vec![[rb.x, rb.y], [qb.x, qb.y]];
                bot = vec![(zb - floor_b) / upf];
            }
            to_box(&mut pts, &mut bot, bn.at, bn.top, (qb, jb), floor_b, upf);
            runs.push(mk(&bn.level, pts, bot));
        }
        let ft = runs.iter().map(|x| run_ft(x, upf)).sum::<f64>();
        (runs, ft, blocked)
    };
    let body = fr.cables.body_in.max(0.1);
    // Each circuit's cable: the documents' wire legend, else the rules' (plenum where they say).
    let cable_of = |role: &str, base: &Cable| docs.cable(role, base).0;
    let nac_cable = cable_of("nac", &fr.cables.nac);
    // One size up where a NAC's drop needs it (#14 to #12, #12 to #10).
    let up_awg = nac_cable.awg.saturating_sub(2).max(10);
    let class_a = docs.class_a.is_some();
    let mut circuits: Vec<Circuit> = Vec::new();
    let new_circuit =
        |name: String, system: &'static str, role: &'static str, source: &Node, cable: Cable, order: &'static str, rule: &'static str| Circuit {
            name,
            system,
            role,
            source: source.clone(),
            legs: Vec::new(),
            members: Vec::new(),
            cable,
            amps: 0.0,
            order,
            rule,
            class_a,
            upsized: None,
            blocked: 0,
            amps_why: BTreeSet::new(),
        };

    // -- The notification appliance circuits. --
    let appliances: Vec<&FaDevice> = fa_devs.iter().filter(|x| x.role == Role::Notification).collect();
    let sources: Vec<Node> = supplies.iter().cloned().chain(std::iter::once(facp.clone())).collect();
    let dist3 = |a: &Node, p: Vec2, level: &str| manhattan((b.carry)(a.at, &a.level, level), p) + (elev(&a.level) - elev(level)).abs() * upf;
    // The documents' circuits: by name, their source by the name's panel part, else the nearest.
    let mut known: BTreeMap<String, Vec<&FaDevice>> = BTreeMap::new();
    let mut unknown: Vec<&FaDevice> = Vec::new();
    for a in &appliances {
        if a.nac.is_empty() { unknown.push(a) } else { known.entry(a.nac.to_ascii_uppercase()).or_default().push(a) }
    }
    let nearest_source = |p: Vec2, level: &str| -> Node {
        // A power supply of the appliance's storey first (the panel where its storey has none);
        // else the nearest storey's; else the panel.
        let supplies_here: Vec<&Node> = sources.iter().filter(|s| same_level(&s.level, level) && s.handle != facp.handle).collect();
        let here: Vec<&Node> =
            if supplies_here.is_empty() { sources.iter().filter(|s| same_level(&s.level, level)).collect() } else { supplies_here };
        let pool: Vec<&Node> = if here.is_empty() { sources.iter().filter(|s| s.handle != facp.handle).collect() } else { here };
        let pool: Vec<&Node> = if pool.is_empty() { sources.iter().collect() } else { pool };
        pool.into_iter().min_by(|x, y| dist3(x, p, level).total_cmp(&dist3(y, p, level))).cloned().unwrap_or_else(|| facp.clone())
    };
    let mut nac_counts: HashMap<Handle, usize> = HashMap::new();
    for (name, mut members) in known.into_iter().take(MAX_CIRCUITS) {
        let head = name.split(['.', '-', ':']).next().unwrap_or_default();
        let first = members.first().map(|m| (m.at, m.level.clone()));
        let source = sources
            .iter()
            .find(|s| !head.is_empty() && s.tag.eq_ignore_ascii_case(head))
            .cloned()
            .or_else(|| first.map(|(p, l)| nearest_source(p, &l)))
            .unwrap_or_else(|| facp.clone());
        let by_address = members.iter().all(|m| address_number(&m.address).is_some());
        if by_address {
            members.sort_by_key(|m| address_number(&m.address).unwrap_or(u32::MAX));
        } else {
            let pts: Vec<(usize, Vec2)> = members.iter().enumerate().map(|(i, m)| (i, m.at)).collect();
            let order = nn_order((b.carry)(source.at, &source.level, members.first().map_or(&source.level, |m| &m.level)), &pts);
            members = order.into_iter().filter_map(|i| members.get(i).copied()).collect();
        }
        let mut c = new_circuit(
            name.clone(),
            NAC_SYSTEM,
            "nac",
            &source,
            nac_cable.clone(),
            if by_address { "addresses" } else { "nearest neighbour" },
            RULE_NAC,
        );
        let mut at = source.clone();
        for m in members {
            let n = node_of_dev(m);
            let (runs, ft, bl) = leg(&at, &n, body, &mut penetrations);
            c.legs.push((at.clone(), n.clone(), runs, ft));
            c.blocked += bl;
            c.add(m);
            at = n;
        }
        *nac_counts.entry(source.handle).or_default() += 1;
        circuits.push(c);
    }
    // The rest: each storey's appliances to their nearest source, chained nearest neighbour, a
    // circuit closed at its output's load or its voltage drop.
    let mut by_source: BTreeMap<usize, Vec<&FaDevice>> = BTreeMap::new();
    for a in &unknown {
        let s = nearest_source(a.at, &a.level);
        let si = sources.iter().position(|x| x.handle == s.handle).unwrap_or(sources.len().saturating_sub(1));
        by_source.entry(si).or_default().push(a);
    }
    let cap = fr.circuit_cap();
    let allowed = fr.drop_allowed();
    for (si, members) in by_source {
        let Some(source) = sources.get(si).cloned() else { continue };
        // Storey by storey from the source's own, each storey's appliances nearest neighbour.
        let mut lv: Vec<String> = members.iter().map(|m| m.level.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        lv.sort_by(|x, y| (elev(x) - elev(&source.level)).abs().total_cmp(&(elev(y) - elev(&source.level)).abs()));
        let mut ordered: Vec<&FaDevice> = Vec::new();
        for l in &lv {
            let on: Vec<&FaDevice> = members.iter().copied().filter(|m| same_level(&m.level, l)).collect();
            let pts: Vec<(usize, Vec2)> = on.iter().enumerate().map(|(i, m)| (i, m.at)).collect();
            ordered.extend(nn_order((b.carry)(source.at, &source.level, l), &pts).into_iter().filter_map(|i| on.get(i).copied()));
        }
        let mut k = nac_counts.get(&source.handle).copied().unwrap_or(0);
        let mut c: Option<Circuit> = None;
        let mut at = source.clone();
        for m in ordered {
            let n = node_of_dev(m);
            let (mut runs, mut ft, mut bl) = leg(&at, &n, body, &mut penetrations);
            let full = c.as_ref().is_some_and(|c| {
                let amps = c.amps + m.amps;
                let total = c.ft() + ft;
                amps > cap + 1e-9 || (fr.nac.max_devices > 0 && c.members.len() >= fr.nac.max_devices) || fr.drop(amps, total, up_awg) > allowed
            });
            if full && let Some(done) = c.take() {
                circuits.push(done);
            }
            if c.is_none() {
                k += 1;
                c = Some(new_circuit(format!("{}-N{k}", source.tag), NAC_SYSTEM, "nac", &source, nac_cable.clone(), "nearest neighbour", RULE_NAC));
                if at.handle != source.handle {
                    (runs, ft, bl) = leg(&source, &n, body, &mut penetrations);
                    at = source.clone();
                }
            }
            if let Some(c) = c.as_mut() {
                c.legs.push((at.clone(), n.clone(), runs, ft));
                c.blocked += bl;
                c.add(m);
            }
            at = n;
        }
        if let Some(done) = c.take() {
            circuits.push(done);
        }
        nac_counts.insert(source.handle, k);
    }
    // Each NAC's drop: its cable where it carries the circuit, else one size up (the design
    // size and the reason kept beside).
    for c in circuits.iter_mut().filter(|c| c.role == "nac") {
        let ft = c.ft();
        let d0 = fr.drop(c.amps, ft, c.cable.awg.max(1));
        if d0 > allowed && up_awg < c.cable.awg {
            let conductors = c.cable.size.split_once('/').map_or("2", |(_, n)| n.trim());
            let up = Cable { awg: up_awg, size: format!("{up_awg}/{conductors}"), ..c.cable.clone() };
            let d1 = fr.drop(c.amps, ft, up_awg);
            let why = format!(
                "{:.2} A over {:.0} ft drops {:.2} V on #{} (more than the {:.1} V from {:.1} V to the last appliance's {:.1} V): #{} drops {:.2} V; {}",
                c.amps, ft, d0, c.cable.awg, allowed, fr.nac.supply_volts, fr.nac.end_volts_min, up_awg, d1, fr.ohms_source
            );
            c.upsized = Some((c.cable.size.clone(), why));
            c.cable = up;
        }
    }
    // -- Each power supply's start circuit from the panel. --
    for s in &supplies {
        let mut c = new_circuit(
            format!("{}-{}", facp.tag, s.tag),
            NAC_SYSTEM,
            "power supply start",
            &facp,
            cable_of("nac", &fr.cables.trigger),
            "the riser",
            RULE_SUPPLY,
        );
        let (runs, ft, bl) = leg(&facp, s, body, &mut penetrations);
        c.legs.push((facp.clone(), s.clone(), runs, ft));
        c.blocked += bl;
        c.members.push(s.handle);
        c.class_a = false;
        circuits.push(c);
    }
    // -- The signaling line circuits: the documents' loops by address, else storey by storey up
    // the riser, nearest neighbour, a loop closed at its device capacity. --
    let initiating: Vec<Node> = fa_devs.iter().filter(|x| x.role == Role::Initiating).map(node_of_dev).chain(modules.iter().cloned()).collect();
    let info: HashMap<Handle, &FaDevice> = fa_devs.iter().map(|x| (x.handle, x)).collect();
    let mut loops: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    let mut rest: Vec<Node> = Vec::new();
    for n in initiating {
        match info.get(&n.handle).map(|x| x.slc.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()) {
            Some(l) => loops.entry(l).or_default().push(n),
            None => rest.push(n),
        }
    }
    let slc_cable = cable_of("slc", &fr.cables.slc);
    let mut slc_n = 0usize;
    for (name, mut members) in loops.into_iter().take(MAX_CIRCUITS) {
        let by_address = members.iter().all(|m| info.get(&m.handle).is_some_and(|x| address_number(&x.address).is_some()));
        if by_address {
            members.sort_by_key(|m| info.get(&m.handle).and_then(|x| address_number(&x.address)).unwrap_or(u32::MAX));
        }
        slc_n += 1;
        let mut c =
            new_circuit(name, SLC_SYSTEM, "slc", &facp, slc_cable.clone(), if by_address { "addresses" } else { "nearest neighbour" }, RULE_SLC);
        let mut at = facp.clone();
        for n in members {
            let (runs, ft, bl) = leg(&at, &n, body, &mut penetrations);
            c.legs.push((at.clone(), n.clone(), runs, ft));
            c.blocked += bl;
            c.members.push(n.handle);
            at = n;
        }
        circuits.push(c);
    }
    {
        let mut lv: Vec<String> = rest.iter().map(|m| m.level.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        lv.sort_by(|x, y| {
            let (ex, ey) = (elev(x) - elev(&facp.level), elev(y) - elev(&facp.level));
            (ex.abs(), ex).partial_cmp(&(ey.abs(), ey)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut ordered: Vec<Node> = Vec::new();
        for l in &lv {
            let on: Vec<&Node> = rest.iter().filter(|m| same_level(&m.level, l)).collect();
            let pts: Vec<(usize, Vec2)> = on.iter().enumerate().map(|(i, m)| (i, m.at)).collect();
            ordered.extend(nn_order(riser(l), &pts).into_iter().filter_map(|i| on.get(i).map(|n| (*n).clone())));
        }
        let per = fr.slc.devices_per_loop.max(1);
        let mut c: Option<Circuit> = None;
        let mut at = facp.clone();
        for n in ordered {
            let (mut runs, mut ft, mut bl) = leg(&at, &n, body, &mut penetrations);
            let full = c.as_ref().is_some_and(|c| c.members.len() >= per || (fr.slc.max_ft > 0.0 && c.ft() + ft > fr.slc.max_ft));
            if full && let Some(done) = c.take() {
                circuits.push(done);
            }
            if c.is_none() {
                slc_n += 1;
                c = Some(new_circuit(format!("{}-SLC{slc_n}", facp.tag), SLC_SYSTEM, "slc", &facp, slc_cable.clone(), "nearest neighbour", RULE_SLC));
                if at.handle != facp.handle {
                    (runs, ft, bl) = leg(&facp, &n, body, &mut penetrations);
                    at = facp.clone();
                }
            }
            if let Some(c) = c.as_mut() {
                c.legs.push((at.clone(), n.clone(), runs, ft));
                c.blocked += bl;
                c.members.push(n.handle);
            }
            at = n;
        }
        if let Some(done) = c.take() {
            circuits.push(done);
        }
    }
    // -- The annunciators' bus from the panel. --
    for (i, a) in annunciators.iter().enumerate() {
        let mut c = new_circuit(
            format!("{}-ANN{}", facp.tag, i + 1),
            SLC_SYSTEM,
            "annunciator",
            &facp,
            cable_of("annunciator", &fr.cables.annunciator),
            "the riser",
            RULE_SLC,
        );
        let (runs, ft, bl) = leg(&facp, a, body, &mut penetrations);
        c.legs.push((facp.clone(), a.clone(), runs, ft));
        c.blocked += bl;
        c.members.push(a.handle);
        c.class_a = false;
        circuits.push(c);
    }
    // -- Class A: each NAC and SLC back to its source. --
    for c in circuits.iter_mut().filter(|c| c.class_a && matches!(c.role, "nac" | "slc")) {
        if let Some(last) = c.legs.last().map(|l| l.1.clone()) {
            let (runs, ft, bl) = leg(&last, &c.source, body, &mut penetrations);
            c.legs.push((last, c.source.clone(), runs, ft));
            c.blocked += bl;
        }
    }

    // -- Into the spec: each leg's pieces as runs, each circuit a tag. --
    let raceway = docs.raceway.clone();
    let mut rows: Vec<Value> = Vec::new();
    let mut ft_by: BTreeMap<String, f64> = BTreeMap::new();
    let mut legs_total = 0usize;
    for c in circuits.iter_mut().take(MAX_CIRCUITS) {
        let rule = super::super::provenance::rule(c.rule);
        let order_basis = match c.order {
            "addresses" => "the documents' circuit and addresses give the order",
            "nearest neighbour" => "no document gives the order: nearest neighbour from the source",
            _ => "the documents give its two ends",
        };
        let basis = rule.basis_for(order_basis);
        for (li, (_, _, runs, _)) in c.legs.iter_mut().enumerate() {
            for (pi, r) in runs.iter_mut().enumerate() {
                if legs_total >= MAX_LEGS {
                    break;
                }
                r.system = c.system.into();
                r.id = format!("{TAG_PREFIX}{}#{}.{}", c.name.replace('#', "-"), li + 1, pi + 1);
                r.size_source = format!("{} {} ({})", c.cable.ty, c.cable.size, fr.cables.source).chars().take(300).collect();
                super::super::provenance::set_run(r, Made::Inferred, basis.clone());
                out.runs.push(r.clone());
            }
            legs_total += 1;
        }
        let ft = c.ft();
        *ft_by.entry(c.system.to_string()).or_default() += ft;
        let mut t = ElementTag {
            id: format!("{TAG_PREFIX}{}", c.name.replace('#', "-")),
            kind: TAG_KIND.into(),
            level: c.source.level.clone(),
            at_ft: [c.source.at.x / upf, c.source.at.y / upf],
            ..ElementTag::default()
        };
        let src_kind = if c.order == "addresses" { "submittal" } else { "rule" };
        t.set("circuit", c.name.clone(), src_kind);
        t.set("system", c.system, "rule");
        t.set("role", c.role, "rule");
        t.set("source", c.source.tag.clone(), "rule");
        t.set("source_handle", c.source.handle.hex(), "computed");
        t.set("class", if c.class_a { "A" } else { "B" }, if c.class_a { "note" } else { "rule" });
        // The cable as the documents' wire legend gives it, else the rules'.
        let legend = docs.cables.get(match c.role {
            "slc" => "slc",
            "annunciator" => "annunciator",
            _ => "nac",
        });
        let cable_src = if legend.is_some() {
            "submittal"
        } else if docs.plenum.is_some() {
            "note"
        } else {
            "rule"
        };
        t.set("cable_type", c.cable.ty.clone(), cable_src);
        t.set("cable_size", c.cable.size.clone(), if legend.is_some() { "submittal" } else { "rule" });
        t.set("cable_item", c.cable.item.clone(), "rule");
        t.set("cable_awg", c.cable.awg.to_string(), if legend.is_some() { "submittal" } else { "rule" });
        let words = legend.map(|l| format!("{} ({})", l.words, fr.documents.legend_source));
        t.set("cable_source", words.unwrap_or_else(|| fr.cables.source.clone()), cable_src);
        t.set("members", c.members.iter().map(|h| h.hex()).collect::<Vec<_>>().join(","), "computed");
        t.set("order", c.order, src_kind);
        t.set("made", Made::Inferred.as_str(), "rule");
        t.set("basis", basis.clone(), "rule");
        t.set("ft", format!("{:.1}", ft), "computed");
        if c.role == "nac" {
            t.set("amps", format!("{:.2}", c.amps), "computed");
            t.set("drop_v", format!("{:.2}", fr.drop(c.amps, ft, c.cable.awg.max(1))), "computed");
            let why: Vec<&str> = c.amps_why.iter().map(String::as_str).collect();
            t.set("amps_basis", format!("{}: {}", why.join(", "), fr.amps_source), "rule");
        }
        if let Some((design, why)) = &c.upsized {
            t.set("design:cable_size", design.clone(), cable_src);
            t.set("correction:cable_size", why.clone(), "rule");
        }
        if let Some((rw, below, words)) = &raceway {
            t.set("raceway", rw.clone(), "note");
            t.set("raceway_below_ft", below.map_or_else(|| "all".to_string(), |b| format!("{b}")), "note");
            t.set("raceway_source", format!("{words} ({})", fr.documents.source), "note");
        }
        out.tags.push(t);
        if rows.len() < MAX_ROWS {
            rows.push(json!({
                "circuit": c.name, "system": c.system, "role": c.role, "source": c.source.tag, "level": c.source.level,
                "devices": c.members.len(), "legs": c.legs.len(), "ft": r1(ft), "amps": if c.role == "nac" { json!(r2(c.amps)) } else { Value::Null },
                "drop_v": if c.role == "nac" { json!(r2(fr.drop(c.amps, ft, c.cable.awg.max(1)))) } else { Value::Null },
                "cable": format!("{} {}", c.cable.ty, c.cable.size), "class": if c.class_a { "A" } else { "B" }, "order": c.order,
                "upsized": c.upsized.as_ref().map(|u| u.1.clone()), "blocked": c.blocked,
            }));
        }
    }
    // A rated wall's penetration is the cable's (no raceway of its own): its firestop for cable.
    for p in penetrations.iter_mut().filter(|p| p.get("wall").is_some()) {
        let rating = p["rating"].as_str().unwrap_or("rated").to_string();
        p["firestop"] = json!(format!("{} for fire alarm cable through a {rating} wall", rl.firestop.item));
        if let Some(o) = p.as_object_mut() {
            o.remove("raceway_in");
        }
    }
    let wired: BTreeSet<Handle> = circuits.iter().flat_map(|c| c.members.iter().copied()).collect();
    let unrouted: Vec<Value> = fa_devs
        .iter()
        .filter(|x| !wired.contains(&x.handle))
        .take(MAX_ROWS)
        .map(|x| json!({"device": x.handle.hex(), "level": x.level, "kind": x.fa_kind}))
        .collect();
    let per_source: Vec<Value> = sources
        .iter()
        .map(|s| {
            let cs: Vec<&Circuit> = circuits.iter().filter(|c| c.role == "nac" && c.source.handle == s.handle).collect();
            let outputs = if s.handle == facp.handle { fr.control_panel.nac_circuits } else { fr.power_supply.nac_circuits };
            json!({"source": s.tag, "level": s.level, "nacs": cs.len(), "outputs": outputs, "amps": r2(cs.iter().map(|c| c.amps).sum()), "over_outputs": cs.len() > outputs})
        })
        .collect();
    out.report = json!({
        "devices": fa_devs.len(),
        "notification": appliances.len(),
        "initiating": fa_devs.iter().filter(|x| x.role == Role::Initiating).count() + modules.len(),
        "control_panel": {"tag": facp.tag, "level": facp.level, "handle": facp.handle.hex()},
        "power_supplies": supplies.iter().map(|s| json!({"tag": s.tag, "level": s.level})).collect::<Vec<_>>(),
        "annunciators": annunciators.len(),
        "modules": modules.len(),
        "circuits": circuits.len(),
        "nacs": circuits.iter().filter(|c| c.role == "nac").count(),
        "slcs": circuits.iter().filter(|c| c.role == "slc").count(),
        "routed_devices": wired.len(),
        "unrouted": unrouted.len(),
        "unrouted_list": unrouted,
        "ft_by_system": ft_by.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>(),
        "by_source": per_source,
        "upsized": circuits.iter().filter(|c| c.upsized.is_some()).count(),
        "penetrations": penetrations.len(),
        "penetration_list": penetrations.into_iter().take(200).collect::<Vec<_>>(),
        "documents": {
            "raceway": raceway.as_ref().map(|(m, below, w)| json!({"method": m, "below_ft": below.map_or_else(|| json!("all"), |b| json!(b)), "words": w})),
            "plenum": docs.plenum, "class_a": docs.class_a,
            "cables": docs.cables.iter().map(|(k, c)| (k.clone(), json!({"cable": format!("{} {}/{}", c.ty, c.awg, c.conductors).trim().to_string(), "words": c.words}))).collect::<serde_json::Map<_, _>>(),
        },
        "circuit_list": rows,
        "rules": {"nac": fr.nac.source, "slc": fr.slc.source, "amps": fr.amps_source, "cable": fr.cables.source, "riser": fr.riser_source},
        "note": "the power supplies and the control panel each need a dedicated 120 V branch circuit (NFPA 72 10.6.5): not routed here",
    });
    out
}

/// One leg's runs of a fire alarm circuit's tag: by leg number, its pieces in order, with the
/// handles it runs between (the runs' `objects`).
pub(crate) fn legs_of<'a>(t: &ElementTag, runs: &'a [MepRun]) -> Vec<(Handle, Handle, Vec<&'a MepRun>)> {
    let prefix = format!("{}#", t.id);
    let mut by: BTreeMap<(u32, u32), &MepRun> = BTreeMap::new();
    for r in runs.iter().filter(|r| is_system(&r.system) && r.id.starts_with(&prefix)).take(MAX_LEGS) {
        let Some((l, p)) = r.id.get(prefix.len()..).and_then(|x| x.split_once('.')) else { continue };
        let (Ok(l), Ok(p)) = (l.parse::<u32>(), p.parse::<u32>()) else { continue };
        by.insert((l, p), r);
    }
    let mut out: Vec<(Handle, Handle, Vec<&MepRun>)> = Vec::new();
    let mut cur: Option<u32> = None;
    for ((l, _), r) in by {
        let ends = |i: usize| r.objects.get(i).and_then(|o| Handle::parse_hex(o.trim()));
        if cur != Some(l) {
            let (Some(a), Some(bh)) = (ends(0), ends(1)) else { continue };
            out.push((a, bh, Vec::new()));
            cur = Some(l);
        }
        if let Some(last) = out.last_mut() {
            last.2.push(r);
        }
    }
    out
}

/// A path's pieces below `z` (absolute): each stretch of it under that height, as its own path.
pub(crate) fn below(path: &[Vec3], z: f64) -> Vec<Vec<Vec3>> {
    let mut out: Vec<Vec<Vec3>> = Vec::new();
    let mut cur: Vec<Vec3> = Vec::new();
    for w in path.windows(2).take(20_000) {
        let [a, b] = w else { continue };
        let (a, b) = (*a, *b);
        let (ina, inb) = (a.z < z - 1e-9, b.z < z - 1e-9);
        let cut = |p: Vec3, q: Vec3| {
            let t = if (q.z - p.z).abs() > 1e-12 { ((z - p.z) / (q.z - p.z)).clamp(0.0, 1.0) } else { 0.0 };
            p + (q - p) * t
        };
        match (ina, inb) {
            (true, true) => {
                if cur.is_empty() {
                    cur.push(a);
                }
                cur.push(b);
            }
            (true, false) => {
                if cur.is_empty() {
                    cur.push(a);
                }
                cur.push(cut(a, b));
                out.push(std::mem::take(&mut cur));
            }
            (false, true) => {
                cur = vec![cut(a, b), b];
            }
            (false, false) => {}
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out.retain(|p| p.len() >= 2 && p.windows(2).any(|w| matches!(w, [a, b] if super::super::mepatoms::dist(*a, *b) > 1e-6)));
    out
}

#[cfg(test)]
#[path = "conduitroute_fa_tests.rs"]
mod tests;
