//! The BIM view: the built model drawn the way BIM tools show a building — shaded with edges,
//! every trade in its own colour from one editable table, the architecture ghosted so the MEP in
//! front reads, the sheet's linework (title block, plan lines) left out of 3D.
//!
//! - `bim.look {on?, ghost?, ghost_opacity?, sheet?, level?}`: the view's settings.
//! - `bim.trades {colors?, visible?, reset?}`: the trade colour table.
//! - `bim.tree {level?, group?, category?, type?, path?, visible?, select?, zoom?}`: the model
//!   tree, Level ▸ Trade ▸ Category ▸ Type ▸ element; a branch shown or hidden, selected and
//!   zoomed to.
//! - `bim.element {handle?, open_source?}`: one element's properties (type, size, level,
//!   system, circuit, quantity, source sheet and spot, linked issues) and where it is (plan
//!   point, bottom and top, height above its floor, host wall and face); `open_source` goes to
//!   it on the sheet.
//!
//! The settings live in the building spec (`bim_view`). [`display`] is what the 3D view and
//! `model.render` draw: a drawing holding only the built model (the objects shared, not
//! copied), its layers coloured by trade, the ghosted trades see-through.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cadaddy_doc::color::Color;
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::validate::{ModelWall, WallSpan};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The key of the view's settings in the building spec.
const KEY: &str = "bim_view";
/// The see-through material the ghosted trades get in the view.
pub const GHOST_MATERIAL: &str = "BIM Ghost";
/// Default ghost opacity (architecture at a fifth).
pub const GHOST_OPACITY: f64 = 0.2;
/// Most objects the tree and the view look at.
const MAX_SCAN: usize = 2_000_000;
/// Most elements a tree lists under one type, and in all.
const MAX_LIST: usize = 1_000;
const MAX_LISTED: usize = 20_000;
/// Most tree branches kept hidden, and parts of a path.
const MAX_PATHS: usize = 500;
const MAX_PATH_PARTS: usize = 4;

/// One row of the trade colour table.
#[derive(Clone, Copy, Debug)]
pub struct Trade {
    pub key: &'static str,
    pub label: &'static str,
    /// The tree's trade: Architecture, Structure, Mechanical, Plumbing, Fire Protection,
    /// Electrical, Site.
    pub group: &'static str,
    pub rgb: [u8; 3],
    /// Drawn see-through when the view ghosts the shell.
    pub ghost: bool,
}

const fn t(key: &'static str, label: &'static str, group: &'static str, rgb: [u8; 3], ghost: bool) -> Trade {
    Trade { key, label, group, rgb, ghost }
}

/// The trades and their default colours: architecture neutral, structure tan, supply duct
/// blue, return and exhaust green, sanitary magenta, vent purple, cold water blue, hot water
/// orange-red, gas yellow, fire protection red, conduit and tray orange-yellow, equipment light
/// grey.
pub const TRADES: [Trade; 24] = [
    t("architecture", "Architecture", "Architecture", [214, 212, 206], true),
    t("openings", "Doors and Windows", "Architecture", [176, 198, 212], true),
    t("structure", "Structure", "Structure", [198, 168, 124], true),
    t("site", "Site", "Site", [176, 188, 164], true),
    t("duct_supply", "Supply Air Duct", "Mechanical", [28, 144, 232], false),
    t("duct_return", "Return Air Duct", "Mechanical", [40, 168, 92], false),
    t("duct_exhaust", "Exhaust Air Duct", "Mechanical", [122, 186, 58], false),
    t("duct_outside", "Outside Air Duct", "Mechanical", [0, 188, 188], false),
    t("duct", "Duct", "Mechanical", [92, 160, 212], false),
    t("terminals", "Air Terminals", "Mechanical", [150, 200, 236], false),
    t("equipment", "Equipment", "Mechanical", [190, 194, 200], false),
    t("pipe_cold", "Domestic Cold Water", "Plumbing", [30, 100, 222], false),
    t("pipe_hot", "Domestic Hot Water", "Plumbing", [236, 92, 38], false),
    t("pipe_sanitary", "Sanitary", "Plumbing", [198, 40, 170], false),
    t("pipe_vent", "Vent", "Plumbing", [138, 80, 202], false),
    t("pipe_storm", "Storm", "Plumbing", [84, 122, 150], false),
    t("pipe_gas", "Gas", "Plumbing", [232, 200, 20], false),
    t("pipe", "Piping", "Plumbing", [40, 160, 92], false),
    t("fixtures", "Plumbing Fixtures", "Plumbing", [236, 240, 246], false),
    t("fire", "Fire Protection", "Fire Protection", [222, 30, 30], false),
    t("conduit", "Conduit", "Electrical", [246, 166, 24], false),
    t("tray", "Cable Tray", "Electrical", [236, 128, 20], false),
    t("devices", "Electrical Devices", "Electrical", [250, 212, 40], false),
    t("lights", "Light Fixtures", "Electrical", [255, 244, 196], false),
];

/// The trade row of `key`.
pub fn trade(key: &str) -> Option<&'static Trade> {
    TRADES.iter().find(|t| t.key.eq_ignore_ascii_case(key))
}

/// The view's settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The BIM view is off: 3D shows the drawing as it is.
    pub off: bool,
    /// Ghost the shell (`None`: when the model has MEP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghost: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghost_opacity: Option<f64>,
    /// Draw the sheet's own linework in 3D too.
    pub sheet: bool,
    /// The one level shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The table's colours changed from the defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, [u8; 3]>,
    /// The trades hidden.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden: BTreeSet<String>,
    /// Tree branches hidden: [level, trade?, category?, type?].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_paths: Vec<Vec<String>>,
}

impl Look {
    /// The settings kept in `d`'s building spec (the defaults when none are).
    pub fn of(d: &Drawing) -> Look {
        d.building_spec
            .as_deref()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .and_then(|v| v.get(KEY).cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// The colour of trade `key` (its own, else the default).
    pub fn rgb(&self, key: &str) -> [u8; 3] {
        self.colors.get(key).copied().or_else(|| trade(key).map(|t| t.rgb)).unwrap_or([200, 200, 200])
    }

    pub fn opacity(&self) -> f64 {
        self.ghost_opacity.filter(|v| v.is_finite()).unwrap_or(GHOST_OPACITY).clamp(0.02, 1.0)
    }

    fn path_hidden(&self, c: &Class) -> bool {
        let parts = [c.level.as_str(), c.group, c.category.as_str(), c.ty.as_str()];
        self.hidden_paths.iter().any(|p| !p.is_empty() && p.iter().zip(parts).all(|(a, b)| part_is(a, b)))
    }

    /// Whether the view shows an element of class `c`.
    pub fn shows(&self, c: &Class) -> bool {
        !self.hidden.contains(c.key) && self.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&c.level)) && !self.path_hidden(c)
    }
}

