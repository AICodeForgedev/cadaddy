//! Overrides: the edits made to generated elements, kept as a layer of their own.
//!
//! A build (WALLGEN, `model.build`, a rule re-run after a new revision) generates elements from
//! the plan. What a person, an agent or an import then changes about them — a device moved, a
//! type changed, a stud verified in the field, a false detection deleted, a missing item added —
//! is not written into the generated geometry. It is an [`Override`] keyed to the element's stable
//! identity ([`ElementKey`]: what it was made as, on which level, from which plan object, and
//! where it stood), with the source area it was made in and a signature of that area's content.
//!
//! After every re-run the engine re-applies the overrides to the regenerated elements: kept where
//! the source area is unchanged ([`Status::Active`]), flagged where the new revision touches it
//! ([`Status::Review`]), and listed as [`Status::Orphaned`] when their element is gone. An
//! override is never dropped except by reverting it.
//!
//! The applied overrides of an element are marked in its XDATA ([`OVERRIDE_APP`], one `1000`
//! string per override id), so re-applying twice changes nothing.

use serde::{Deserialize, Serialize};

use crate::{Entity, Handle, RawTag};

/// The XDATA application name marking an element an override has been applied to.
pub const OVERRIDE_APP: &str = "CADADDY_OVERRIDE";
/// Most overrides one drawing keeps (hostile files).
pub const MAX_OVERRIDES: usize = 100_000;
/// Longest note kept on an override, in characters.
pub const MAX_NOTE: usize = 2000;
/// The format version written; newer ones are refused.
pub const VERSION: u32 = 1;

/// Who made an override.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    /// A person, in CADaddy.
    #[default]
    User,
    /// An agent (MCP, the control channel, a script).
    Agent,
    /// An import (markups, a field package, shop drawings).
    Import,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Agent => "agent",
            Author::Import => "import",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "user" | "person" | "manual" => Some(Author::User),
            "agent" => Some(Author::Agent),
            "import" => Some(Author::Import),
            _ => None,
        }
    }
}

/// Where an override stands after the last re-run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Applied; its source area is as it was when the override was made.
    #[default]
    Active,
    /// Applied, but the drawing changed in its source area: someone should look at it.
    Review,
    /// Its element is gone from the regenerated model: listed, not applied.
    Orphaned,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Review => "review",
            Status::Orphaned => "orphaned",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "active" | "kept" => Some(Status::Active),
            "review" | "flagged" => Some(Status::Review),
            "orphaned" | "orphan" => Some(Status::Orphaned),
            _ => None,
        }
    }
}

/// What an override does to its element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Change {
    /// Moved by `delta` (drawing units).
    Move { delta: [f64; 3] },
    /// Given another BIM type mark (`before`: the mark it was generated with).
    Type { mark: String, before: Option<String> },
    /// Verified in the field (a stud, a device): the generated element stands as built.
    Verify,
    /// A false detection, deleted. `removed`: the element as it was when last deleted (to put it
    /// back on revert or hide).
    Delete { removed: Option<Box<Entity>> },
    /// A missing item, added: this object (drawn by hand, or imported).
    Add { entity: Box<Entity> },
    /// Properties set by a person ([`crate::props`], source `user`): put back on the element
    /// after every rebuild. `before`: what each key held when first set (none: no value), to put
    /// back on revert.
    Props {
        props: Vec<crate::props::Entry>,
        #[serde(default)]
        before: Vec<(String, Option<crate::props::Prop>)>,
    },
}

impl Change {
    pub fn kind(&self) -> &'static str {
        match self {
            Change::Move { .. } => "move",
            Change::Type { .. } => "type",
            Change::Verify => "verify",
            Change::Delete { .. } => "delete",
            Change::Add { .. } => "add",
            Change::Props { .. } => "props",
        }
    }
}

/// The stable identity of a generated element: what the build made it as, on which level, from
/// which plan object, and where it stood. The plan object's handle survives edits in place; a new
/// revision from another file gives new handles, and then the element is the nearest of its part
/// on its level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementKey {
    pub part: String,
    pub level: String,
    /// The plan object's handle (hex).
    pub source: String,
    /// The element's centre in plan, as generated.
    pub at: [f64; 2],
}

