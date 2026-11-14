//! The small settings kept in the building spec, read once per spec text (Q6481).
//!
//! The building spec (`Drawing::building_spec`) is the whole job as JSON: on a large job it runs
//! to tens of megabytes. Beside the building it carries a few small settings that the screen
//! reads all the time: the coordination state (section box, coordination mode, isolation, colour
//! schemes, saved views: the `coord_*` keys), the BIM view's look (`bim_view`), the fidelity rules
//! (`fidelity`) and the drawing units (`units_per_foot`). Reading one of them by parsing the whole
//! text cost a large model 50-100 ms each time, and the 3D view, the BIM and Properties palettes
//! and Walk did it on every frame. Here they are read once for each copy of the spec text and kept
//! until that text changes; every reader of these settings goes through [`settings`].
//!
//! The key is the spec text's own copy ([`cadaddy_doc::shared::Shared`]): drawings an edit made
//! from one another share it until an edit changes the spec, and a change always makes a new copy,
//! so a kept answer is never stale. The cache holds no text alive (a weak handle), only the small
//! settings, and the last few spec texts (several open drawings).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cadaddy_doc::Drawing;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;

/// How many spec texts are kept (one per open drawing, a few at most).
const KEEP: usize = 4;

/// Whether a top-level key of the spec is one of the small settings kept here.
fn small(key: &str) -> bool {
    key.starts_with("coord_") || key == crate::cmd::bimlook::KEY || key == crate::cmd::fidelity::KEY
}

/// The small settings of one spec text.
#[derive(Debug, Default)]
pub struct Settings {
    keys: BTreeMap<String, Value>,
    /// Drawing units per foot (the typed spec's, else 12), worked out the first time it is asked.
    upf: OnceLock<f64>,
}

static NULL: Value = Value::Null;

impl Settings {
    /// The value of top-level key `key` (`Null` when the spec has none or isn't JSON). Only the
    /// small settings are kept: `coord_*`, `bim_view` and `fidelity`.
    pub fn get(&self, key: &str) -> &Value {
        self.keys.get(key).unwrap_or(&NULL)
    }
}

struct Entry {
    text: Weak<Option<String>>,
    settings: Arc<Settings>,
}

static KEPT: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
thread_local! {
    static PARSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has parsed a whole spec text here (a test counter: a frame of the
/// screen that reads these settings parses nothing once they are read).
pub fn whole_parses() -> u64 {
    PARSES.with(std::cell::Cell::get)
}

fn counted() {
    PARSES.with(|n| n.set(n.get().saturating_add(1)));
}

fn kept() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The small settings of `d`'s building spec (empty when it has none).
pub fn settings(d: &Drawing) -> Arc<Settings> {
    let spec = &d.building_spec;
    if spec.is_none() {
        return Arc::default();
    }
    {
        let mut list = kept();
        list.retain(|e| e.text.strong_count() > 0);
        if let Some(i) = list.iter().position(|e| spec.is(&e.text)) {
            let e = list.remove(i);
            let s = Arc::clone(&e.settings);
            list.insert(0, e);
            return s;
        }
    }
    // Parsed outside the lock: another thread's drawing doesn't wait for this one.
    let settings = Arc::new(read(spec.as_deref().unwrap_or_default()));
    let mut list = kept();
    list.insert(0, Entry { text: spec.downgrade(), settings: Arc::clone(&settings) });
    list.truncate(KEEP);
    settings
}

/// Top-level key `key` of `d`'s building spec, if it is one of the small settings (`Null`
/// otherwise).
pub fn get(d: &Drawing, key: &str) -> Value {
    settings(d).get(key).clone()
}

/// Drawing units per foot of `d`'s building spec (12 when it has none or it doesn't read).
pub fn units_per_foot(d: &Drawing) -> f64 {
    let s = settings(d);
    *s.upf.get_or_init(|| {
        counted();
        d.building_spec
            .as_deref()
            .and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok())
            .map(|sp| sp.units_per_foot)
            .filter(|u| u.is_finite() && *u > 0.0)
            .unwrap_or(12.0)
    })
}

/// The small settings of spec text `text`: one pass over it that keeps those keys and skips the
/// rest without building it.
fn read(text: &str) -> Settings {
    counted();
    let mut de = serde_json::Deserializer::from_str(text);
    // Text after the object (or none at all) doesn't read as a spec, as with a whole parse.
    let keys = de.deserialize_any(Small).ok().filter(|_| de.end().is_ok()).unwrap_or_default();
    Settings { keys, upf: OnceLock::new() }
}

struct Small;

impl<'de> Visitor<'de> for Small {
    type Value = BTreeMap<String, Value>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a building spec object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = BTreeMap::new();
        while let Some(k) = map.next_key::<String>()? {
            if small(&k) {
                // A repeated key: the last one, as a whole-text parse keeps.
                let v: Value = map.next_value()?;
                out.insert(k, v);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(out)
    }
    // Anything that isn't an object has no settings.
    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(BTreeMap::new())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(BTreeMap::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with_spec(text: &str) -> Drawing {
        let mut d = Drawing::new_imperial();
        d.building_spec = Some(text.to_string()).into();
        d
    }

    #[test]
    fn reads_the_small_keys_as_a_whole_parse_would() {
        let spec = json!({
            "version": 1, "units_per_foot": 304.8, "rooms": [{"name": "A", "coord_x": 1}],
            "coord_sectionbox": {"on": true, "min": [0, 0, 0], "max": [1, 2, 3]},
            "coord_mode": {"on": true}, "bim_view": {"off": false}, "fidelity": {"default_tier": "floor"},
        })
        .to_string();
        let d = with_spec(&spec);
        let whole: Value = serde_json::from_str(&spec).unwrap();
        for k in ["coord_sectionbox", "coord_mode", "bim_view", "fidelity", "coord_views"] {
            assert_eq!(get(&d, k), whole.get(k).cloned().unwrap_or(Value::Null), "{k}");
        }
        assert_eq!(get(&d, "rooms"), Value::Null, "the building itself isn't kept");
        assert_eq!(units_per_foot(&d), 304.8, "the typed spec's units");
        assert_eq!(units_per_foot(&with_spec("{\"units_per_foot\": \"x\"}")), 12.0, "a spec that doesn't read: inches");
        assert_eq!(settings(&Drawing::new_imperial()).get("coord_mode"), &Value::Null);
        for bad in ["", "not json", "[1, 2]", "7", "{\"coord_mode\": ", "{\"coord_mode\": 1} x"] {
            assert_eq!(get(&with_spec(bad), "coord_mode"), Value::Null, "{bad:?}");
        }
    }

    #[test]
    fn a_spec_text_is_parsed_once_until_it_changes() {
        // The counter is this thread's: other tests parse on their own threads.
        let mut d = with_spec(&json!({"coord_mode": {"on": true}, "units_per_foot": 12.0}).to_string());
        let before = whole_parses();
        for _ in 0..50 {
            assert_eq!(get(&d, "coord_mode")["on"], json!(true));
            let _ = units_per_foot(&d);
            let copy = d.clone();
            let _ = get(&copy, "bim_view");
        }
        assert_eq!(whole_parses() - before, 2, "one settings read and one typed read for units");
        d.building_spec = Some(json!({"coord_mode": {"on": false}}).to_string()).into();
        assert_eq!(get(&d, "coord_mode")["on"], json!(false), "a changed spec is read again");
        if let Some(t) = d.building_spec.as_mut() {
            *t = json!({"coord_mode": {"on": true}}).to_string();
        }
        assert_eq!(get(&d, "coord_mode")["on"], json!(true), "changed in place too");
    }
}