fn save(s: &mut Session, look: &Look) -> Result<()> {
    let d = s.doc()?;
    let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
    let val = serde_json::to_value(look).unwrap_or(Value::Null);
    let val = if val.as_object().is_some_and(|o| o.is_empty()) { Value::Null } else { val };
    if spec.get(KEY).cloned().unwrap_or(Value::Null) == val {
        return Ok(());
    }
    if let Some(o) = spec.as_object_mut() {
        if val.is_null() {
            o.remove(KEY);
        } else {
            o.insert(KEY.into(), val);
        }
    }
    s.doc_mut()?.building_spec = Some(spec.to_string());
    Ok(())
}

// ─── What an element is ──────────────────────────────────────────────────────

/// A built element's place in the model: trade, tree trade, category, type, level.
#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub key: &'static str,
    pub group: &'static str,
    pub category: String,
    pub ty: String,
    pub level: String,
}

/// The tags after the build's marker: `[part, level, source, more…]`.
pub(crate) fn made_tags(e: &Entity) -> Option<&[cadaddy_doc::RawTag]> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    let rest = x.get(at + 1..)?;
    // Up to the next application's tags.
    let end = rest.iter().position(|t| t.code == 1001).unwrap_or(rest.len());
    rest.get(..end)
}

/// The name a build gave an element, the tag after its source (a rooftop unit's `RTU-1`).
pub fn made_name(e: &Entity) -> Option<&str> {
    made_tags(e)?.get(3).filter(|t| t.code == 1000 && !t.value.contains(':')).map(|t| t.value.as_str())
}

/// The value of tag `key:` among an element's build tags.
pub fn made_tag<'a>(e: &'a Entity, key: &str) -> Option<&'a str> {
    made_tags(e)?.iter().filter(|t| t.code == 1000).find_map(|t| {
        let (k, v) = t.value.split_once(':')?;
        k.eq_ignore_ascii_case(key).then_some(v.trim())
    })
}

/// The plan object a built element was made from.
pub fn made_source(e: &Entity) -> Option<Handle> {
    let h = made_tags(e)?.get(2).and_then(|t| u64::from_str_radix(t.value.trim(), 16).ok())?;
    (h != 0).then_some(Handle(h))
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// The trade of a pipe system, by its name.
pub fn pipe_trade(system: &str) -> &'static str {
    let low = system.to_ascii_lowercase();
    let w = words(system);
    let has = |xs: &[&str]| xs.iter().any(|x| low.contains(x));
    let word = |xs: &[&str]| w.iter().any(|t| xs.contains(&t.as_str()));
    if has(&["fire", "sprink", "standpipe"]) || word(&["fp", "fs", "sp"]) {
        "fire"
    } else if has(&["sanitary", "sewer", "waste", "soil", "grease"]) || word(&["san", "ss", "sw", "w", "gw"]) {
        "pipe_sanitary"
    } else if has(&["vent"]) || word(&["v", "vtr"]) {
        "pipe_vent"
    } else if has(&["storm", "rain", "drain"]) || word(&["st", "rd", "sd", "od"]) {
        "pipe_storm"
    } else if has(&["gas", "propane"]) || word(&["g", "ng", "lp"]) {
        "pipe_gas"
    } else if has(&["hot", "recirc"]) || word(&["hw", "dhw", "hwr", "hwc", "hhw", "dhwr"]) {
        "pipe_hot"
    } else if has(&["cold", "domestic", "potable", "water"]) || word(&["cw", "dcw", "dw"]) {
        "pipe_cold"
    } else {
        "pipe"
    }
}

/// The trade of a duct system, by its duct type or name.
pub fn duct_trade(duct_type: Option<&str>, system: &str) -> &'static str {
    match duct_type.map(str::to_ascii_lowercase).as_deref() {
        Some("supply") => return "duct_supply",
        Some("return") => return "duct_return",
        Some("exhaust") => return "duct_exhaust",
        Some("outside_air") => return "duct_outside",
        _ => {}
    }
    let low = system.to_ascii_lowercase();
    let w = words(system);
    let word = |xs: &[&str]| w.iter().any(|t| xs.contains(&t.as_str()));
    if low.contains("supply") || word(&["sa", "s"]) {
        "duct_supply"
    } else if low.contains("return") || word(&["ra", "r"]) {
        "duct_return"
    } else if low.contains("exhaust") || word(&["ea", "ex", "e"]) {
        "duct_exhaust"
    } else if low.contains("outside") || low.contains("fresh") || word(&["oa"]) {
        "duct_outside"
    } else {
        "duct"
    }
}

fn title(s: &str) -> String {
    let mut out = String::new();
    for (k, w) in s.split(['_', ' ']).filter(|w| !w.is_empty()).enumerate() {
        if k > 0 {
            out.push(' ');
        }
        let mut c = w.chars();
        if let Some(f) = c.next() {
            out.extend(f.to_uppercase());
            out.push_str(c.as_str());
        }
    }
    out
}

/// The block name of an element's source, when it is a block reference.
fn source_block(d: &Drawing, e: &Entity) -> Option<String> {
    let src = d.model.get(made_source(e)?)?;
    match &src.kind {
        EntityKind::Insert(i) => Some(i.block.clone()),
        _ => None,
    }
}

