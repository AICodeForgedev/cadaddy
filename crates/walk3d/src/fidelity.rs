//! Fidelity levels: which built elements the 3D view draws at which zoom, as the plan viewer's
//! tiled zoom does. Far out the view holds the building (shell, roof, rooftop and big
//! equipment, mains); closer in each floor (walls, openings, branch runs, panels); in a room its
//! devices, fixtures and luminaires; up close the connectors and fasteners.
//!
//! What an element's level is comes from rules data: the starter rules
//! (`fidelity_starter.json`) and a project's own rules before them, kept in the building spec
//! (key [`KEY`]). Each rule matches what the build made (trade, category, part, kind, words in
//! its type or name, its level) and its size (nominal inches, or its largest extent in feet).
//! How far the view is zoomed (screen pixels per foot) picks the level shown, with hysteresis so
//! a zoom resting on a threshold doesn't flicker.
//!
//! Fidelity changes only what is drawn: counts, takeoffs and validation read the whole model.

use serde::{Deserialize, Serialize};

/// The key of a project's own fidelity settings in the building spec.
pub const KEY: &str = "fidelity";

/// Most project rules kept (a hostile spec can't make every element run thousands of rules).
pub const MAX_RULES: usize = 500;

/// Most force-shown selections kept.
pub const MAX_SHOW: usize = 64;

/// How close the view must be for an element to draw: each level shows itself and the ones
/// before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fidelity {
    /// Shell, slabs, roof, rooftop equipment, main ducts and pipes, big gear.
    #[default]
    Building,
    /// Walls, doors and windows, branch ducts, mid-size pipes and conduit runs, panels, terminals.
    Floor,
    /// Devices, plumbing fixtures, luminaires, boxes, cable runs, framing and drywall.
    Room,
    /// Couplings, straps, rings, fasteners and sheet seams.
    Detail,
}

impl Fidelity {
    pub const ALL: [Fidelity; 4] = [Fidelity::Building, Fidelity::Floor, Fidelity::Room, Fidelity::Detail];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Fidelity {
        match i {
            0 => Fidelity::Building,
            1 => Fidelity::Floor,
            2 => Fidelity::Room,
            _ => Fidelity::Detail,
        }
    }

    /// Its name in rules and commands (`building`, `floor`, `room`, `detail`).
    pub fn name(self) -> &'static str {
        match self {
            Fidelity::Building => "building",
            Fidelity::Floor => "floor",
            Fidelity::Room => "room",
            Fidelity::Detail => "detail",
        }
    }

    /// Its name for people (translated by the UI).
    pub fn label(self) -> &'static str {
        match self {
            Fidelity::Building => "Building",
            Fidelity::Floor => "Floor",
            Fidelity::Room => "Room",
            Fidelity::Detail => "Detail",
        }
    }

    pub fn parse(s: &str) -> Option<Fidelity> {
        Fidelity::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(s.trim()))
    }

    /// The next coarser level (none above Building).
    pub fn coarser(self) -> Option<Fidelity> {
        self.index().checked_sub(1).map(Fidelity::from_index)
    }
}

/// How an element finer than the level shown draws at the next coarser level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Far {
    /// Not at all (a room's devices from a floor away).
    #[default]
    None,
    /// Its system's runs on its level merged into one set of centre lines.
    Line,
}

/// One rule: an element matching every condition it sets is at `tier`. Lists match when any
/// entry does (case-insensitive); empty lists and unset bounds match everything.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub tier: Fidelity,
    /// Trade keys (`conduit`, `pipe_cold`, `devices`…) or trade groups (`Electrical`…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trade: Vec<String>,
    /// Categories (`Walls`, `Pipe Fittings`…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub category: Vec<String>,
    /// What the build made it as (`wall`, `roof`, `device`, `mep`…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub part: Vec<String>,
    /// An MEP element's kind (`duct`, `pipe`, `conduit`, `cable`, `equipment`…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kind: Vec<String>,
    /// An MEP element's piece, by prefix (`segment`, `fitting`, `fitting:coupling`, `terminal`…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub piece: Vec<String>,
    /// Words or phrases found in its type, name or system (whole words).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// Words found in its level's name (`roof`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub level_words: Vec<String>,
    /// Its nominal size (a pipe's diameter, a duct's larger side), inches; unknown never matches
    /// a bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size_in: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_size_in: Option<f64>,
    /// Its largest extent, feet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_extent_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_extent_ft: Option<f64>,
    pub far: Far,
    /// Why the rule is (its source).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// What a rule sees of an element.
