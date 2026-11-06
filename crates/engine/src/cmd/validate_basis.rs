//! What each element is by the documents, what the project's rules say and what the build
//! corrected, for Validate (Q6259: judges who see only the prints fail correct decisions).
//!
//! The model already records all three ([`super::provenance`], Q6216): every built object says
//! whether the documents draw it, it was derived from a matching source, or it was inferred from
//! a rule of the code or of trade practice, with its basis; a value the build corrected for the
//! field keeps its design value beside it with the reason and the rule; the project rules carry
//! a tier (field verified, an answered RFI, a submittal, a revision, the specification) above
//! the drawings they override. This module reads them, never a second record of its own:
//! - [`Basis::annotate`] puts `provenance` (`made`, `basis`, `from`) and `corrections` on every
//!   element and run of a packet's `objects.json` (a plan element's built twin's corrections
//!   too, naming the twin);
//! - [`project_rules`] lists the project's active rules with tier and source, and what each
//!   overrode;
//! - [`prompt_section`] tells the judge to judge a correction on its stated basis, and that a
//!   print disagreeing with a correction or with a project rule above the drawings is not a fail
//!   by itself;
//! - [`Basis::screen`] makes the prescreen follow the same rule: each finding on an element
//!   carries its provenance and corrections, and a `fail` that is only the correction itself (the
//!   finding expects the design value and finds the corrected one) is `unsure`, to be judged on
//!   the correction's basis. A finding that judged the code itself (it cites the code rule)
//!   stands: it is a judgement of that basis.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::projrules::SourceTier;
use cadaddy_doc::props;
use cadaddy_doc::{Drawing, Handle};
use cadaddy_walk3d::Made;
use cadaddy_walk3d::validate::Status;
use serde_json::{Value, json};

/// Most project rules listed in `objects.json`, and in `prompt.md`.
const MAX_RULES: usize = 300;
const MAX_PROMPT_RULES: usize = 60;
/// Longest rule value or basis written (characters).
const MAX_TEXT: usize = 400;
const MAX_PROMPT_TEXT: usize = 160;
/// Most corrections listed on one element.
const MAX_CORRECTIONS: usize = 20;
/// Most entities looked at (hostile drawings).
const MAX_ENTITIES: usize = 3_000_000;

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// The words of a value, upper case, `/` and `.` kept inside them (`3/4`, `1.5`).
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '.'))
        .map(|w| w.trim_matches(['/', '.']).to_ascii_uppercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Does `hay` hold the words of `needle`, in order and side by side?
fn says(hay: &str, needle: &str) -> bool {
    let (h, n) = (tokens(hay), tokens(needle));
    !n.is_empty() && h.windows(n.len()).any(|w| w == n.as_slice())
}

/// One corrected value: the property, its design value, the value installed and why.
#[derive(Clone, Debug)]
struct Fix {
    key: String,
    design: String,
    corrected: String,
    why: String,
    /// The built twin that carries it (a plan element's), else none.
    on: Option<String>,
}

impl Fix {
    fn to_json(&self) -> Value {
        let (rule, reason) = match self.why.split_once(':') {
            // `E-GFCI NEC 210.8: why`: a rule is a few words before the first colon.
            Some((r, why)) if !r.trim().is_empty() && r.split_whitespace().count() <= 4 => (r.trim().to_string(), why.trim().to_string()),
            _ => (String::new(), self.why.trim().to_string()),
        };
        let mut j = json!({"key": self.key, "design": cut(&self.design, MAX_TEXT), "corrected": cut(&self.corrected, MAX_TEXT), "rule": rule, "reason": cut(&reason, MAX_TEXT)});
        if let Some(h) = &self.on {
            j["on"] = json!(h);
        }
        j
    }
}

/// The model's provenance and corrections, read once for a packet or a screen.
pub(crate) struct Basis<'a> {
    d: &'a Drawing,
    /// A plan element's built twins (what the build made from it).
    twins: HashMap<Handle, Vec<Handle>>,
}