/// What a built element is; `None` for anything the build didn't make (the sheet's lines).
pub fn classify(d: &Drawing, e: &Entity) -> Option<Class> {
    let part = super::wallgen::made_as(e)?;
    let level = super::bspec::made_level(e).unwrap_or_default();
    let layer = e.common.layer.to_ascii_uppercase();
    let size = made_tag(e, "size");
    let own_type = made_tag(e, "type").map(str::to_string);
    let mk = |key: &'static str, category: &str, ty: String| {
        let group = trade(key).map_or("Architecture", |t| t.group);
        Some(Class { key, group, category: category.to_string(), ty, level: level.clone() })
    };
    if part.eq_ignore_ascii_case("mep") {
        let system = made_tag(e, "system").unwrap_or("");
        let kind = made_tag(e, "kind").unwrap_or("").to_ascii_lowercase();
        let mpart = made_tag(e, "part").unwrap_or("segment").to_ascii_lowercase();
        let sized = |base: &str| match size {
            Some(z) if !z.is_empty() => format!("{base} {z}"),
            _ => base.to_string(),
        };
        let base = if system.is_empty() { title(&kind) } else { system.to_string() };
        let fitting = mpart.starts_with("fitting");
        let (key, noun) = match kind.as_str() {
            "duct" => (duct_trade(made_tag(e, "duct_type"), system), "Duct"),
            "pipe" => (pipe_trade(system), "Pipe"),
            "tray" => ("tray", "Cable Tray"),
            "conduit" | "cable" => ("conduit", if kind == "cable" { "Cable" } else { "Conduit" }),
            "grounding" => ("conduit", "Grounding"),
            "equipment" => ("equipment", "Equipment"),
            _ => ("equipment", "Equipment"),
        };
        if let Some(term) = mpart.strip_prefix("terminal:") {
            let (key, cat) = match kind.as_str() {
                "duct" => ("terminals", "Air Terminals"),
                _ if key == "fire" => ("fire", "Sprinklers"),
                "pipe" => ("fixtures", "Plumbing Fixtures"),
                _ => ("devices", "Devices"),
            };
            return mk(key, cat, sized(&format!("{base} {}", title(term))));
        }
        if let Some(kind_of) = mpart.strip_prefix("equipment:").or((kind == "equipment").then_some("")) {
            let ty = made_tag(e, "tag").map(str::to_string).filter(|t| !t.is_empty()).unwrap_or_else(|| title(kind_of));
            return mk("equipment", "Equipment", if ty.is_empty() { "Equipment".into() } else { ty });
        }
        let category = if fitting { format!("{noun} Fittings") } else { noun.to_string() };
        return mk(key, &category, sized(&base));
    }
    let p = part.to_ascii_lowercase();
    let typed = |fallback: &str| own_type.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| fallback.to_string());
    match p.as_str() {
        "wall" => mk("architecture", "Walls", typed("Wall")),
        "floor" => mk("architecture", "Floors", typed("Floor")),
        "ceiling" => mk("architecture", "Ceilings", typed("Ceiling")),
        "soffit" => mk("architecture", "Soffits", typed("Soffit")),
        "roof" | "cricket" => mk("architecture", "Roofs", typed(&title(&p))),
        "parapet" => mk("architecture", "Parapets", typed("Parapet")),
        "roof-drain" => mk("architecture", "Roofs", typed("Roof Drain")),
        "stair" => mk("architecture", "Stairs", typed("Stair")),
        "railing" => mk("architecture", "Railings", typed("Railing")),
        "grid" => mk("architecture", "Ceiling Grid", typed("Ceiling Grid")),
        "room" => mk("architecture", "Rooms", typed("Room")),
        "door" => mk("openings", "Doors", typed("Door")),
        "window" | "glass" => mk("openings", "Windows", typed("Window")),
        "curtainwall" => mk("openings", "Curtain Walls", typed("Curtain Wall")),
        "column" | "beam" | "footing" | "brace" => mk("structure", &format!("{}s", title(&p)), typed(&title(&p))),
        "ground" | "site" => mk("site", "Site", typed("Ground")),
        "light" => mk("lights", "Light Fixtures", source_block(d, e).unwrap_or_else(|| typed("Light"))),
        "diffuser" => mk("terminals", "Air Terminals", source_block(d, e).unwrap_or_else(|| typed("Diffuser"))),
        "equipment" => mk("equipment", "Equipment", typed(&source_block(d, e).unwrap_or_else(|| "Equipment".into()))),
        "device" => {
            let ty = own_type.clone().or_else(|| source_block(d, e)).unwrap_or_else(|| e.common.layer.to_string());
            if layer.contains("LIGHT") || layer.contains("LTG") {
                mk("lights", "Light Fixtures", ty)
            } else if layer.contains("PLUMB") || layer.contains("FIXT") {
                mk("fixtures", "Plumbing Fixtures", ty)
            } else if layer.contains("SPRINK") || layer.contains("FIRE") {
                mk("fire", "Sprinklers", ty)
            } else {
                mk("devices", "Devices", ty)
            }
        }
        _ if layer.contains("STRUCT") || layer.contains("COLUMN") => mk("structure", &title(&p), typed(&title(&p))),
        _ => mk("architecture", &title(&p), typed(&title(&p))),
    }
}

// ─── The drawing the view draws ──────────────────────────────────────────────

/// Whether the drawing holds a built model (any object the build made).
pub fn has_model(d: &Drawing) -> bool {
    d.model.iter().take(MAX_SCAN).any(|e| super::wallgen::made_as(e).is_some())
}

/// Whether ghosting is on for `look` over the built model of `d`: as set, else when the model
/// has MEP (with only the shell, a ghosted shell would show nothing solid).
pub fn ghosting(d: &Drawing, look: &Look) -> bool {
    look.ghost.unwrap_or_else(|| d.model.iter().take(MAX_SCAN).any(|e| super::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("mep"))))
}

/// What the 3D view draws: the built model in the BIM view's look, or `None` when the view is
/// off or nothing is built (then the drawing is drawn as it is).
pub fn display(d: &Drawing) -> Option<Drawing> {
    let look = Look::of(d);
    if look.off || !has_model(d) {
        return None;
    }
    Some(compose(d, &look, ghosting(d, &look), true, &|_, _| true))
}

/// How far each level's built elements move so the storeys stack: each level is built over its
/// own plan, and plans drawn side by side on one sheet would stand side by side. A level's base
/// point maps to the building origin (as the IFC export places storeys); the lowest level stays
/// where it is. Levels whose plan is in another drawing don't move.
pub fn stack_offsets(d: &Drawing) -> Vec<(String, cadaddy_geom::Vec2)> {
    let Some(sp) = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok()) else {
        return Vec::new();
    };
    let bases = cadaddy_walk3d::level_bases(d, &sp.to_rules());
    let own: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    let Some(lowest) = own.iter().min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)) else { return Vec::new() };
    let Some(r) = bases.iter().find(|(n, _)| n == &lowest.name).map(|b| b.1) else { return Vec::new() };
    bases
        .into_iter()
        .filter(|(n, _)| own.iter().any(|l| &l.name == n))
        .map(|(n, b)| (n, r - b))
        .filter(|(_, o)| o.x.is_finite() && o.y.is_finite() && (o.x.abs() > 1e-9 || o.y.abs() > 1e-9))
        .collect()
}

/// The move that stacks a built object: its level's offset ([`stack_offsets`]) less what
/// `model.build` already moved it by (a stacked build); none when that leaves nothing.
pub(crate) fn stack_move(level_offset: Option<cadaddy_geom::Vec2>, e: &Entity) -> Option<cadaddy_geom::Vec2> {
    let o = level_offset? - super::bspec::stacked_by(e);
    (o.is_finite() && o.len() > 1e-6).then_some(o)
}