#[derive(Clone, Debug, Default)]
pub struct Element<'a> {
    pub trade: &'a str,
    pub group: &'a str,
    pub category: &'a str,
    pub ty: &'a str,
    pub part: &'a str,
    pub kind: &'a str,
    pub piece: &'a str,
    pub system: &'a str,
    pub name: &'a str,
    pub level: &'a str,
    pub size_in: Option<f64>,
    pub extent_ft: Option<f64>,
}

fn any_eq(list: &[String], v: &str) -> bool {
    list.is_empty() || list.iter().any(|x| x.trim().eq_ignore_ascii_case(v.trim()))
}

/// Whether `phrase` is in `text` as whole words.
fn has_phrase(text: &str, phrase: &str) -> bool {
    let low = text.to_ascii_lowercase();
    let p = phrase.trim().to_ascii_lowercase();
    if p.is_empty() {
        return false;
    }
    let bytes = low.as_bytes();
    let mut from = 0;
    while let Some(i) = low.get(from..).and_then(|s| s.find(&p)) {
        let at = from + i;
        let end = at + p.len();
        let before = at == 0 || !bytes.get(at - 1).is_some_and(u8::is_ascii_alphanumeric);
        let after = !bytes.get(end).is_some_and(u8::is_ascii_alphanumeric);
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

fn within(v: Option<f64>, lo: Option<f64>, hi: Option<f64>) -> bool {
    if lo.is_none() && hi.is_none() {
        return true;
    }
    let Some(v) = v.filter(|v| v.is_finite()) else { return false };
    lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v < h)
}

impl Rule {
    pub fn matches(&self, e: &Element) -> bool {
        (self.trade.is_empty() || any_eq(&self.trade, e.trade) || any_eq(&self.trade, e.group))
            && any_eq(&self.category, e.category)
            && any_eq(&self.part, e.part)
            && any_eq(&self.kind, e.kind)
            && (self.piece.is_empty()
                || self.piece.iter().any(|p| !p.trim().is_empty() && e.piece.to_ascii_lowercase().starts_with(&p.trim().to_ascii_lowercase())))
            && (self.words.is_empty() || self.words.iter().any(|w| [e.ty, e.name, e.system].iter().any(|t| has_phrase(t, w))))
            && (self.level_words.is_empty() || self.level_words.iter().any(|w| has_phrase(e.level, w)))
            && within(e.size_in, self.min_size_in, self.max_size_in)
            && within(e.extent_ft, self.min_extent_ft, self.max_extent_ft)
    }
}

/// Zoom thresholds: the level shown is the finest whose threshold one foot on screen reaches
/// (screen pixels per foot), with `hysteresis` (a share) either side so it doesn't flicker.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Zoom {
    pub floor_px_per_ft: f64,
    pub room_px_per_ft: f64,
    pub detail_px_per_ft: f64,
    pub hysteresis: f64,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { floor_px_per_ft: 8.0, room_px_per_ft: 30.0, detail_px_per_ft: 200.0, hysteresis: 0.15 }
    }
}

/// A project's changes to the zoom thresholds (unset: the starter's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoomPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_px_per_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_px_per_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_px_per_ft: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hysteresis: Option<f64>,
}

/// Elements shown at every zoom whatever their level ("show all outlets on level 2"), drawn as
/// markers. Unset fields match everything; at least one must be set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Show {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Words in its type or name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

impl Show {
    pub fn is_empty(&self) -> bool {
        [&self.trade, &self.category, &self.words, &self.level].iter().all(|f| f.as_deref().is_none_or(|s| s.trim().is_empty()))
    }

    pub fn matches(&self, e: &Element) -> bool {
        fn set(f: &Option<String>) -> Option<&str> {
            f.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        !self.is_empty()
            && set(&self.trade).is_none_or(|t| t.eq_ignore_ascii_case(e.trade) || t.eq_ignore_ascii_case(e.group))
            && set(&self.category).is_none_or(|c| c.eq_ignore_ascii_case(e.category))
            && set(&self.words).is_none_or(|w| has_phrase(e.ty, w) || has_phrase(e.name, w) || has_phrase(e.category, w))
            && set(&self.level).is_none_or(|l| l.eq_ignore_ascii_case(e.level))
    }
}

/// A project's own fidelity settings (the building spec's [`KEY`]): its rules go before the
/// starter's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectFidelity {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: Option<ZoomPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_tier: Option<Fidelity>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<Show>,
}

impl ProjectFidelity {
    pub fn is_empty(&self) -> bool {
        self == &ProjectFidelity::default()
    }
}

