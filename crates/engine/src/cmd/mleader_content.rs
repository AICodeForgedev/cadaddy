//! MLEADER's leader types (straight, spline, none) and block content (with its attributes):
//! applied to the multileader `mleader_kind` builds, for the prompt machine and the JSON form.
//! The current multileader style supplies both unless the command chooses; the per-object
//! overrides (leader colour and linetype, arrowhead, text frame) are read here too.

use cadaddy_doc::{Attrib, Entity, EntityKind, Handle, Insert, MLeader, MLeaderStyle};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::Value;

use super::{bad, str_param};
use crate::{EngineError, LeaderType, Result, Session};

/// How the multileader differs from a straight leader with text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Content {
    pub leader: LeaderType,
    /// Block content: the block name, and whether its extents centre (rather than its insertion
    /// point) attaches to the end of the landing.
    pub block: Option<(String, bool)>,
    /// Attribute values of the block content by tag (else each definition's default).
    pub values: serde_json::Map<String, Value>,
}

/// The current multileader style (CMLEADERSTYLE; the first style, else Standard).
pub(super) fn current_style(s: &Session) -> MLeaderStyle {
    let Ok(d) = s.doc() else { return MLeaderStyle::default() };
    let cur = d.header.str("CMLEADERSTYLE", "Standard");
    d.mleader_styles.iter().find(|m| m.name.eq_ignore_ascii_case(&cur)).or(d.mleader_styles.first()).cloned().unwrap_or_default()
}

/// A style's leader type.
pub(super) fn style_leader(st: &MLeaderStyle) -> LeaderType {
    match st.leader_type.to_ascii_lowercase().as_str() {
        "spline" => LeaderType::Spline,
        "none" => LeaderType::None,
        _ => LeaderType::Straight,
    }
}

/// A style's block content, when it has one whose block exists.
pub(super) fn style_block(s: &Session, st: &MLeaderStyle) -> Option<(String, bool)> {
    let name = st.block.trim();
    (!name.is_empty()).then(|| block_name(s, name).ok().map(|n| (n, st.block_center))).flatten()
}

/// Per-object overrides of a multileader's style (`None`: leave as it is).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Overrides {
    pub leader_color: Option<Option<cadaddy_doc::color::Color>>,
    pub leader_linetype: Option<Option<String>>,
    pub arrow: Option<Option<String>>,
    pub text_frame: Option<bool>,
}

impl Overrides {
    /// `leaderColor` (a colour, or "ByBlock"/null to drop the override), `leaderLinetype` (a
    /// linetype of the drawing, or null), `arrow` (an arrowhead name such as "_DOT", or null) and
    /// `textFrame` (bool). Checked before anything changes.
    pub fn from_json(s: &Session, cmd: &str, p: &Value) -> Result<Overrides> {
        let mut o = Overrides::default();
        match p.get("leaderColor") {
            None => {}
            Some(Value::Null) => o.leader_color = Some(None),
            Some(v) => {
                let c = v
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|i| i.to_string()))
                    .and_then(|t| cadaddy_doc::color::Color::parse(&t))
                    .ok_or_else(|| bad(cmd, "`leaderColor` must be a colour such as \"1\", \"red\" or \"255,0,0\""))?;
                o.leader_color = Some(Some(c));
            }
        }
        match p.get("leaderLinetype") {
            None => {}
            Some(Value::Null) => o.leader_linetype = Some(None),
            Some(v) => {
                let n = v.as_str().map(str::trim).unwrap_or_default();
                let d = s.doc()?;
                let found = d.linetypes.iter().find(|l| l.name.eq_ignore_ascii_case(n)).map(|l| l.name.clone());
                let found = found.or_else(|| ["ByLayer", "ByBlock", "Continuous"].iter().find(|x| x.eq_ignore_ascii_case(n)).map(|x| x.to_string()));
                o.leader_linetype = Some(Some(found.ok_or_else(|| bad(cmd, format!("no linetype \"{n}\" in the drawing")))?));
            }
        }
        match p.get("arrow") {
            None => {}
            Some(Value::Null) => o.arrow = Some(None),
            Some(v) => {
                let n = v.as_str().map(str::trim).filter(|n| n.chars().count() <= 64 && cadaddy_render::Arrowhead::is_known(n));
                o.arrow = Some(Some(n.ok_or_else(|| bad(cmd, "`arrow` must be an arrowhead name such as \"_DOT\" or \"_OPEN\""))?.to_string()));
            }
        }
        match p.get("textFrame") {
            None | Some(Value::Null) => {}
            Some(v) => o.text_frame = Some(v.as_bool().ok_or_else(|| bad(cmd, "`textFrame` must be true or false"))?),
        }
        Ok(o)
    }
    pub fn is_empty(&self) -> bool {
        *self == Overrides::default()
    }
    pub fn apply(&self, m: &mut MLeader) {
        if let Some(c) = &self.leader_color {
            m.leader_color = *c;
        }
        if let Some(l) = &self.leader_linetype {
            m.leader_linetype.clone_from(l);
        }
        if let Some(a) = &self.arrow {
            m.arrow.clone_from(a);
        }
        if let Some(f) = self.text_frame {
            m.text_frame = f;
        }
    }
}

