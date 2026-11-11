//! The objects a bill-of-materials or takeoff line names (Q6529): a line names what it counted
//! by an object handle, by the spec device it was built from (`device:<level>:x,y`), by a spec
//! run (`run:<index>`), or by the name or tag its build gave an element (`EXIT_SIGN-1`,
//! `FCU-3`). [`Refs`] turns each into the model's objects, so every line resolves to handles
//! (`bim.takeoff.show`, `bim.props.get`) and the ids the documents gave stay beside them.

use std::collections::HashMap;

use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::BuildingSpec;

/// Built elements looked at, at most (the build's own elements come after a print set's
/// linework: the cap is on the built ones).
const MAX_BUILT: usize = super::mepatoms::MAX_ELEMENTS;
/// Objects one id stands for, at most.
const MAX_PER_ID: usize = 20_000;

/// Every built element by the ids a line may name it by.
#[derive(Default)]
pub(crate) struct Refs {
    /// By the spec device it was built from (`devid`), its build name, and its `tag:`
    /// (upper case): the elements.
    by_id: HashMap<String, Vec<String>>,
    /// By the base of its MEP id (`mepid` up to its last `/`): a spec run's segments and
    /// fittings.
    by_run: HashMap<String, Vec<String>>,
    /// Each spec run's base id, by its index.
    run_bases: Vec<String>,
}

fn push(map: &mut HashMap<String, Vec<String>>, key: &str, h: &str) {
    let key = key.trim();
    if key.is_empty() {
        return;
    }
    let v = map.entry(key.to_ascii_uppercase()).or_default();
    if v.len() < MAX_PER_ID && v.last().is_none_or(|l| l != h) {
        v.push(h.to_string());
    }
}

impl Refs {
    /// The drawing's built elements by their ids; `sp` gives the spec runs' ids.
    pub(crate) fn of(d: &Drawing, sp: &BuildingSpec) -> Refs {
        let mut r = Refs { run_bases: sp.mep.runs.iter().map(super::mep::run_base).collect(), ..Refs::default() };
        for e in d.model.iter().filter(|e| super::bimlook::made_tags(e).is_some()).take(MAX_BUILT) {
            let h = e.handle.hex();
            let tags = super::fieldview::wallgen_tags(e);
            if tags.first() == Some(&"device")
                && let Some(id) = super::fieldview::tag_value(&tags, "devid")
            {
                push(&mut r.by_id, id, &h);
            }
            if let Some(name) = super::bimlook::made_name(e) {
                push(&mut r.by_id, name, &h);
            }
            if let Some(tag) = super::fieldview::tag_value(&tags, "tag") {
                push(&mut r.by_id, tag, &h);
            }
            if tags.first() == Some(&"mep")
                && let Some(id) = super::fieldview::tag_value(&tags, "mepid")
            {
                // A run built as pieces names its part after the run (`<run>#0/elbow3`).
                let base = id.rsplit_once('/').map_or(id, |(b, _)| b);
                let base = base.rsplit_once('#').filter(|(_, n)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())).map_or(base, |(b, _)| b);
                let v = r.by_run.entry(base.to_string()).or_default();
                if v.len() < MAX_PER_ID {
                    v.push(h);
                }
            }
        }
        r
    }

    /// The objects `id` names: itself when it is an object of `d`, the elements built from the
    /// spec device or run it names, else those its build named or tagged so; none when the model
    /// has none.
    pub(crate) fn resolve(&self, d: &Drawing, id: &str) -> Vec<String> {
        let id = id.trim();
        if let Some(i) = id.strip_prefix("run:").and_then(|i| i.parse::<usize>().ok()) {
            return self.run_bases.get(i).and_then(|b| self.by_run.get(b)).cloned().unwrap_or_default();
        }
        if let Some(v) = self.by_id.get(&id.to_ascii_uppercase()) {
            return v.clone();
        }
        match Handle::parse_hex(id).filter(|h| d.entity(*h).is_some()) {
            Some(h) => vec![h.hex()],
            None => Vec::new(),
        }
    }
}

/// A line's ids as handles: the objects they name (in order, each once), the ids that are not
/// handles (the documents' ids, as the line gave them), and how many of those name no object.
pub(crate) fn split(refs: &Refs, d: &Drawing, ids: &[String]) -> (Vec<String>, Vec<String>, usize) {
    let mut handles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut sources = Vec::new();
    let mut unresolved = 0usize;
    for id in ids {
        let own = Handle::parse_hex(id.trim()).filter(|h| d.entity(*h).is_some() && !refs.by_id.contains_key(&id.trim().to_ascii_uppercase()));
        let hs = if let Some(h) = own {
            vec![h.hex()]
        } else {
            sources.push(id.clone());
            refs.resolve(d, id)
        };
        if hs.is_empty() {
            unresolved += 1;
        }
        for h in hs {
            if seen.insert(h.clone()) {
                handles.push(h);
            }
        }
    }
    (handles, sources, unresolved)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Session;

    /// A made-up level: a 1" conduit run built as pieces and a panel.
    fn built() -> Session {
        let mut s = Session::new();
        let spec = json!({"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
            "mep": {"systems": [{"name": "EC", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10}],
                "runs": [{"system": "EC", "level": "L1", "points": [[0, 500], [300, 500]], "size_in": [1, 0], "bottoms_ft": [10]}],
                "equipment": [{"tag": "PNL-9", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2}]}});
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap_or_else(|e| panic!("{e}"));
        s.execute("model.build", &json!({"route": false})).unwrap_or_else(|e| panic!("{e}"));
        s
    }

    #[test]
    fn a_run_built_as_pieces_a_tagged_unit_and_a_handle_resolve_to_objects() {
        let s = built();
        let (sp, _) = crate::cmd::bspec::current(&s, "test").unwrap_or_else(|e| panic!("{e}"));
        let d = s.doc().unwrap_or_else(|e| panic!("{e}"));
        let refs = Refs::of(d, &sp);
        // The run's pieces (`<run>#0/conduit0`…): its sticks and fittings.
        let run = refs.resolve(d, "run:0");
        let atoms = d.model.iter().filter(|e| super::super::mepatoms::is_atom(e)).count();
        assert!(atoms > 0 && run.len() == atoms, "{} of {atoms} pieces", run.len());
        // The panel by the tag it was built with, in any case.
        let panel = refs.resolve(d, "pnl-9");
        assert_eq!(panel.len(), 1, "{panel:?}");
        // A handle is itself; an id the model has nothing for is nothing.
        assert_eq!(refs.resolve(d, &panel[0]), panel);
        assert!(refs.resolve(d, "run:7").is_empty() && refs.resolve(d, "NO-SUCH-TAG").is_empty());
        let ids = vec![panel[0].clone(), "run:0".into(), "NO-SUCH-TAG".into()];
        let (hs, sources, unresolved) = split(&refs, d, &ids);
        assert_eq!((hs.len(), sources, unresolved), (1 + atoms, vec!["run:0".to_string(), "NO-SUCH-TAG".to_string()], 1));
    }
}