/// The rules in force.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FidelityRules {
    /// Where the rules come from.
    pub source: String,
    pub zoom: Zoom,
    /// The level of a built element no rule matches.
    pub default_tier: Fidelity,
    pub rules: Vec<Rule>,
    /// How many of `rules` (from the front) are the project's.
    #[serde(skip)]
    pub project_rules: usize,
    /// Force-shown selections.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<Show>,
    /// The colour force-shown elements and their markers draw in.
    pub marker_rgb: [u8; 3],
}

impl Default for FidelityRules {
    fn default() -> Self {
        FidelityRules {
            source: String::new(),
            zoom: Zoom::default(),
            default_tier: Fidelity::Room,
            rules: Vec::new(),
            project_rules: 0,
            show: Vec::new(),
            marker_rgb: [255, 0, 200],
        }
    }
}

const STARTER: &str = include_str!("fidelity_starter.json");

impl FidelityRules {
    /// The rules CADaddy ships.
    pub fn starter() -> FidelityRules {
        serde_json::from_str(STARTER).unwrap_or_default()
    }

    /// The starter rules with a project's settings over them.
    pub fn with_project(p: Option<&ProjectFidelity>) -> FidelityRules {
        let mut r = FidelityRules::starter();
        let Some(p) = p else { return r };
        let own: Vec<Rule> = p.rules.iter().take(MAX_RULES).cloned().collect();
        r.project_rules = own.len();
        r.rules.splice(0..0, own);
        if let Some(z) = &p.zoom {
            let ok = |v: Option<f64>, d: f64| v.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d);
            r.zoom.floor_px_per_ft = ok(z.floor_px_per_ft, r.zoom.floor_px_per_ft);
            r.zoom.room_px_per_ft = ok(z.room_px_per_ft, r.zoom.room_px_per_ft);
            r.zoom.detail_px_per_ft = ok(z.detail_px_per_ft, r.zoom.detail_px_per_ft);
            r.zoom.hysteresis = z.hysteresis.filter(|v| v.is_finite()).map_or(r.zoom.hysteresis, |v| v.clamp(0.0, 0.9));
        }
        if let Some(t) = p.default_tier {
            r.default_tier = t;
        }
        r.show = p.show.iter().filter(|s| !s.is_empty()).take(MAX_SHOW).cloned().collect();
        if r.project_rules > 0 || p.zoom.is_some() || p.default_tier.is_some() {
            r.source = format!("{} + project rules (building spec \"{KEY}\")", r.source);
        }
        r
    }

    /// The rules for a building spec's JSON text (the starter's when it has none).
    pub fn of_spec(spec: Option<&str>) -> FidelityRules {
        FidelityRules::with_project(project_of(spec).as_ref())
    }

    /// An element's level, and the rule that set it (`None`: the default).
    pub fn assign(&self, e: &Element) -> (Fidelity, Option<usize>) {
        match self.rules.iter().position(|r| r.matches(e)) {
            Some(i) => (self.rules.get(i).map_or(self.default_tier, |r| r.tier), Some(i)),
            None => (self.default_tier, None),
        }
    }

    /// How an element that rule `rule` placed draws one level coarser.
    pub fn far(&self, rule: Option<usize>) -> Far {
        rule.and_then(|i| self.rules.get(i)).map_or(Far::None, |r| r.far)
    }

    /// Whether a force-shown selection takes the element.
    pub fn shown(&self, e: &Element) -> bool {
        self.show.iter().any(|s| s.matches(e))
    }

    /// The level a view at `px_per_ft` screen pixels a foot shows, coming from `current`: a
    /// finer level once its threshold is passed by the hysteresis, a coarser one once it is
    /// dropped below by it.
    pub fn auto_level(&self, px_per_ft: f64, current: Fidelity) -> Fidelity {
        if !px_per_ft.is_finite() || px_per_ft <= 0.0 {
            return current;
        }
        let z = &self.zoom;
        let h = z.hysteresis.clamp(0.0, 0.9);
        for (level, thr) in [(Fidelity::Detail, z.detail_px_per_ft), (Fidelity::Room, z.room_px_per_ft), (Fidelity::Floor, z.floor_px_per_ft)] {
            let at = if level <= current { thr * (1.0 - h) } else { thr * (1.0 + h) };
            if px_per_ft >= at {
                return level;
            }
        }
        Fidelity::Building
    }
}

/// A project's settings in a building spec's JSON text.
pub fn project_of(spec: Option<&str>) -> Option<ProjectFidelity> {
    let v: serde_json::Value = serde_json::from_str(spec?).ok()?;
    serde_json::from_value(v.get(KEY)?.clone()).ok()
}

