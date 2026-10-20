//! Drawing properties (DWGPROPS): the summary (title, subject, author, keywords, comments, last
//! saved by, revision number) and custom properties as name/value pairs. The hyperlink base is the
//! `HYPERLINKBASE` header variable; created, modified and editing time are `TDCREATE`, `TDUPDATE`
//! and `TDINDWG`.
//!
//! In DXF and DWG files they are the `DWGPROPS` XRECORD of the named object dictionary, as
//! AutoCAD files hold it: the marker `DWGPROPS COOKIE` (group 1), title 2, subject 3, author 4,
//! comments 6, keywords 7, last saved by 8, revision number 9, then the custom properties as
//! `name=value` in groups 300 to 309 (ten; unused ones `=`). A .cdy file keeps them whole (any
//! number of custom properties).

use serde::{Deserialize, Serialize};

use crate::RawTag;

/// Custom properties a DXF or DWG file holds (groups 300–309).
pub const FILE_CUSTOM: usize = 10;
/// Most custom properties kept (hostile input).
pub const MAX_CUSTOM: usize = 1000;
/// Longest text kept for one property.
pub const MAX_TEXT: usize = 4096;
/// The XRECORD's key in the named object dictionary, and its marker.
pub const KEY: &str = "DWGPROPS";
const COOKIE: &str = "DWGPROPS COOKIE";

/// The drawing's summary and custom properties.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DwgProps {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub keywords: String,
    pub comments: String,
    pub last_saved_by: String,
    pub revision_number: String,
    /// Name/value pairs, in order; names are unique (without regard to case).
    pub custom: Vec<(String, String)>,
}

fn clip(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\t').take(MAX_TEXT).collect()
}

impl DwgProps {
    pub fn is_empty(&self) -> bool {
        *self == DwgProps::default()
    }

    /// A custom property's value (its name without regard to case).
    pub fn custom(&self, name: &str) -> Option<&str> {
        self.custom.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Set (or, with `None`, remove) a custom property; a new one goes last. False when the name is
    /// empty, has `=` (the file form's separator) or the list is full.
    pub fn set_custom(&mut self, name: &str, value: Option<&str>) -> bool {
        let name = clip(name.trim());
        if name.is_empty() || name.contains('=') {
            return false;
        }
        let at = self.custom.iter().position(|(n, _)| n.eq_ignore_ascii_case(&name));
        match (at, value) {
            (Some(i), Some(v)) => {
                if let Some(slot) = self.custom.get_mut(i) {
                    slot.1 = clip(v);
                }
            }
            (Some(i), None) => {
                self.custom.remove(i);
            }
            (None, Some(v)) if self.custom.len() < MAX_CUSTOM => self.custom.push((name, clip(v))),
            (None, Some(_)) => return false,
            (None, None) => {}
        }
        true
    }

    /// The summary fields by their DWGPROPS names (`title`, `subject`, `author`, `keywords`,
    /// `comments`, `lastSavedBy`, `revisionNumber`).
    pub fn field_mut(&mut self, name: &str) -> Option<&mut String> {
        Some(match name.to_ascii_lowercase().replace(['_', ' '], "").as_str() {
            "title" => &mut self.title,
            "subject" => &mut self.subject,
            "author" => &mut self.author,
            "keywords" => &mut self.keywords,
            "comments" => &mut self.comments,
            "lastsavedby" => &mut self.last_saved_by,
            "revisionnumber" | "revision" => &mut self.revision_number,
            _ => return None,
        })
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        Some(match name.to_ascii_lowercase().replace(['_', ' '], "").as_str() {
            "title" => &self.title,
            "subject" => &self.subject,
            "author" => &self.author,
            "keywords" => &self.keywords,
            "comments" => &self.comments,
            "lastsavedby" => &self.last_saved_by,
            "revisionnumber" | "revision" => &self.revision_number,
            _ => return None,
        })
    }

    /// The DWGPROPS XRECORD's data groups (after `100 AcDbXrecord` and `280`). Only the first
    /// [`FILE_CUSTOM`] custom properties fit.
    pub fn to_tags(&self) -> Vec<RawTag> {
        let t = |code: i32, v: &str| RawTag { code, value: v.replace(['\r', '\n'], " ") };
        let mut out = vec![
            t(1, COOKIE),
            t(2, &self.title),
            t(3, &self.subject),
            t(4, &self.author),
            t(6, &self.comments),
            t(7, &self.keywords),
            t(8, &self.last_saved_by),
            t(9, &self.revision_number),
        ];
        for i in 0..FILE_CUSTOM {
            let v = self.custom.get(i).map_or_else(|| "=".to_string(), |(n, v)| format!("{n}={v}"));
            out.push(t(300 + i as i32, &v));
        }
        out
    }

    /// The properties in a DWGPROPS XRECORD's groups; `None` when they aren't one.
    pub fn from_tags(tags: &[RawTag]) -> Option<DwgProps> {
        if !tags.iter().any(|t| t.code == 1 && t.value.trim().eq_ignore_ascii_case(COOKIE)) {
            return None;
        }
        let mut p = DwgProps::default();
        for t in tags {
            let v = clip(&t.value);
            match t.code {
                2 => p.title = v,
                3 => p.subject = v,
                4 => p.author = v,
                6 => p.comments = v,
                7 => p.keywords = v,
                8 => p.last_saved_by = v,
                9 => p.revision_number = v,
                300..=399 => {
                    if let Some((n, val)) = v.split_once('=')
                        && !n.trim().is_empty()
                    {
                        p.set_custom(n, Some(val));
                    }
                }
                _ => {}
            }
        }
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_form_round_trips_and_ignores_what_it_cant_hold() {
        let mut p =
            DwgProps { title: "Sample Floor Plan".into(), author: "Project A team".into(), comments: "Two\nlines".into(), ..Default::default() };
        assert!(p.set_custom("Phase", Some("Design")));
        assert!(p.set_custom("phase", Some("Construction")), "names without regard to case");
        assert!(!p.set_custom("a=b", Some("x")) && !p.set_custom("  ", Some("x")));
        assert_eq!(p.custom("PHASE"), Some("Construction"));
        let back = DwgProps::from_tags(&p.to_tags()).unwrap();
        assert_eq!(back.title, "Sample Floor Plan");
        assert_eq!(back.comments, "Two lines", "one line in the file");
        assert_eq!(back.custom, vec![("Phase".to_string(), "Construction".to_string())]);
        assert_eq!(p.to_tags().len(), 8 + FILE_CUSTOM);
        assert_eq!(DwgProps::from_tags(&[RawTag { code: 1, value: "OTHER".into() }]), None);
        assert!(p.set_custom("Phase", None) && p.custom.is_empty());
        for i in 0..MAX_CUSTOM + 5 {
            p.set_custom(&format!("P{i}"), Some("v"));
        }
        assert_eq!(p.custom.len(), MAX_CUSTOM);
    }
}