/// One override.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub id: u32,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub change: Change,
    /// The element it applies to; none for an added item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ElementKey>,
    /// The source area: `[min x, min y, max x, max y]` in plan.
    pub area: [f64; 4],
    /// A signature of the plan content in the source area when the override was made (or last
    /// accepted).
    pub area_sig: u64,
    #[serde(default)]
    pub status: Status,
    /// The element it is applied to now, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<Handle>,
}

impl Override {
    /// The display id, `OV-0001`.
    pub fn label(&self) -> String {
        label(self.id)
    }
}

/// The display id of override `id`.
pub fn label(id: u32) -> String {
    format!("OV-{id:04}")
}

/// Parse `OV-0001`, `ov-1` or `1`.
pub fn parse_label(s: &str) -> Option<u32> {
    let t = s.trim();
    let n = t.get(..3).filter(|p| p.eq_ignore_ascii_case("OV-")).map_or(t, |_| t.get(3..).unwrap_or(""));
    n.parse().ok()
}

/// A drawing's overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Overrides {
    pub version: u32,
    /// The next id handed out (ids are never reused, so a revert can't be confused with a later
    /// override).
    pub next_id: u32,
    /// Hidden: not applied, so the model shows what the rules generate.
    #[serde(default)]
    pub hidden: bool,
    pub items: Vec<Override>,
}

impl Default for Overrides {
    fn default() -> Self {
        Overrides { version: VERSION, next_id: 1, hidden: false, items: Vec::new() }
    }
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn get(&self, id: u32) -> Option<&Override> {
        self.items.iter().find(|o| o.id == id)
    }
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Override> {
        self.items.iter_mut().find(|o| o.id == id)
    }
    /// Hand out the next id (`None` when the drawing holds too many overrides).
    pub fn take_id(&mut self) -> Option<u32> {
        if self.items.len() >= MAX_OVERRIDES {
            return None;
        }
        let id = self.next_id.max(1);
        self.next_id = id.checked_add(1)?;
        Some(id)
    }
    pub fn remove(&mut self, id: u32) -> Option<Override> {
        let at = self.items.iter().position(|o| o.id == id)?;
        Some(self.items.remove(at))
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parse, refusing a newer version and capping what a hostile file holds: at most
    /// [`MAX_OVERRIDES`], notes cut to [`MAX_NOTE`], non-finite numbers cleared.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut o: Overrides = serde_json::from_str(text).map_err(|e| format!("overrides: {e}"))?;
        if o.version > VERSION {
            return Err(format!("overrides: version {} is newer than this CADaddy reads ({VERSION})", o.version));
        }
        o.items.truncate(MAX_OVERRIDES);
        let finite = |v: &mut f64| {
            if !v.is_finite() {
                *v = 0.0;
            }
        };
        for i in &mut o.items {
            if let Some(n) = &mut i.note {
                *n = n.chars().take(MAX_NOTE).collect();
            }
            i.area.iter_mut().for_each(finite);
            if let Some(t) = &mut i.target {
                t.at.iter_mut().for_each(finite);
            }
            if let Change::Move { delta } = &mut i.change {
                delta.iter_mut().for_each(finite);
            }
        }
        let top = o.items.iter().map(|i| i.id).max().unwrap_or(0);
        o.next_id = o.next_id.max(top.saturating_add(1)).max(1);
        Ok(o)
    }
}

/// The override ids applied to an element (from its XDATA).
pub fn applied_ids(xdata: &[RawTag]) -> Vec<u32> {
    let Some(at) = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(OVERRIDE_APP)) else {
        return Vec::new();
    };
    xdata.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).filter_map(|t| parse_label(&t.value)).collect()
}

/// `xdata` without the override marker.
fn strip_marker(xdata: &[RawTag]) -> Vec<RawTag> {
    let mut out = Vec::with_capacity(xdata.len());
    let mut skip = false;
    for t in xdata {
        if t.code == 1001 {
            skip = t.value.trim().eq_ignore_ascii_case(OVERRIDE_APP);
        }
        if !skip {
            out.push(t.clone());
        }
    }
    out
}