/// The attributes of block content `ins`: one per non-constant definition, valued from
/// `values` by tag or the definition's default.
fn content_attribs(s: &Session, ins: &Insert, values: &serde_json::Map<String, Value>) -> Vec<Attrib> {
    let Ok(d) = s.doc() else { return Vec::new() };
    let Some(blk) = d.block(&ins.block) else { return Vec::new() };
    let m = ins.transform(blk.base.xy());
    let mut out = Vec::new();
    for e in blk.entities.iter() {
        let EntityKind::AttDef(ad) = &e.kind else { continue };
        if ad.constant {
            continue;
        }
        let mut k = EntityKind::Text(ad.text.clone());
        k.transform(&m);
        let EntityKind::Text(mut text) = k else { continue };
        let given = values.iter().find(|(t, _)| t.eq_ignore_ascii_case(&ad.tag)).and_then(|(_, v)| match v {
            Value::String(t) => Some(t.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
        text.value = given.unwrap_or_else(|| ad.text.value.clone());
        out.push(Attrib {
            tag: ad.tag.clone(),
            text,
            invisible: ad.invisible,
            constant: false,
            prompt: String::new(),
            props: Default::default(),
            multiline: None,
            modes: ad.modes.for_reference(),
        });
    }
    out
}

/// A block MLEADER can use as content: defined, and not an anonymous (`*`) block.
pub(super) fn block_name(s: &Session, name: &str) -> Result<String> {
    let name = name.trim();
    match s.doc()?.block(name) {
        Some(b) if !b.name.starts_with('*') => Ok(b.name.clone()),
        _ => Err(EngineError::Other(format!("Block \"{name}\" not found."))),
    }
}

/// Apply the leader type and block content to a multileader from `mleader_kind`.
pub(super) fn apply(s: &Session, k: EntityKind, c: &Content) -> Result<EntityKind> {
    let EntityKind::MLeader(mut m) = k else { return Ok(k) };
    m.spline = c.leader == LeaderType::Spline;
    let land = m.landing.xy();
    // The side the content sits on, as `mleader_kind` chose it for the text.
    let dir = if m.leaders.first().and_then(|l| l.first()).is_some_and(|a| a.x > land.x) { -1.0 } else { 1.0 };
    if c.leader == LeaderType::None {
        // Content only, at the picked location (text from its top left corner).
        m.leaders.clear();
        m.dogleg = 0.0;
        if let Some(t) = &mut m.text {
            t.insert = m.landing;
            t.attach = 1;
        }
    }
    if let Some((name, center)) = &c.block {
        let anchor = land + Vec2::new(m.dogleg * dir, 0.0);
        let mut ins = Insert {
            block: name.clone(),
            insert: Vec3::new(anchor.x, anchor.y, m.landing.z),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        if *center {
            // Move the block so the centre of its extents lands on the anchor.
            let d = s.doc()?;
            let probe = Entity { handle: Handle(0), common: Default::default(), kind: EntityKind::Insert(ins.clone()) };
            let b = cadaddy_doc::entity_bounds(d, &probe, 0);
            if !b.is_empty() {
                let c = b.center();
                if c.is_finite() {
                    ins.insert = Vec3::new(ins.insert.x + anchor.x - c.x, ins.insert.y + anchor.y - c.y, ins.insert.z);
                }
            }
        }
        ins.attribs = content_attribs(s, &ins, &c.values);
        m.text = None;
        m.block = Some(ins);
    }
    Ok(EntityKind::MLeader(m))
}

/// `leaderType` ("straight" | "spline" | "none"), `block` (a block name), `blockAttach`
/// ("center" | "insertion") and `attribs` ({TAG: value}) from the JSON form; the current style's
/// leader type and block content when not given.
pub(super) fn json_content(s: &Session, p: &Value) -> Result<Content> {
    let st = current_style(s);
    let values = match p.get("attribs") {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err(bad("mleader", "`attribs` must be {TAG: value}")),
    };
    let leader = match str_param(p, "leaderType").map(str::to_ascii_lowercase).as_deref() {
        None => style_leader(&st),
        Some("straight") => LeaderType::Straight,
        Some("spline") => LeaderType::Spline,
        Some("none") => LeaderType::None,
        Some(_) => return Err(bad("mleader", "`leaderType` must be \"straight\", \"spline\" or \"none\"")),
    };
    let block = match str_param(p, "block") {
        // "" asks for text content even when the style has a block.
        None if p.get("text").is_some() => None,
        None => style_block(s, &st),
        Some("") => None,
        Some(n) => {
            let center = match str_param(p, "blockAttach").map(str::to_ascii_lowercase).as_deref() {
                None | Some("center") => true,
                Some("insertion") => false,
                Some(_) => return Err(bad("mleader", "`blockAttach` must be \"center\" or \"insertion\"")),
            };
            Some((block_name(s, n).map_err(|e| bad("mleader", e.to_string()))?, center))
        }
    };
    Ok(Content { leader, block, values })
}
