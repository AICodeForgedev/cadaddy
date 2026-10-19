//! Data extraction settings: what to extract and where it goes, saved to a `.dxe` file to run
//! again later, and kept on a table made by extraction so DATAEXTRACTION Update can refill it.
//!
//! The `.dxe` file is CADaddy's own JSON (not Autodesk's format):
//!
//! ```json
//! { "format": "cadaddy-dataextraction", "version": 1,
//!   "drawings": ["other.dxf"], "includeCurrent": true,
//!   "blocks": "*", "tags": "*", "includeBlocks": true, "nested": true,
//!   "types": ["Line", "Circle"], "properties": ["Layer", "Length"],
//!   "combine": true, "file": "out.csv", "title": "Schedule" }
//! ```
//!
//! `types` are object types other than block references (as the Properties palette names them,
//! wildcards allowed); `properties` are columns from [`super::props::PROPERTIES`].

use cadaddy_doc::RawTag;
use serde::{Deserialize, Serialize};

use crate::{EngineError, Result};

/// The `format` value of a settings file.
pub(crate) const FORMAT: &str = "cadaddy-dataextraction";
/// The xdata application name of a table's settings.
pub(crate) const APP: &str = "CADADDY_DXE";
/// Largest settings file read.
const MAX_FILE: usize = 1_000_000;
/// Most drawings, object types, properties and handles a settings file lists.
pub(crate) const MAX_LIST: usize = 10_000;

fn star() -> String {
    "*".into()
}
fn yes() -> bool {
    true
}
fn format() -> String {
    FORMAT.into()
}
fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    #[serde(default = "format")]
    pub format: String,
    #[serde(default = "one")]
    pub version: u32,
    /// Other drawing files to extract from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawings: Vec<String>,
    /// The current drawing too (always, when no other drawings are listed).
    #[serde(default = "yes")]
    pub include_current: bool,
    #[serde(default = "star")]
    pub blocks: String,
    #[serde(default = "star")]
    pub tags: String,
    /// Block references are extracted (with their attributes).
    #[serde(default = "yes")]
    pub include_blocks: bool,
    #[serde(default = "yes")]
    pub nested: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<String>,
    #[serde(default = "yes")]
    pub combine: bool,
    /// Only these objects of the current drawing (handles), when given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap_or(Settings {
            format: format(),
            version: 1,
            drawings: Vec::new(),
            include_current: true,
            blocks: star(),
            tags: star(),
            include_blocks: true,
            nested: true,
            types: Vec::new(),
            properties: Vec::new(),
            combine: true,
            handles: Vec::new(),
            file: None,
            title: None,
        })
    }
}

impl Settings {
    /// Settings that can be used: ours, a version we read, lists within bounds and properties we know.
    pub fn check(&self) -> Result<()> {
        if self.format != FORMAT || self.version != 1 {
            return Err(EngineError::Other("Not a CADaddy data extraction settings file.".into()));
        }
        if [self.drawings.len(), self.types.len(), self.properties.len(), self.handles.len()].iter().any(|n| *n > MAX_LIST) {
            return Err(EngineError::Other("The settings list too many items.".into()));
        }
        for p in &self.properties {
            if super::props::canonical(p).is_none() {
                return Err(EngineError::Other(format!("Unknown property \"{p}\" (one of: {}).", super::props::PROPERTIES.join(", "))));
            }
        }
        Ok(())
    }

    pub fn parse(text: &str) -> Result<Settings> {
        if text.len() > MAX_FILE {
            return Err(EngineError::Other("The settings file is too large.".into()));
        }
        let s: Settings = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| EngineError::Other(format!("Settings file: {e}")))?;
        s.check()?;
        Ok(s)
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The settings as xdata for a table: `1001 CADADDY_DXE`, then the JSON in `1000` strings
    /// of at most 240 bytes (split between characters).
    pub fn xdata(&self) -> Vec<RawTag> {
        let json = serde_json::to_string(self).unwrap_or_default();
        let mut out = vec![RawTag { code: 1001, value: APP.into() }];
        let mut cur = String::new();
        for c in json.chars() {
            if cur.len() + c.len_utf8() > 240 {
                out.push(RawTag { code: 1000, value: std::mem::take(&mut cur) });
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            out.push(RawTag { code: 1000, value: cur });
        }
        out
    }

    /// The settings kept in `xdata` by [`Settings::xdata`], if any.
    pub fn from_xdata(xdata: &[RawTag]) -> Option<Settings> {
        let start = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
        let mut text = String::new();
        for t in xdata.get(start + 1..)?.iter().take_while(|t| t.code != 1001).filter(|t| t.code == 1000) {
            if text.len() > MAX_FILE {
                return None;
            }
            text.push_str(&t.value);
        }
        Settings::parse(&text).ok()
    }

    /// `xdata` with these settings in place of any kept before (other applications' kept).
    pub fn replace_in(&self, xdata: &[RawTag]) -> Vec<RawTag> {
        let mut out = Vec::new();
        let mut ours = false;
        for t in xdata {
            if t.code == 1001 {
                ours = t.value.trim().eq_ignore_ascii_case(APP);
            }
            if !ours {
                out.push(t.clone());
            }
        }
        out.extend(self.xdata());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_hostile_files_are_refused() {
        let s =
            Settings { types: vec!["Line".into()], properties: vec!["length".into()], title: Some("Ünïcode ".repeat(60)), ..Settings::default() };
        assert_eq!(Settings::from_xdata(&s.xdata()), Some(s.clone()));
        assert!(s.xdata().iter().all(|t| t.value.len() <= 240));
        assert_eq!(Settings::parse(&s.to_text()).ok(), Some(s.clone()));
        let other = vec![RawTag { code: 1001, value: "OTHER".into() }, RawTag { code: 1000, value: "x".into() }];
        let both = s.replace_in(&other);
        assert_eq!(both[..2], other[..]);
        assert_eq!(Settings::from_xdata(&s.replace_in(&both)), Some(s));
        assert!(Settings::parse("{\"format\":\"autodesk\"}").is_err());
        assert!(Settings::parse("{\"properties\":[\"Mass\"]}").is_err());
        assert!(Settings::parse("not json").is_err());
        assert_eq!(Settings::parse("{}").map(|s| s.combine).ok(), Some(true));
    }
}