/// `d` with only the built elements `keep` and `look` let through (and, with `look.sheet`, the
/// sheet's own objects), coloured by trade; the ghosted trades see-through when `ghost`; the
/// levels stacked ([`stack_offsets`]) when `stack`.
pub fn compose(d: &Drawing, look: &Look, ghost: bool, stack: bool, keep: &dyn Fn(&Entity, &Class) -> bool) -> Drawing {
    let offsets = if stack { stack_offsets(d) } else { Vec::new() };
    let offset_of = |level: &str| offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(level)).map(|o| o.1);
    let mut out = d.clone();
    out.model = cadaddy_doc::EntityStore::new();
    let rgb = |k: &str| {
        let c = look.rgb(k);
        Color::True(cadaddy_color::Rgb(c[0], c[1], c[2]))
    };
    let ghosted = |k: &str| ghost && trade(k).is_some_and(|t| t.ghost);
    // What each kept object is; each layer's trade when all its objects share one.
    let mut kept: Vec<(&Arc<Entity>, Option<&'static str>, Option<cadaddy_geom::Vec2>)> = Vec::new();
    let mut layer_trade: BTreeMap<String, Option<&'static str>> = BTreeMap::new();
    for e in d.model.iter().take(MAX_SCAN) {
        match classify(d, e) {
            Some(c) => {
                if !look.shows(&c) || !keep(e, &c) {
                    continue;
                }
                let slot = layer_trade.entry(e.common.layer.to_ascii_uppercase()).or_insert(Some(c.key));
                if *slot != Some(c.key) {
                    *slot = None;
                }
                kept.push((e, Some(c.key), stack_move(offset_of(&c.level), e)));
            }
            None if look.sheet => kept.push((e, None, None)),
            None => {}
        }
    }
    if ghost {
        out.materials.retain(|m| !m.name.eq_ignore_ascii_case(GHOST_MATERIAL));
        out.materials.push(cadaddy_doc::material::Material {
            name: GHOST_MATERIAL.into(),
            color: cadaddy_color::Rgb(226, 226, 222),
            roughness: 1.0,
            metallic: 0.0,
            opacity: look.opacity(),
            ..cadaddy_doc::material::Material::default()
        });
    }
    for l in out.layers.iter_mut() {
        if let Some(Some(k)) = layer_trade.get(&l.name.to_ascii_uppercase()) {
            l.color = rgb(k);
            if ghosted(k) {
                l.material = Some(GHOST_MATERIAL.into());
            }
        }
    }
    let mut items: Vec<Entity> = Vec::new();
    for (e, key, off) in kept {
        let Some(k) = key else {
            out.model.push_shared(Arc::clone(e));
            continue;
        };
        let on_layer = layer_trade.get(&e.common.layer.to_ascii_uppercase()).copied().flatten() == Some(k);
        let own_material = e.common.material.as_deref().map(str::trim).is_some_and(|m| !m.is_empty() && !m.eq_ignore_ascii_case("BYLAYER"));
        if off.is_none() && on_layer && matches!(e.common.color, Color::ByLayer) && !(ghosted(k) && own_material) {
            out.model.push_shared(Arc::clone(e));
        } else {
            let mut c = (**e).clone();
            if !(on_layer && matches!(e.common.color, Color::ByLayer)) {
                c.common.color = rgb(k);
            }
            if ghosted(k) && (own_material || !on_layer) {
                c.common.material = Some(GHOST_MATERIAL.to_string()).into();
            }
            if let Some(o) = off {
                c.kind.transform(&cadaddy_geom::Mat3::translate(o));
            }
            items.push(c);
        }
    }
    for c in items {
        out.model.push(c);
    }
    out
}

// ─── bim.look ────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.look", "BIM View", run_look).menu(&["View", "BIM View"]).params(
            "{on?: bool, ghost?: bool | \"auto\" (the shell see-through, MEP solid; auto: when there is MEP), ghost_opacity?: 0.02-1 (0.2), sheet?: bool (the sheet's own lines in 3D too; default false), level?: name | null (show one level), style?: visual style (default shadededges)} → {on, ghost, ghost_opacity, sheet, level, style, built, trades}: how 3D and model.render draw the built model",
        ),
        CommandSpec::new("bim.trades", "Trade Colours", run_trades).params(
            "{colors?: {trade: [r, g, b] | \"#rrggbb\"}, visible?: {trade: bool}, reset?: bool | [trade]} → {trades: [{trade, label, group, color, default, visible, ghosted, count}]}: the one colour table every trade is drawn in",
        ),
        CommandSpec::new("bim.tree", "Model Tree", run_tree).params(
            "{level?, group?, category?, type? (filters: the tree trade is the group), path?: [level, trade?, category?, type?] (`*` for any), visible?: bool (show or hide that branch in the view; with filters and no path, what they name on every level they don't limit), select?: bool (select its elements), zoom?: bool (zoom to them), limit?: elements listed per type (50, max 1000)} → {levels: [{name, count, visible, trades: [{name, count, visible, categories: [{name, count, visible, types: [{name, count, visible, elements: [{handle, name}]}]}]}]}], total, selected?}",
        ),
        CommandSpec::new("bim.element", "Element Properties", run_element)
            .params(
                "{handle? (default the one selected), open_source?: bool (plan view, zoomed to its source on the sheet, the source selected)} → {handle, trade, group, category, type, size, level, system, circuit, quantity: {count, length_ft?, area_sf?, height_ft?}, source: {handle, kind, layer, at, sheet?, sheet_file?}, attributes, issues: [{kind: bcf|rfi, guid|id, title, status}], at: [x,y] (building feet on its level's plan), bottom_ft, top_ft (above the datum), aff_in, center_aff_in, top_aff_in (above its level's floor), at_ceiling, host: {wall, type, face, off_face_in, off_centreline_in, thickness_in, along_ft, from: stored|nearest} | null (when hosted: its stored host wall's face, else the face of a wall there at its height, within the G-HOST tolerance), in_opening? (on the face line of a wall piece over an opening, below it: that face + wall_from_aff_in, wall_to_aff_in), stored_host? (its stored host wall is not among its level's built walls), nearest_wall (within 10 ft; a wall: itself, measured from its longest piece, `at` that piece's middle), wall_unmeasured? (a wall whose footprint gives no centreline: why)}",
            )
            .noundo(),
    ]
}

fn ghost_param(p: &Value) -> Result<Option<Option<bool>>> {
    match p.get("ghost") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(Some(*b))),
        Some(Value::String(t)) if t.eq_ignore_ascii_case("auto") => Ok(Some(None)),
        Some(v) => Err(bad("bim.look", format!("`ghost` must be true, false or \"auto\", got {v}"))),
    }
}

fn level_name(s: &Session, cmd: &str, want: &str) -> Result<String> {
    let d = s.doc()?;
    let mut names: Vec<String> = Vec::new();
    for e in d.model.iter().take(MAX_SCAN) {
        if super::wallgen::made_as(e).is_some()
            && let Some(l) = super::bspec::made_level(e)
            && !names.iter().any(|n| n.eq_ignore_ascii_case(&l))
        {
            names.push(l);
            if names.len() > 500 {
                break;
            }
        }
    }
    if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(want.trim())) {
        return Ok(n.clone());
    }
    if let Ok((sp, true)) = super::bspec::current(s, cmd)
        && let Some(l) = sp.level_matching(want).and_then(|i| sp.levels.get(i))
    {
        return Ok(l.name.clone());
    }
    let shown: Vec<&str> = names.iter().take(50).map(String::as_str).collect();
    Err(bad(cmd, format!("there is no level named {want} in the built model — levels: {shown:?}")))
}

fn look_json(d: &Drawing, look: &Look, style: &str) -> Value {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut built = 0usize;
    for e in d.model.iter().take(MAX_SCAN) {
        if let Some(c) = classify(d, e) {
            built += 1;
            *counts.entry(c.key).or_default() += 1;
        }
    }
    json!({
        "on": !look.off,
        "ghost": ghosting(d, look),
        "ghost_auto": look.ghost.is_none(),
        "ghost_opacity": look.opacity(),
        "sheet": look.sheet,
        "level": look.level,
        "style": style,
        "built": built,
        "trades": counts,
    })
}

