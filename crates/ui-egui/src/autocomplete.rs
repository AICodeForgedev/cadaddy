//! AutoComplete for the command line, as AutoCAD's works: command names and aliases (the exact
//! match first, then names starting with the typed text, the ones used most first), system
//! variables, and names that only contain the text (mid-string search). At a prompt asking for a
//! named object (a layer, a block, a style, …) the drawing's names of that kind are offered.

use serde::{Deserialize, Serialize};

/// Most suggestions listed.
pub const MAX: usize = 10;
/// Most commands whose use is counted.
const MAX_USAGE: usize = 1000;

/// What a suggestion stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Command,
    SystemVariable,
    /// A named object answering the prompt.
    Name,
}

/// One AutoComplete suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// What goes on the command line (a command id in capitals, a variable or object name).
    pub id: String,
    /// The text in the list ("REC (RECTANG)", "RECOVER", "Walls").
    pub display: String,
    /// The command's label, the variable's or object's kind.
    pub label: String,
    pub kind: Kind,
}

/// AutoComplete settings (AUTOCOMPLETEMODE's parts), kept with the interface preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Suggest at all.
    pub enabled: bool,
    /// Include system variables.
    pub sysvars: bool,
    /// Include names containing the text anywhere, not only at the start.
    pub mid_string: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true, sysvars: true, mid_string: true }
    }
}

/// How often each command was started from the command line (command id, count).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage(pub Vec<(String, u32)>);

impl Usage {
    pub fn count(&self, id: &str) -> u32 {
        self.0.iter().find(|(c, _)| c.eq_ignore_ascii_case(id)).map_or(0, |(_, n)| *n)
    }
    /// One more use of command `id`.
    pub fn note(&mut self, id: &str) {
        let id = id.to_ascii_lowercase();
        match self.0.iter_mut().find(|(c, _)| *c == id) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => {
                if self.0.len() >= MAX_USAGE {
                    // The least used one makes room.
                    if let Some(i) = self.0.iter().enumerate().min_by_key(|(_, (_, n))| *n).map(|(i, _)| i) {
                        self.0.swap_remove(i);
                    }
                }
                self.0.push((id, 1));
            }
        }
    }
}

/// The named objects a prompt asks for, recognised from its message: (kind, words in the message).
const NAMED: [(&str, &[&str]); 11] = [
    ("Layer state", &["layer state"]),
    ("Layer", &["layer name", "name of layer", "layer(s)", "layers to", "layer to"]),
    ("Block", &["block name", "name of block", "block to"]),
    ("Text style", &["text style", "style name"]),
    ("Dimension style", &["dimension style", "dimstyle"]),
    ("Multileader style", &["multileader style", "mleader style"]),
    ("Table style", &["table style"]),
    ("Linetype", &["linetype"]),
    ("Layout", &["layout name", "name of layout", "layout to"]),
    ("View", &["view name", "named view", "view to"]),
    ("Group", &["group name", "name of group", "group to"]),
];

/// The kind of named object prompt `message` asks for, if any.
pub fn named_kind(message: &str) -> Option<&'static str> {
    let m = message.to_ascii_lowercase();
    NAMED.iter().find(|(_, words)| words.iter().any(|w| m.contains(w))).map(|(k, _)| *k)
}