impl<'a> Basis<'a> {
    pub(crate) fn of(d: &'a Drawing) -> Basis<'a> {
        let mut twins: HashMap<Handle, Vec<Handle>> = HashMap::new();
        for e in d.model.iter().take(MAX_ENTITIES) {
            if super::wallgen::made_as(e).is_some()
                && let Some(src) = super::bimlook::made_source(e)
            {
                twins.entry(src).or_default().push(e.handle);
            }
        }
        Basis { d, twins }
    }

    /// What an element is by the documents: its build mark, else (a plan element read off the
    /// prints) its placement status: placed by a rule or an agent is inferred, the rest drawn.
    fn provenance(&self, handle: &str) -> Value {
        let h = Handle::parse_hex(handle);
        let ent = h.and_then(|h| self.d.model.get(h));
        if let Some((m, b)) = ent.and_then(|e| super::provenance::resolve_one(self.d, e)) {
            let mut j = json!({"made": m.as_str(), "from": "build"});
            if !m.is_drawn() {
                j["basis"] = json!(cut(if b.trim().is_empty() { "no basis was recorded" } else { &b }, MAX_TEXT));
            }
            return j;
        }
        let Some(e) = ent else { return json!({"made": Made::Drawn.as_str(), "from": "prints"}) };
        let prov = super::validate::provenance(e);
        let status = if super::validate::has_override(e) { Some(Status::Override) } else { prov.get("status").and_then(|s| Status::parse(s)) };
        let src = prov.get("source").filter(|s| !s.trim().is_empty());
        let by = |what: &str| cut(&src.map_or_else(|| what.to_string(), |s| format!("{what}: {s}")), MAX_TEXT);
        match status {
            Some(Status::Rule) => json!({"made": Made::Inferred.as_str(), "from": "placement", "basis": by("placed by a rule")}),
            Some(Status::Agent) => json!({"made": Made::Inferred.as_str(), "from": "placement", "basis": by("an agent's edit")}),
            Some(s @ (Status::Zone | Status::FieldVerified | Status::Override)) => {
                json!({"made": Made::Drawn.as_str(), "from": "placement", "placed": s.name()})
            }
            _ => json!({"made": Made::Drawn.as_str(), "from": "prints"}),
        }
    }

    /// The corrections an element carries, and its built twins'.
    fn fixes(&self, handle: &str) -> Vec<Fix> {
        let Some(h) = Handle::parse_hex(handle) else { return Vec::new() };
        let of = |x: Handle, on: Option<String>| -> Vec<Fix> {
            self.d
                .model
                .get(x)
                .map(|e| super::provenance::corrections_of(&props::read(&e.common.xdata)))
                .unwrap_or_default()
                .into_iter()
                .map(|(key, design, corrected, why)| Fix { key, design, corrected, why, on: on.clone() })
                .collect()
        };
        let mut out = of(h, None);
        for t in self.twins.get(&h).into_iter().flatten().take(50) {
            out.extend(of(*t, Some(t.hex())));
        }
        out.truncate(MAX_CORRECTIONS);
        out
    }

    /// Put `provenance` and `corrections` on every element and run of a packet's objects; the
    /// counts (`drawn`, `derived`, `inferred`, `corrected`) go into `counts` and are returned.
    pub(crate) fn annotate(&self, objects: &mut Value) -> Value {
        let mut by: BTreeMap<&'static str, u64> = [("drawn", 0), ("derived", 0), ("inferred", 0), ("corrected", 0)].into_iter().collect();
        for list in ["elements", "runs"] {
            let Some(items) = objects.get_mut(list).and_then(Value::as_array_mut) else { continue };
            for el in items.iter_mut() {
                let Some(h) = el.get("handle").and_then(Value::as_str).map(str::to_string) else { continue };
                let p = self.provenance(&h);
                if let Some(n) = p["made"].as_str().and_then(|m| by.iter_mut().find(|(k, _)| **k == m)).map(|(_, n)| n) {
                    *n += 1;
                }
                el["provenance"] = p;
                let fixes = self.fixes(&h);
                if !fixes.is_empty() {
                    if let Some(n) = by.get_mut("corrected") {
                        *n += 1;
                    }
                    el["corrections"] = json!(fixes.iter().map(Fix::to_json).collect::<Vec<_>>());
                }
            }
        }
        let counts = json!(by);
        objects["counts"]["provenance"] = counts.clone();
        counts
    }

    /// The prescreen follows the packet's rule (see the module's words): every finding on an
    /// element carries its provenance (when not drawn) and its corrections, and a `fail` that
    /// is the correction itself becomes `unsure`, the tallies in `checked` moved with it.
    pub(crate) fn screen(&self, findings: &mut [(u8, Value)], checked: &mut BTreeMap<String, BTreeMap<String, u64>>) -> u64 {
        let mut moved = 0u64;
        for (rank, f) in findings.iter_mut() {
            let Some(h) = f.get("handle").and_then(Value::as_str).map(str::to_string) else { continue };
            let p = self.provenance(&h);
            if p["made"] != json!(Made::Drawn.as_str()) {
                f["provenance"] = p;
            }
            let fixes = self.fixes(&h);
            if fixes.is_empty() {
                continue;
            }
            f["corrections"] = json!(fixes.iter().map(Fix::to_json).collect::<Vec<_>>());
            // A finding that judged the code (it cites the code rule) judged the basis itself.
            if f["verdict"] != "fail" || f["measured"].get("code").is_some_and(|c| !c.is_null()) {
                continue;
            }
            let (expected, found) = (f["expected"].as_str().unwrap_or_default().to_string(), f["found"].as_str().unwrap_or_default().to_string());
            let Some(fix) =
                fixes.iter().find(|x| !tokens(&x.design).eq(&tokens(&x.corrected)) && says(&expected, &x.design) && says(&found, &x.corrected))
            else {
                continue;
            };
            let rule = f["rule"].as_str().unwrap_or_default().to_string();
            let was = f["issue"].as_str().unwrap_or_default().to_string();
            f["verdict"] = json!("unsure");
            f["issue"] = json!(format!(
                "corrected for the field: {} is {} where the documents give {} ({}); judge it on that basis, the documents' value alone is not a fail ({was})",
                fix.key,
                cut(&fix.corrected, MAX_PROMPT_TEXT),
                cut(&fix.design, MAX_PROMPT_TEXT),
                cut(&fix.why, MAX_TEXT)
            ));
            f["judge_on_basis"] = fix.to_json();
            *rank = 5;
            if let Some(t) = checked.get_mut(&rule) {
                if let Some(n) = t.get_mut("fail") {
                    *n = n.saturating_sub(1);
                }
                *t.entry("unsure".into()).or_default() += 1;
            }
            moved += 1;
        }
        moved
    }
}

/// Is a tier one of the documents' (or the field's) above the project rules file and the
/// built-in defaults: a value that outranks the drawings it overrides.
fn above_drawings(t: SourceTier) -> bool {
    t < SourceTier::ProjectRule
}

/// The project's active rules, sorted: domain, key, value, tier, source, who set it and why,
/// and the values it overrode.
pub(crate) fn project_rules(d: &Drawing) -> (Vec<Value>, usize) {
    let mut all = d.project_rules.list(None);
    all.sort_by(|a, b| (a.domain.as_str(), a.key.as_str()).cmp(&(b.domain.as_str(), b.key.as_str())));
    let total = all.len();
    let list = all
        .into_iter()
        .take(MAX_RULES)
        .map(|e| {
            let mut j = json!({
                "domain": e.domain.as_str(),
                "key": e.key.as_str(),
                "value": cut(&e.active.value, MAX_TEXT),
                "tier": e.active.tier.as_str(),
                "outranks_drawings": above_drawings(e.active.tier),
                "source": cut(&e.active.source, MAX_TEXT),
            });
            if !e.active.set_by.trim().is_empty() {
                j["set_by"] = json!(e.active.set_by);
            }
            if let Some(why) = e.changelog.first().map(|c| c.reason.trim()).filter(|r| !r.is_empty()) {
                j["reason"] = json!(cut(why, MAX_TEXT));
            }
            if !e.overridden.is_empty() {
                j["overrode"] = json!(
                    e.overridden
                        .iter()
                        .take(10)
                        .map(|v| json!({"value": cut(&v.value, MAX_TEXT), "tier": v.tier.as_str(), "source": cut(&v.source, MAX_TEXT)}))
                        .collect::<Vec<_>>()
                );
            }
            j
        })
        .collect();
    (list, total)
}

/// The judge's section on provenance, corrections and project rules.
pub(crate) fn prompt_section(rules: &[Value], total_rules: usize, counts: &Value) -> String {
    let n = |k: &str| counts.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut s = String::new();
    s.push_str("## What the documents draw, what was filled in, and what was corrected\n\n");
    s.push_str(&format!(
        "Drawings are incomplete and sometimes wrong; the model finishes the building and corrects it the way the field would, and says so on every object. Here: {} drawn, {} derived, {} inferred, {} with corrections.\n\n",
        n("drawn"),
        n("derived"),
        n("inferred"),
        n("corrected")
    ));
    s.push_str("- `provenance` on each element and run: `drawn` (a document draws it), `derived` (copied from a named source that matches, another floor's or a typical room's), `inferred` (laid in from a rule of the code or of trade practice where nothing draws it), with its `basis` (the source or the rule). Don't guess it from the source text. A derived or inferred object is judged on its basis: is the source or rule right for this place, and was it applied correctly? That no print draws it is not `extra` by itself.\n");
    s.push_str("- `corrections` on an element: a value the build corrected for the field, with the `design` value the documents give, the `corrected` value installed, the `reason` and the `rule`. A correction with a stated basis is judged on that basis: `pass` when the basis holds here and the corrected value meets it, `fail` only when the basis is wrong or misapplied (name which). A print showing the design value is not a fail by itself: that is what was corrected.\n");
    s.push_str("- `project_rules` in `objects.json`: the project's active rules, each with its `tier` and `source`. One with `outranks_drawings` (tier field_verified, rfi, submittal, drawing_revision or spec) outranks the drawings it overrides: judge the model against the rule's value, cite the rule, and don't fail an element for following it where a print says otherwise. A prescreen finding that a correction or a rule explains says so (`judge_on_basis`).\n\n");
    if rules.is_empty() {
        s.push_str("The project has no rules of its own.\n\n");
        return s;
    }
    s.push_str(&format!("The project rules ({total_rules}; `objects.json` `project_rules` has each in full):\n\n"));
    let mut shown: Vec<&Value> = rules.iter().collect();
    // The ones above the drawings first.
    shown.sort_by_key(|r| !r["outranks_drawings"].as_bool().unwrap_or(false));
    for r in shown.iter().take(MAX_PROMPT_RULES) {
        let src = r["source"].as_str().unwrap_or_default();
        s.push_str(&format!(
            "- `{}/{}` = {} ({}{})\n",
            r["domain"].as_str().unwrap_or_default(),
            r["key"].as_str().unwrap_or_default(),
            cut(r["value"].as_str().unwrap_or_default(), MAX_PROMPT_TEXT),
            r["tier"].as_str().unwrap_or_default(),
            if src.trim().is_empty() { String::new() } else { format!(": {}", cut(src, MAX_PROMPT_TEXT)) }
        ));
    }
    if total_rules > MAX_PROMPT_RULES.min(rules.len()) {
        s.push_str(&format!("- … {} more in `objects.json`\n", total_rules - MAX_PROMPT_RULES.min(rules.len())));
    }
    s.push('\n');
    s
}

/// `prompt` with `section` before its rules (at the end when it has none).
pub(crate) fn with_section(prompt: String, section: &str) -> String {
    match prompt.find("## Rules to check") {
        Some(i) => format!("{}{section}{}", prompt.get(..i).unwrap_or_default(), prompt.get(i..).unwrap_or_default()),
        None => prompt + "\n" + section,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_said_by_its_words_in_order() {
        assert!(says("a 3/4\" EMT conduit (keynote 4)", "3/4\""));
        assert!(!says("a 3/4\" EMT conduit", "1\""));
        assert!(says("catalog LX-100 per the submittal", "LX-100"));
        assert!(!says("catalog LX-1000", "LX-100"));
        assert!(!says("anything", ""));
    }

    #[test]
    fn a_correction_names_its_rule_apart_from_its_reason() {
        let f = Fix { key: "size".into(), design: "3/4".into(), corrected: "1".into(), why: "E-FILL NEC 358: over 40% fill".into(), on: None };
        let j = f.to_json();
        assert_eq!((j["rule"].as_str(), j["reason"].as_str()), (Some("E-FILL NEC 358"), Some("over 40% fill")));
        let g = Fix { why: "the approved submittal says LX-200, the schedule says LX-100".into(), ..f };
        assert_eq!(g.to_json()["rule"], "");
    }

    #[test]
    fn the_section_goes_before_the_rules() {
        let p = with_section("# Head\n\n## Rules to check\n\nR\n".to_string(), "## Basis\n\n");
        assert_eq!(p, "# Head\n\n## Basis\n\n## Rules to check\n\nR\n");
        assert!(with_section("no rules".into(), "## Basis\n").ends_with("## Basis\n"));
    }
}