fn run_look(s: &mut Session, p: &Value) -> Result<Value> {
    let mut look = Look::of(s.doc()?);
    if let Some(on) = p.get("on").and_then(Value::as_bool) {
        look.off = !on;
    }
    if let Some(g) = ghost_param(p)? {
        look.ghost = g;
    }
    if let Some(v) = p.get("ghost_opacity").filter(|v| !v.is_null()) {
        let v = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad("bim.look", "`ghost_opacity` must be a number from 0.02 to 1"))?;
        look.ghost_opacity = Some(v.clamp(0.02, 1.0));
    }
    if let Some(b) = p.get("sheet").and_then(Value::as_bool) {
        look.sheet = b;
    }
    match p.get("level") {
        Some(Value::Null) => look.level = None,
        Some(Value::String(l)) if l.trim().is_empty() => look.level = None,
        Some(Value::String(l)) => look.level = Some(level_name(s, "bim.look", l)?),
        Some(v) => return Err(bad("bim.look", format!("`level` must be a level name or null, got {v}"))),
        None => {}
    }
    save(s, &look)?;
    // The view's style: shaded with edges unless asked otherwise.
    let style = match str_param(p, "style") {
        Some(st) => {
            let vs = cadaddy_render::shade::VisualStyle::parse(st).ok_or_else(|| bad("bim.look", format!("unknown visual style {st}")))?;
            Some(vs)
        }
        None if p.get("on").and_then(Value::as_bool) == Some(true) => Some(cadaddy_render::shade::VisualStyle::ShadedEdges),
        None => None,
    };
    if let Some(vs) = style
        && s.state()?.visual_style != vs
    {
        s.state_mut()?.visual_style = vs;
        s.touch();
    }
    let name = s.state()?.visual_style.name();
    Ok(look_json(s.doc()?, &look, name))
}

// ─── bim.trades ──────────────────────────────────────────────────────────────

fn parse_rgb(v: &Value) -> Option<[u8; 3]> {
    if let Some(a) = v.as_array() {
        let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
        return match n[..] {
            [r, g, b] if [r, g, b].iter().all(|c| c.is_finite() && (0.0..=255.0).contains(c)) => Some([r as u8, g as u8, b as u8]),
            _ => None,
        };
    }
    let t = v.as_str()?.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let ch = |k: usize| t.get(k..k + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    Some([ch(0)?, ch(2)?, ch(4)?])
}

fn known(cmd: &str, key: &str) -> Result<&'static str> {
    trade(key).map(|t| t.key).ok_or_else(|| {
        let keys: Vec<&str> = TRADES.iter().map(|t| t.key).collect();
        bad(cmd, format!("there is no trade {key} — trades: {keys:?}"))
    })
}

fn run_trades(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.trades";
    let mut look = Look::of(s.doc()?);
    match p.get("reset") {
        Some(Value::Bool(true)) => {
            look.colors.clear();
            look.hidden.clear();
        }
        Some(Value::Array(a)) => {
            for k in a.iter().take(100) {
                let k = known(CMD, k.as_str().unwrap_or(""))?;
                look.colors.remove(k);
                look.hidden.remove(k);
            }
        }
        _ => {}
    }
    if let Some(o) = p.get("colors").and_then(Value::as_object) {
        for (k, v) in o.iter().take(100) {
            let key = known(CMD, k)?;
            let c = parse_rgb(v).ok_or_else(|| bad(CMD, format!("`colors.{k}` must be [r, g, b] (0-255) or \"#rrggbb\", got {v}")))?;
            if trade(key).is_some_and(|t| t.rgb == c) {
                look.colors.remove(key);
            } else {
                look.colors.insert(key.to_string(), c);
            }
        }
    }
    if let Some(o) = p.get("visible").and_then(Value::as_object) {
        for (k, v) in o.iter().take(100) {
            let key = known(CMD, k)?;
            let on = v.as_bool().ok_or_else(|| bad(CMD, format!("`visible.{k}` must be true or false")))?;
            if on {
                look.hidden.remove(key);
            } else {
                look.hidden.insert(key.to_string());
            }
        }
    }
    save(s, &look)?;
    let d = s.doc()?;
    let ghost = ghosting(d, &look);
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for e in d.model.iter().take(MAX_SCAN) {
        if let Some(c) = classify(d, e) {
            *counts.entry(c.key).or_default() += 1;
        }
    }
    let trades: Vec<Value> = TRADES
        .iter()
        .map(|t| {
            json!({
                "trade": t.key,
                "label": t.label,
                "group": t.group,
                "color": look.rgb(t.key),
                "default": t.rgb,
                "visible": !look.hidden.contains(t.key),
                "ghosted": ghost && t.ghost,
                "count": counts.get(t.key).copied().unwrap_or(0),
            })
        })
        .collect();
    Ok(json!({ "trades": trades }))
}

// ─── bim.tree ────────────────────────────────────────────────────────────────

type Types = BTreeMap<String, Vec<Handle>>;
type Categories = BTreeMap<String, Types>;
type Groups = BTreeMap<&'static str, Categories>;

fn path_param(cmd: &str, p: &Value) -> Result<Option<Vec<String>>> {
    match p.get("path") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= MAX_PATH_PARTS => {
            let parts: Option<Vec<String>> = a.iter().map(|v| v.as_str().map(|t| t.trim().chars().take(200).collect())).collect();
            parts.map(Some).ok_or_else(|| bad(cmd, "`path` must be names: [level, trade?, category?, type?]"))
        }
        Some(v) => Err(bad(cmd, format!("`path` must be [level, trade?, category?, type?], got {v}"))),
    }
}

fn under(path: &[String], c: &Class) -> bool {
    let parts = [c.level.as_str(), c.group, c.category.as_str(), c.ty.as_str()];
    path.iter().zip(parts).all(|(a, b)| part_is(a, b))
}