/// `xdata` with the override marker listing `ids` (none: no marker).
pub fn with_ids(xdata: &[RawTag], ids: &[u32]) -> Vec<RawTag> {
    let mut out = strip_marker(xdata);
    if !ids.is_empty() {
        out.push(RawTag { code: 1001, value: OVERRIDE_APP.into() });
        out.extend(ids.iter().map(|&i| RawTag { code: 1000, value: label(i) }));
    }
    out
}

/// `xdata` marked with override `id` too.
pub fn mark(xdata: &[RawTag], id: u32) -> Vec<RawTag> {
    let mut ids = applied_ids(xdata);
    if !ids.contains(&id) {
        ids.push(id);
    }
    with_ids(xdata, &ids)
}

/// `xdata` with override `id` taken off the marker.
pub fn unmark(xdata: &[RawTag], id: u32) -> Vec<RawTag> {
    let ids: Vec<u32> = applied_ids(xdata).into_iter().filter(|&i| i != id).collect();
    with_ids(xdata, &ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityKind, Line};
    use cadaddy_geom::Vec3;

    fn sample() -> Overrides {
        let mut o = Overrides::default();
        let e = Entity::new(Handle(0x2A), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3 { x: 1.0, y: 0.0, z: 0.0 } }));
        for change in [
            Change::Move { delta: [12.0, 0.0, 0.0] },
            Change::Type { mark: "H".into(), before: Some("A".into()) },
            Change::Verify,
            Change::Delete { removed: None },
            Change::Add { entity: Box::new(e) },
        ] {
            let id = o.take_id().unwrap_or_default();
            o.items.push(Override {
                id,
                author: Author::Agent,
                note: Some("checked on the sample sheet".into()),
                target: Some(ElementKey { part: "device".into(), level: "L1".into(), source: "1F".into(), at: [1.0, 2.0] }),
                change,
                area: [0.0, 0.0, 10.0, 10.0],
                area_sig: 7,
                status: Status::Review,
                applied: Some(Handle(0x30)),
            });
        }
        o
    }

    #[test]
    fn json_round_trips_every_change() {
        let o = sample();
        let text = o.to_json().unwrap_or_default();
        assert!(text.contains("\"kind\":\"move\"") && text.contains("\"kind\":\"add\""), "{text}");
        let back = Overrides::from_json(&text).unwrap_or_default();
        assert_eq!(back, o);
        assert_eq!(back.items.iter().map(|i| i.change.kind()).collect::<Vec<_>>(), ["move", "type", "verify", "delete", "add"]);
    }

    #[test]
    fn hostile_json_is_capped_and_newer_versions_refused() {
        assert!(Overrides::from_json(r#"{"version":99,"next_id":1,"items":[]}"#).is_err());
        assert!(Overrides::from_json("not json").is_err());
        let mut o = sample();
        o.next_id = 0;
        if let Some(i) = o.items.first_mut() {
            i.note = Some("x".repeat(MAX_NOTE * 3));
        }
        let back = Overrides::from_json(&o.to_json().unwrap_or_default()).unwrap_or_default();
        assert_eq!(back.items.first().and_then(|i| i.note.as_ref()).map(|n| n.chars().count()), Some(MAX_NOTE));
        // Ids are never reused even when the file's counter is stale.
        assert_eq!(back.next_id, 6);
    }

    #[test]
    fn markers_list_each_id_once_and_leave_other_xdata_alone() {
        let other = vec![RawTag { code: 1001, value: "OTHER_APP".into() }, RawTag { code: 1000, value: "keep".into() }];
        let x = mark(&mark(&mark(&other, 3), 9), 3);
        assert_eq!(applied_ids(&x), [3, 9]);
        assert_eq!(x.get(..2), Some(other.as_slice()));
        let x = unmark(&x, 3);
        assert_eq!(applied_ids(&x), [9]);
        assert_eq!(unmark(&x, 9), other);
        assert_eq!(parse_label("ov-0012"), Some(12));
        assert_eq!(parse_label("12"), Some(12));
        assert_eq!(parse_label("OV-x"), None);
        assert_eq!(label(12), "OV-0012");
    }
}