/// A size tag's nominal inches: the largest of its dimensions (`12x10` → 12, `1-1/2"` → 1.5,
/// `3/4` → 0.75, `8"Ø` → 8).
pub fn size_inches(s: &str) -> Option<f64> {
    let one = |p: &str| -> Option<f64> {
        let t: String = p.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '/' | '-' | ' ')).collect();
        let t = t.trim().trim_matches('-').trim();
        if t.is_empty() {
            return None;
        }
        let frac = |f: &str| -> Option<f64> {
            match f.split_once('/') {
                Some((a, b)) => {
                    let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                    (b != 0.0).then(|| a / b)
                }
                None => f.trim().parse().ok(),
            }
        };
        // `1-1/2` or `1 1/2`: whole and fraction.
        let v = match t.split_once(['-', ' ']) {
            Some((w, f)) if f.contains('/') => w.trim().parse::<f64>().ok()? + frac(f)?,
            _ => frac(t)?,
        };
        v.is_finite().then_some(v)
    };
    s.split(['x', 'X', '×', '*']).filter_map(one).filter(|v| *v > 0.0).reduce(f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el<'a>(trade: &'a str, category: &'a str, part: &'a str) -> Element<'a> {
        Element { trade, category, part, ..Element::default() }
    }

    #[test]
    fn the_starter_rules_load_and_place_each_kind() {
        let r = FidelityRules::starter();
        assert!(!r.rules.is_empty(), "the starter rules parse");
        assert!(!r.source.is_empty());
        let tier = |e: &Element| r.assign(e).0;
        // The shell and the roof: the building.
        assert_eq!(tier(&el("architecture", "Roofs", "roof")), Fidelity::Building);
        assert_eq!(tier(&el("architecture", "Floors", "floor")), Fidelity::Building);
        // Walls and doors: the floor.
        assert_eq!(tier(&el("architecture", "Walls", "wall")), Fidelity::Floor);
        assert_eq!(tier(&el("openings", "Doors", "door")), Fidelity::Floor);
        // Devices, luminaires, fixtures: the room.
        assert_eq!(tier(&el("devices", "Devices", "device")), Fidelity::Room);
        assert_eq!(tier(&el("lights", "Light Fixtures", "light")), Fidelity::Room);
        assert_eq!(tier(&el("fixtures", "Plumbing Fixtures", "mep")), Fidelity::Room);
        // Pipes by size: a main at the building, a branch at the floor, a small one in the room.
        let pipe = |size: f64| Element {
            trade: "pipe_cold",
            category: "Pipe",
            part: "mep",
            kind: "pipe",
            piece: "segment",
            size_in: Some(size),
            ..Element::default()
        };
        assert_eq!(tier(&pipe(4.0)), Fidelity::Building);
        assert_eq!(tier(&pipe(1.5)), Fidelity::Floor);
        assert_eq!(tier(&pipe(0.5)), Fidelity::Room);
        // Ducts: mains and branches.
        let duct = |size: f64| Element {
            trade: "duct_supply",
            category: "Duct",
            part: "mep",
            kind: "duct",
            piece: "segment",
            size_in: Some(size),
            ..Element::default()
        };
        assert_eq!(tier(&duct(24.0)), Fidelity::Building);
        assert_eq!(tier(&duct(8.0)), Fidelity::Floor);
        // Conduit as runs on the floor; its couplings at detail.
        let conduit = Element {
            trade: "conduit",
            category: "Conduit",
            part: "mep",
            kind: "conduit",
            piece: "segment",
            size_in: Some(0.75),
            ..Element::default()
        };
        assert_eq!(tier(&conduit), Fidelity::Floor);
        let coupling = Element { piece: "fitting:coupling", category: "Conduit Fittings", ..conduit.clone() };
        assert_eq!(tier(&coupling), Fidelity::Detail);
        // Equipment: big gear and fans at the building, rooftop units, panels on the floor.
        let eq = |ty: &'static str, level: &'static str| Element {
            trade: "equipment",
            category: "Equipment",
            part: "equipment",
            ty,
            level,
            ..Element::default()
        };
        assert_eq!(tier(&eq("Switchboard", "Level 1")), Fidelity::Building);
        assert_eq!(tier(&eq("Exhaust Fan", "Level 3")), Fidelity::Building);
        assert_eq!(tier(&eq("Unit", "Roof")), Fidelity::Building);
        assert_eq!(tier(&eq("Panelboard", "Level 2")), Fidelity::Floor);
        // A big unknown box is big gear by its size.
        let big = Element { extent_ft: Some(12.0), ..eq("Unit", "Level 2") };
        assert_eq!(tier(&big), Fidelity::Building);
    }

    #[test]
    fn project_rules_go_first_and_zoom_patches_apply() {
        let p: ProjectFidelity = serde_json::from_value(serde_json::json!({
            "rules": [{"tier": "building", "trade": ["devices"], "words": ["panic button"], "why": "owner wants them seen"}],
            "zoom": {"room_px_per_ft": 50.0, "hysteresis": 5.0},
            "default_tier": "detail",
            "show": [{"trade": "devices", "level": "Level 2"}, {}]
        }))
        .unwrap();
        let r = FidelityRules::with_project(Some(&p));
        assert_eq!(r.project_rules, 1);
        let button = Element { trade: "devices", category: "Devices", part: "device", ty: "Panic Button", ..Element::default() };
        assert_eq!(r.assign(&button), (Fidelity::Building, Some(0)));
        let outlet = Element { ty: "Duplex", ..button.clone() };
        assert_eq!(r.assign(&outlet).0, Fidelity::Room);
        assert_eq!(r.zoom.room_px_per_ft, 50.0);
        assert_eq!(r.zoom.hysteresis, 0.9, "clamped");
        assert_eq!(r.default_tier, Fidelity::Detail);
        assert_eq!(r.assign(&el("none", "Things", "thing")).0, Fidelity::Detail);
        // The empty selection is dropped; the other picks level 2's devices only.
        assert_eq!(r.show.len(), 1);
        assert!(r.shown(&Element { level: "Level 2", ..outlet.clone() }));
        assert!(!r.shown(&Element { level: "Level 3", ..outlet.clone() }));
        // Read back from a building spec.
        let spec = serde_json::json!({ KEY: p }).to_string();
        assert_eq!(FidelityRules::of_spec(Some(&spec)), r);
        assert_eq!(FidelityRules::of_spec(None), FidelityRules::starter());
    }

    #[test]
    fn zoom_picks_a_level_with_hysteresis() {
        let r = FidelityRules::starter();
        let z = r.zoom;
        let mut now = Fidelity::Building;
        // Zooming in passes each threshold only past the hysteresis.
        now = r.auto_level(z.floor_px_per_ft * 1.05, now);
        assert_eq!(now, Fidelity::Building, "just past the threshold isn't enough");
        now = r.auto_level(z.floor_px_per_ft * 1.3, now);
        assert_eq!(now, Fidelity::Floor);
        // Resting a little under the threshold keeps the level (no flicker).
        assert_eq!(r.auto_level(z.floor_px_per_ft * 0.95, now), Fidelity::Floor);
        assert_eq!(r.auto_level(z.floor_px_per_ft * 0.7, now), Fidelity::Building);
        // Far in: straight to detail; far out: straight to building.
        assert_eq!(r.auto_level(z.detail_px_per_ft * 2.0, Fidelity::Building), Fidelity::Detail);
        assert_eq!(r.auto_level(z.floor_px_per_ft * 0.1, Fidelity::Detail), Fidelity::Building);
        assert_eq!(r.auto_level(f64::NAN, Fidelity::Room), Fidelity::Room);
        // Every level in order as the view nears.
        let mut seen = vec![];
        let mut l = Fidelity::Building;
        let mut px = 1.0;
        while px < 1000.0 {
            l = r.auto_level(px, l);
            if seen.last() != Some(&l) {
                seen.push(l);
            }
            px *= 1.1;
        }
        assert_eq!(seen, Fidelity::ALL.to_vec());
    }

    #[test]
    fn sizes_and_words() {
        assert_eq!(size_inches("12x10"), Some(12.0));
        assert_eq!(size_inches("1-1/2\""), Some(1.5));
        assert_eq!(size_inches("3/4"), Some(0.75));
        assert_eq!(size_inches("8\"Ø"), Some(8.0));
        assert_eq!(size_inches("0.75"), Some(0.75));
        assert_eq!(size_inches("1 1/4"), Some(1.25));
        assert_eq!(size_inches(""), None);
        assert_eq!(size_inches("1/0"), None);
        assert!(has_phrase("EF-1 Exhaust Fan", "exhaust fan"));
        assert!(has_phrase("EF-1", "ef"));
        assert!(!has_phrase("Reflector", "ef"));
        assert!(!has_phrase("anything", " "));
        assert_eq!(Fidelity::parse(" Room "), Some(Fidelity::Room));
        assert_eq!(Fidelity::Room.coarser(), Some(Fidelity::Floor));
        assert_eq!(Fidelity::Building.coarser(), None);
        assert!(Fidelity::ALL.iter().all(|f| Fidelity::from_index(f.index()) == *f));
    }
}