/// Whether a tree path's part names `name` (case aside); `*` names any (a trade on every level).
fn part_is(part: &str, name: &str) -> bool {
    part == "*" || part.eq_ignore_ascii_case(name)
}

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.tree";
    let path = path_param(CMD, p)?;
    let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |n| usize::try_from(n).unwrap_or(MAX_LIST).min(MAX_LIST));
    let filt = |k: &str| str_param(p, k).map(str::trim).filter(|t| !t.is_empty());
    let (fl, fg, fc, ft) = (filt("level"), filt("group"), filt("category"), filt("type"));
    let mut selected: Option<usize> = None;
    // Shown or hidden by the filters alone: the branch they name on every level (or theirs).
    let path = match path {
        None if p.get("visible").is_some() && [fl, fg, fc, ft].iter().any(Option::is_some) => {
            let mut parts: Vec<String> = [fl, fg, fc, ft].iter().map(|f| f.unwrap_or("*").to_string()).collect();
            while parts.last().is_some_and(|x| x == "*") {
                parts.pop();
            }
            Some(parts)
        }
        p => p,
    };
    if let Some(path) = &path {
        if let Some(vis) = p.get("visible").and_then(Value::as_bool) {
            let mut look = Look::of(s.doc()?);
            look.hidden_paths.retain(|h| !(h.len() >= path.len() && path.iter().zip(h).all(|(a, b)| part_is(a, b) || a.eq_ignore_ascii_case(b))));
            if !vis && look.hidden_paths.len() < MAX_PATHS {
                look.hidden_paths.push(path.clone());
            }
            save(s, &look)?;
        }
        if bool_or(p, "select", false) || bool_or(p, "zoom", false) {
            let d = s.doc()?;
            let hs: Vec<Handle> =
                d.model.iter().take(MAX_SCAN).filter(|e| classify(d, e).is_some_and(|c| under(path, &c))).map(|e| e.handle).collect();
            if hs.is_empty() {
                return Err(bad(CMD, format!("nothing in the model is under {path:?}")));
            }
            selected = Some(hs.len());
            if bool_or(p, "zoom", false) {
                // In 3D the levels stand stacked (`stack_offsets`); in plan, over their plans.
                let three_d = s.state()?.visual_style != cadaddy_render::shade::VisualStyle::Wireframe2d;
                let d = s.doc()?;
                let offs = if three_d { stack_offsets(d) } else { Vec::new() };
                let mut b = cadaddy_geom::Bounds2::EMPTY;
                for h in &hs {
                    let Some(e) = d.model.get(*h) else { continue };
                    let eb = cadaddy_doc::entity_bounds(d, e, 0);
                    let lv = super::bspec::made_level(e).unwrap_or_default();
                    let o = stack_move(offs.iter().find(|(n, _)| n.eq_ignore_ascii_case(&lv)).map(|o| o.1), e).unwrap_or(cadaddy_geom::Vec2::ZERO);
                    if !eb.is_empty() {
                        b = b.union(&cadaddy_geom::Bounds2::new(eb.min + o, eb.max + o));
                    }
                }
                if !b.is_empty() {
                    let pad = b.width().max(b.height()) * 0.05;
                    let b = b.expand(pad);
                    super::view::zoom(s, &json!({ "mode": "window", "p1": [b.min.x, b.min.y], "p2": [b.max.x, b.max.y] }))?;
                }
            }
            if bool_or(p, "select", true) {
                s.set_selection(hs);
            }
        }
    }
    let d = s.doc()?;
    let look = Look::of(d);
    let mut tree: BTreeMap<String, Groups> = BTreeMap::new();
    let mut total = 0usize;
    for e in d.model.iter().take(MAX_SCAN) {
        let Some(c) = classify(d, e) else { continue };
        let ok = |f: Option<&str>, v: &str| f.is_none_or(|f| f.eq_ignore_ascii_case(v));
        if !(ok(fl, &c.level) && ok(fg, c.group) && ok(fc, &c.category) && ok(ft, &c.ty)) {
            continue;
        }
        total += 1;
        let level = if c.level.is_empty() { "(no level)".to_string() } else { c.level.clone() };
        tree.entry(level).or_default().entry(c.group).or_default().entry(c.category).or_default().entry(c.ty).or_default().push(e.handle);
    }
    let mut listed = 0usize;
    let count_g = |g: &Categories| g.values().flat_map(|t| t.values()).map(Vec::len).sum::<usize>();
    let shown = |parts: &[&str]| {
        let owned: Vec<String> = parts.iter().map(|t| t.to_string()).collect();
        !look.hidden_paths.iter().any(|h| !h.is_empty() && h.len() <= owned.len() && h.iter().zip(&owned).all(|(a, b)| part_is(a, b)))
    };
    let levels: Vec<Value> = tree
        .iter()
        .map(|(lv, groups)| {
            let trades: Vec<Value> = groups
                .iter()
                .map(|(g, cats)| {
                    let categories: Vec<Value> = cats
                        .iter()
                        .map(|(cat, types)| {
                            let types: Vec<Value> = types
                                .iter()
                                .map(|(ty, hs)| {
                                    let n = limit.min(MAX_LISTED.saturating_sub(listed)).min(hs.len());
                                    listed += n;
                                    let elements: Vec<Value> = hs
                                        .iter()
                                        .take(n)
                                        .map(|h| json!({ "handle": h.hex(), "name": format!("{ty} [{}]", h.hex()) }))
                                        .collect();
                                    json!({ "name": ty, "count": hs.len(), "visible": shown(&[lv, g, cat, ty]), "elements": elements })
                                })
                                .collect();
                            json!({ "name": cat, "count": types.iter().filter_map(|t| t.get("count").and_then(Value::as_u64)).sum::<u64>(), "visible": shown(&[lv, g, cat]), "types": types })
                        })
                        .collect();
                    json!({ "name": g, "count": count_g(cats), "visible": shown(&[lv, g]), "categories": categories })
                })
                .collect();
            let n: usize = groups.values().map(count_g).sum();
            json!({ "name": lv, "count": n, "visible": shown(&[lv]) && look.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(lv)), "trades": trades })
        })
        .collect();
    let mut out = json!({ "levels": levels, "total": total });
    if let Some(n) = selected {
        out["selected"] = json!(n);
    }
    if total == 0 {
        out["next_step"] = json!("nothing is built yet: run model.build or bim.build");
    }
    Ok(out)
}

// ─── bim.element ─────────────────────────────────────────────────────────────

/// The widest distance between two of `pts` (two sweeps: far from the first, far from that).
fn span(pts: &[Vec3], flat: bool) -> f64 {
    let at = |p: &Vec3| if flat { Vec3::new(p.x, p.y, 0.0) } else { *p };
    let Some(first) = pts.first().map(at) else { return 0.0 };
    let far = |from: Vec3| pts.iter().map(at).fold((from, 0.0f64), |(b, bd), q| if (q - from).len() > bd { (q, (q - from).len()) } else { (b, bd) });
    let (a, _) = far(first);
    let (_, d) = far(a);
    if d.is_finite() { d } else { 0.0 }
}

fn quantity(c: &Class, e: &Entity, upf: f64) -> Value {
    let EntityKind::Mesh(m) = &e.kind else { return json!({ "count": 1 }) };
    let pts: Vec<Vec3> = m.vertices.iter().take(200_000).copied().filter(|p| p.is_finite()).collect();
    let (z0, z1) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.z), hi.max(p.z)));
    let height = if z1 >= z0 { z1 - z0 } else { 0.0 };
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    // Its plan area: its faces facing up, or (a ceiling drawn as one face seen from below) down,
    // whichever there is more of. Never -0 (an empty sum of floats).
    let up_area = || {
        let (mut up, mut down) = (0.0_f64, 0.0_f64);
        for n in m.triangles().into_iter().take(400_000).map(|t| (t[1] - t[0]).cross(t[2] - t[0])) {
            if !n.is_finite() || n.len() <= 0.0 {
                continue;
            }
            let k = n.z / n.len();
            if k > 0.7 {
                up += n.len() * 0.5;
            } else if k < -0.7 {
                down += n.len() * 0.5;
            }
        }
        up.max(down).max(0.0)
    };
    match c.category.as_str() {
        "Walls" | "Parapets" | "Railings" | "Curtain Walls" => {
            let len = span(&pts, true) / upf;
            json!({ "count": 1, "length_ft": r2(len), "height_ft": r2(height / upf), "area_sf": r2(len * height / upf) })
        }
        "Floors" | "Ceilings" | "Soffits" | "Roofs" | "Site" | "Ceiling Grid" | "Rooms" => {
            json!({ "count": 1, "area_sf": r2(up_area() / (upf * upf)) })
        }
        "Duct" | "Pipe" | "Conduit" | "Cable Tray" | "Cable" | "Grounding" => json!({ "count": 1, "length_ft": r2(span(&pts, false) / upf) }),
        _ => json!({ "count": 1, "height_ft": r2(height / upf) }),
    }
}