/// The names of objects of `kind` in drawing `d`.
pub fn names(d: &cadaddy_engine::doc::Drawing, kind: &str) -> Vec<String> {
    match kind {
        "Layer" => d.layers.iter().map(|l| l.name.clone()).collect(),
        "Layer state" => d.layer_states.iter().map(|l| l.name.clone()).collect(),
        "Block" => d.blocks.values().filter(|b| !b.name.starts_with('*') && !b.anonymous && !b.name.contains('|')).map(|b| b.name.clone()).collect(),
        "Text style" => d.text_styles.iter().map(|s| s.name.clone()).collect(),
        "Dimension style" => d.dim_styles.iter().map(|s| s.name.clone()).collect(),
        "Multileader style" => d.mleader_styles.iter().map(|s| s.name.clone()).collect(),
        "Table style" => d.table_styles.iter().map(|s| s.name.clone()).collect(),
        "Linetype" => d.linetypes.iter().map(|l| l.name.clone()).collect(),
        "Layout" => std::iter::once("Model".to_string()).chain(d.layouts.iter().map(|l| l.name.clone())).collect(),
        "View" => d.views.iter().map(|v| v.name.clone()).collect(),
        "Group" => d.groups.iter().map(|g| g.name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// Names (of a prompt's kind) for `text`: those starting with it, then (with mid-string search)
/// those containing it; shorter first, then alphabetically.
pub fn complete_names(text: &str, kind: &str, all: &[String], mid_string: bool) -> Vec<Suggestion> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Vec::new();
    }
    let mut starts: Vec<&String> = all.iter().filter(|n| n.to_ascii_lowercase().starts_with(&t)).collect();
    starts.sort_by_key(|n| (n.len(), n.to_ascii_lowercase()));
    let mut inside: Vec<&String> = if mid_string {
        all.iter().filter(|n| !n.to_ascii_lowercase().starts_with(&t) && n.to_ascii_lowercase().contains(&t)).collect()
    } else {
        Vec::new()
    };
    inside.sort_by_key(|n| (n.len(), n.to_ascii_lowercase()));
    starts
        .into_iter()
        .chain(inside)
        .take(MAX)
        .map(|n| Suggestion { id: n.clone(), display: n.clone(), label: kind.to_string(), kind: Kind::Name })
        .collect()
}

/// Commands, aliases and system variables for `text` typed at the command prompt.
pub fn complete_commands(text: &str, settings: &Settings, usage: &Usage, sysvars: &[String]) -> Vec<Suggestion> {
    if !settings.enabled || text.is_empty() || text.contains(' ') {
        return Vec::new();
    }
    let p = text.to_ascii_lowercase();
    let specs = cadaddy_engine::command_specs();
    let listed = |c: &cadaddy_engine::CommandSpec| !c.id.contains('.');
    let mut out: Vec<Suggestion> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let cmd = |c: &cadaddy_engine::CommandSpec, display: String| Suggestion {
        id: c.id.to_ascii_uppercase(),
        display,
        label: c.label.to_string(),
        kind: Kind::Command,
    };
    // 1. The exact command or alias.
    let resolved = cadaddy_engine::cmd::resolve_alias(&p);
    if let Some(c) = cadaddy_engine::cmd::find_command(&resolved).filter(|c| listed(c)) {
        let display = if c.id == p { c.id.to_ascii_uppercase() } else { format!("{} ({})", p.to_ascii_uppercase(), c.id.to_ascii_uppercase()) };
        out.push(cmd(c, display));
        seen.push(c.id);
    }
    // 2. Commands whose name or an alias starts with the text: the most used first, then the
    //    shorter name.
    let mut starts: Vec<(u32, usize, String, &cadaddy_engine::CommandSpec)> = Vec::new();
    for c in specs.iter().filter(|c| listed(c) && !seen.contains(&c.id)) {
        if c.id.starts_with(&p) {
            starts.push((usage.count(c.id), c.id.len(), c.id.to_ascii_uppercase(), c));
        } else if let Some(a) = c.aliases.iter().copied().filter(|a| a.starts_with(&p)).min_by_key(|a| (a.len(), *a)) {
            starts.push((usage.count(c.id), a.len(), format!("{} ({})", a.to_ascii_uppercase(), c.id.to_ascii_uppercase()), c));
        }
    }
    starts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, _, display, c) in starts {
        out.push(cmd(c, display));
        seen.push(c.id);
    }
    // 3. System variables starting with the text.
    let var = |n: &String| Suggestion {
        id: n.to_ascii_uppercase(),
        display: n.to_ascii_uppercase(),
        label: "System variable".into(),
        kind: Kind::SystemVariable,
    };
    let up = p.to_ascii_uppercase();
    if settings.sysvars {
        let mut vars: Vec<&String> =
            sysvars.iter().filter(|v| v.to_ascii_uppercase().starts_with(&up) && !seen.iter().any(|s| s.eq_ignore_ascii_case(v))).collect();
        vars.sort_by_key(|v| (v.len(), v.to_ascii_uppercase()));
        out.extend(vars.into_iter().map(var));
    }
    // 4. Mid-string: commands (then variables) containing the text elsewhere.
    if settings.mid_string && p.len() >= 2 {
        let mut inside: Vec<(u32, usize, &cadaddy_engine::CommandSpec)> = specs
            .iter()
            .filter(|c| listed(c) && !seen.contains(&c.id) && !c.id.starts_with(&p) && c.id.contains(&p))
            .map(|c| (usage.count(c.id), c.id.len(), c))
            .collect();
        inside.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.id.cmp(b.2.id)));
        out.extend(inside.into_iter().map(|(_, _, c)| cmd(c, c.id.to_ascii_uppercase())));
        if settings.sysvars {
            let mut vars: Vec<&String> = sysvars
                .iter()
                .filter(|v| {
                    let u = v.to_ascii_uppercase();
                    !u.starts_with(&up) && u.contains(&up)
                })
                .collect();
            vars.sort_by_key(|v| (v.len(), v.to_ascii_uppercase()));
            out.extend(vars.into_iter().map(var));
        }
    }
    out.truncate(MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ranks_prefix_matches_and_the_exact_match_stays_first() {
        let set = Settings::default();
        let none = Usage::default();
        let plain = complete_commands("di", &set, &none, &[]);
        assert_eq!(plain[0].id, "DIST", "DI is DIST's alias: {plain:?}");
        // DIMLINEAR used often: it moves ahead of the other DI… commands (not ahead of the alias).
        let mut u = Usage::default();
        for _ in 0..5 {
            u.note("dimlinear");
        }
        let ranked = complete_commands("di", &set, &u, &[]);
        assert_eq!(ranked[0].id, "DIST");
        assert_eq!(ranked[1].id, "DIMLINEAR", "{ranked:?}");
        assert_eq!(u.count("DIMLINEAR"), 5);
    }

    #[test]
    fn system_variables_and_mid_string_search() {
        let vars: Vec<String> = ["ORTHOMODE", "OSMODE", "GRIDMODE", "SNAPMODE"].iter().map(|s| s.to_string()).collect();
        let set = Settings::default();
        let r = complete_commands("orthom", &set, &Usage::default(), &vars);
        assert!(r.iter().any(|s| s.id == "ORTHOMODE" && s.kind == Kind::SystemVariable), "{r:?}");
        // Mid-string: "mode" finds the variables ending in it, and commands containing it.
        let m = complete_commands("mode", &set, &Usage::default(), &vars);
        assert!(m.iter().any(|s| s.id == "GRIDMODE"), "{m:?}");
        let off = Settings { mid_string: false, ..set };
        assert!(!complete_commands("mode", &off, &Usage::default(), &vars).iter().any(|s| s.id == "GRIDMODE"));
        let no_vars = Settings { sysvars: false, ..set };
        assert!(!complete_commands("orthom", &no_vars, &Usage::default(), &vars).iter().any(|s| s.kind == Kind::SystemVariable));
        assert!(complete_commands("li", &Settings { enabled: false, ..set }, &Usage::default(), &vars).is_empty());
    }

    #[test]
    fn named_objects_complete_at_their_prompts() {
        assert_eq!(named_kind("Enter layer name to make current"), Some("Layer"));
        assert_eq!(named_kind("Enter block name or [?]"), Some("Block"));
        assert_eq!(named_kind("Enter name of new layer state"), Some("Layer state"));
        assert_eq!(named_kind("Specify first point"), None);
        let layers: Vec<String> = ["0", "Walls", "Walkway", "Doors", "Fire walls"].iter().map(|s| s.to_string()).collect();
        let r = complete_names("wal", "Layer", &layers, true);
        let ids: Vec<&str> = r.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["Walls", "Walkway", "Fire walls"]);
        assert_eq!(complete_names("wal", "Layer", &layers, false).len(), 2);
        assert!(complete_names("", "Layer", &layers, true).is_empty());
    }
}