/// Categories that are not hosted by a wall (the walls themselves, slabs, runs, structure).
const UNHOSTED: [&str; 19] = [
    "Wall", "Floor", "Ceiling", "Soffit", "Roof", "Room", "Site", "Grid", "Parapet", "Railing", "Stair", "Column", "Beam", "Footing", "Brace",
    "Duct", "Pipe", "Conduit", "Cable",
];

/// Where a built element is, so an agent can check its position without reading meshes: its
/// plan point (`at`, building feet on its level's plan), its bottom and top (`bottom_ft`,
/// `top_ft`: feet above the building's datum), its bottom, centre and top above its level's
/// floor (`aff_in`, `center_aff_in`, `top_aff_in`), whether it reaches the ceiling, and its host
/// wall: the wall face nearest it within the G-HOST tolerance (`host`; none for runs, slabs,
/// structure and what hangs at the ceiling), and the nearest wall within 10 ft whatever it is
/// (`nearest_wall`, the face's side and the distance off it, inches, negative inside the wall).
fn placement(s: &Session, e: &Entity, c: &Class) -> serde_json::Map<String, Value> {
    const CMD: &str = "bim.element";
    let mut out = serde_json::Map::new();
    let (Ok(d), Ok((sp, _))) = (s.doc(), super::bspec::current(s, CMD)) else { return out };
    let frame = super::validate::Frame::of_spec(d, &sp);
    let upf = frame.upf();
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    if b.is_empty() {
        return out;
    }
    let r2 = |v: f64| if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 };
    let at = frame.built_on(&c.level, e, b.center());
    out.insert("at".into(), json!([r2(at[0]), r2(at[1])]));
    let z = match &e.kind {
        EntityKind::Mesh(m) => {
            let (z0, z1) = m
                .vertices
                .iter()
                .take(1_000_000)
                .filter(|p| p.z.is_finite())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.z), b.max(p.z)));
            (z0 <= z1).then_some((z0, z1))
        }
        EntityKind::Insert(i) if i.insert.z.is_finite() => Some((i.insert.z, i.insert.z)),
        _ => None,
    };
    let level = sp.level(&c.level);
    let floor = level.map_or(0.0, |l| l.elevation_ft);
    let ceiling = level.map(|l| if l.ceiling_ft > 0.0 { l.ceiling_ft } else { l.height_ft }).filter(|c| c.is_finite() && *c > 0.0);
    let mut at_ceiling = false;
    if let Some((z0, z1)) = z {
        let (bot, top) = (z0 / upf, z1 / upf);
        let (aff, top_aff) = ((bot - floor) * 12.0, (top - floor) * 12.0);
        out.insert("bottom_ft".into(), json!(r2(bot)));
        out.insert("top_ft".into(), json!(r2(top)));
        out.insert("aff_in".into(), json!(r2(aff)));
        out.insert("center_aff_in".into(), json!(r2((aff + top_aff) / 2.0)));
        out.insert("top_aff_in".into(), json!(r2(top_aff)));
        at_ceiling = ceiling.is_some_and(|c| top_aff >= c * 12.0 - 12.0 && aff > 12.0);
        out.insert("at_ceiling".into(), json!(at_ceiling));
    }
    let hostable = !UNHOSTED.iter().any(|k| c.category.contains(k)) && !at_ceiling;
    let search = super::validate_prescreen::HOST_SEARCH_FT;
    // A built wall: measured from its own longest piece, so its nearest wall is itself (its
    // box's centre may lie in the opening under its header).
    if let (Some("wall"), EntityKind::Mesh(mesh)) = (super::wallgen::made_as(e).as_deref(), &e.kind) {
        let studs = HashMap::new();
        let own = super::validate::mesh_walls(e, mesh, &frame, Some(&c.level), &studs);
        let len = |w: &&ModelWall| (w.b[0] - w.a[0]).hypot(w.b[1] - w.a[1]);
        let longest = own
            .walls
            .iter()
            .max_by(|a, b| len(a).total_cmp(&len(b)))
            .or_else(|| own.spans.iter().map(|s| &s.wall).max_by(|a, b| len(a).total_cmp(&len(b))));
        if let Some(w) = longest {
            let mid = [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
            out.insert("at".into(), json!([r2(mid[0]), r2(mid[1])]));
            let near = super::validate_prescreen::nearest_wall(mid, own.walls.iter().chain(own.spans.iter().map(|s| &s.wall)), search);
            out.insert("host".into(), Value::Null);
            out.insert("nearest_wall".into(), near.map_or(Value::Null, |f| f.to_json()));
            return out;
        }
        if let Some(u) = own.unmeasured.first() {
            out.insert("wall_unmeasured".into(), json!(u.why));
        }
    }
    let pieces = super::validate::walls_near(d, &sp, &frame, &c.level, [at[0], at[1], at[0], at[1]], search);
    let walls: Vec<&ModelWall> = pieces.walls.iter().collect();
    let spans: Vec<&WallSpan> = pieces.spans.iter().collect();
    let near = super::validate_prescreen::nearest_wall(at, walls.iter().copied().chain(spans.iter().map(|s| &s.wall)), search);
    let tol = super::validate_rules::effective(&d.project_rules)
        .ok()
        .and_then(|(rs, _)| rs.into_iter().find(|r| r.rule.id == "G-HOST").and_then(|r| r.rule.tolerance.get("plan_in").copied()))
        .unwrap_or(6.0);
    let center = out.get("center_aff_in").and_then(Value::as_f64);
    let stored = super::validate::stored_host(e);
    let h = super::validate_prescreen::host_of(at, center, stored.as_deref(), &walls, &spans, tol);
    let host = h.face.as_ref().filter(|f| hostable && h.opening.is_none() && f.off_face_in.abs() <= tol).map(|f| {
        let mut j = f.to_json();
        j["from"] = json!(h.from);
        j
    });
    out.insert("host".into(), host.unwrap_or(Value::Null));
    if let (true, Some((f, bottom, top))) = (hostable, &h.opening) {
        let mut j = f.to_json();
        j["wall_from_aff_in"] = json!(r2(*bottom));
        j["wall_to_aff_in"] = json!(r2(*top));
        out.insert("in_opening".into(), j);
    }
    if let Some(m) = &h.stored_missing {
        out.insert("stored_host".into(), json!({"wall": m, "why": "not among the built walls of its level"}));
    }
    out.insert("nearest_wall".into(), near.map_or(Value::Null, |f| f.to_json()));
    out
}

fn run_element(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "bim.element";
    let hs = targets(s, p)?;
    let h = match hs.as_slice() {
        [h] => *h,
        [] => return Err(bad(CMD, "select one element of the built model, or give `handle`")),
        _ => return Err(bad(CMD, format!("{} objects are selected: give `handle` (or select one)", hs.len()))),
    };
    let d = s.doc()?;
    let e = d.model.get(h).cloned().ok_or_else(|| bad(CMD, format!("there is no object {} in the model", h.hex())))?;
    let c = classify(d, &e).ok_or_else(|| bad(CMD, format!("{} is not part of the built model (run model.build / bim.build)", h.hex())))?;
    let upf = super::bspec::current(s, CMD).map(|(sp, _)| sp.units_per_foot).unwrap_or(12.0);
    let d = s.doc()?;
    let src = made_source(&e).and_then(|sh| d.model.get(sh).cloned());
    let mut attributes = serde_json::Map::new();
    let mut source = Value::Null;
    if let Some(se) = &src {
        let b = cadaddy_doc::entity_bounds(d, se, 0);
        let at = if b.is_empty() { Value::Null } else { json!([b.center().x, b.center().y]) };
        if let EntityKind::Insert(i) = &se.kind {
            for a in i.attribs.iter().take(200) {
                attributes.insert(a.tag.clone(), json!(a.text.value));
            }
        }
        source = json!({ "handle": se.handle.hex(), "kind": se.kind.type_name(), "layer": se.common.layer.to_string(), "at": at });
        // The sheet the level's plan is on.
        if let Ok((sp, true)) = super::bspec::current(s, CMD) {
            if let Some(f) = sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(&c.level)).and_then(|l| l.sheet.as_ref()) {
                source["sheet_file"] = json!(f);
            }
            let sheets: Vec<&str> = sp
                .sheets
                .iter()
                .filter(|sh| sh.level.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&c.level)))
                .map(|sh| sh.number.as_str())
                .take(20)
                .collect();
            if let Some(first) = sheets.first() {
                source["sheet"] = json!(first);
                if sheets.len() > 1 {
                    source["sheets"] = json!(sheets);
                }
            }
        }
    }
    let circuit = attributes
        .iter()
        .find(|(k, _)| matches!(k.to_ascii_uppercase().as_str(), "CIRCUIT" | "CKT" | "CIRC" | "CIR"))
        .map(|(_, v)| v.clone())
        .or_else(|| made_tag(&e, "circuit").map(|c| json!(c)))
        .unwrap_or(Value::Null);
    // The issues that name it: BCF topics with it in a viewpoint's selection.
    let guid = super::ifcmodel::cached_index(s).ok().and_then(|idx| idx.by_handle(h).and_then(|i| idx.els.get(i)).map(|el| el.tag.guid.clone()));
    let d = s.doc()?;
    let hex = h.hex();
    let mut issues: Vec<Value> = super::bcf::load(d)
        .topics
        .iter()
        .filter(|t| {
            t.viewpoints
                .iter()
                .any(|v| v.selection.iter().any(|g| g.eq_ignore_ascii_case(&hex) || guid.as_deref().is_some_and(|x| !x.is_empty() && x == g)))
        })
        .take(200)
        .map(|t| json!({ "kind": "bcf", "guid": t.guid, "title": t.title, "status": t.status }))
        .collect();
    // RFIs on its level placed on it (within 2' of its plan outline: it stands over its plan).
    if let Ok((sp, true)) = super::bspec::current(s, CMD) {
        let b = cadaddy_doc::entity_bounds(s.doc()?, &e, 0).expand(2.0 * upf);
        for r in sp.rfis.iter().take(10_000) {
            let at = cadaddy_geom::Vec2::new(r.at[0], r.at[1]);
            if issues.len() < 400 && !b.is_empty() && r.level.eq_ignore_ascii_case(&c.level) && b.contains(at) {
                issues.push(json!({ "kind": "rfi", "id": r.id, "title": r.issue, "about": r.about, "status": format!("{:?}", r.status).to_ascii_lowercase() }));
            }
        }
    }
    let place = placement(s, &e, &c);
    let mut out = json!({
        "handle": hex,
        "trade": c.key,
        "trade_label": trade(c.key).map_or("", |t| t.label),
        "group": c.group,
        "category": c.category,
        "type": c.ty,
        "size": made_tag(&e, "size"),
        "level": c.level,
        "system": made_tag(&e, "system"),
        "circuit": circuit,
        "quantity": quantity(&c, &e, upf),
        "source": source,
        "attributes": attributes,
        "issues": issues,
    });
    if let Some(o) = out.as_object_mut() {
        o.extend(place);
    }
    if bool_or(p, "open_source", false) {
        let Some(se) = src else { return Err(bad(CMD, format!("{hex} has no source object on the sheet"))) };
        let b = cadaddy_doc::entity_bounds(s.doc()?, &se, 0);
        let pad = (b.width().max(b.height()) * 4.0).max(10.0 * upf);
        s.state_mut()?.view_dir = Vec3::Z;
        if !b.is_empty() {
            let c = b.center();
            super::view::zoom(s, &json!({ "mode": "window", "p1": [c.x - pad, c.y - pad], "p2": [c.x + pad, c.y + pad] }))?;
        }
        s.set_selection(vec![se.handle]);
        s.touch();
        let mut out = out;
        out["opened"] = json!(true);
        return Ok(out);
    }
    Ok(out)
}

#[cfg(test)]
mod quantity_tests {
    use super::*;

    #[test]
    fn a_ceiling_drawn_as_one_face_from_below_has_its_area() {
        // A 10' x 12' ceiling (inches) facing down, and an empty mesh: 120 sf, and 0 (not -0).
        let sq = [(0.0, 0.0), (0.0, 144.0), (120.0, 144.0), (120.0, 0.0)].map(|(x, y)| Vec3::new(x, y, 108.0));
        let m = cadaddy_doc::Mesh {
            vertices: sq.to_vec(),
            faces: vec![cadaddy_doc::MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }],
            kind: cadaddy_doc::MeshKind::Polyface,
        };
        let class = Class { key: "ceiling", group: "architecture", category: "Ceilings".into(), ty: String::new(), level: String::new() };
        let q = quantity(&class, &Entity::new(Handle(1), EntityKind::Mesh(m)), 12.0);
        assert_eq!(q["area_sf"], json!(120.0), "{q}");
        let empty = cadaddy_doc::Mesh { vertices: Vec::new(), faces: Vec::new(), kind: cadaddy_doc::MeshKind::Polyface };
        let q = quantity(&class, &Entity::new(Handle(2), EntityKind::Mesh(empty)), 12.0);
        assert!(q["area_sf"].as_f64().is_some_and(|a| a == 0.0 && a.is_sign_positive()), "{q}");
    }
}
