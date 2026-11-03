//! What the trades' documents say (`bim.atoms.docs`): the facts the pieces are laid out from,
//! read out of submittals and specification sections by their wording, each with its file,
//! page and the words it was read from.
//!
//! A document's trade is what its name and words are about (concrete mix, post-tensioning,
//! joists, trusses, insulation, paint, flooring, door hardware, toilet accessories…). What is
//! read from each:
//! - concrete: the mix and its strength (`4000 PSI`), post-tensioning, the strand (`1/2"
//!   diameter, seven wire`), tendon and chair spacing (`CHAIRS @ 4'-0" O.C.`), slab thickness,
//!   vapor barrier (`15 MIL`), welded wire style, gypsum underlayment thickness;
//! - framing: floor framing (an I-joist or floor truss document) with its spacing, depth and the
//!   joists its layout lists (`J1 - 14 @ 24"`), roof trusses with their spacing (`Spacing: 24.0`)
//!   and the quantities their designs list (`Qty: 2`);
//! - finishes: paint coats (`two (2) coats`), flooring products and their piece size;
//! - insulation: the R-values listed;
//! - openings: door hardware sets and the openings each lists, their size and frame;
//! - specialties: toilet accessories by tag, model and quantity, and which rooms they are for
//!   (the document's title: guest rooms, public toilets).
//!
//! A document read again in a later revision replaces the earlier's quantities (a quantity is
//! kept once per thing it counts, from the last file read in path order).

use super::*;

/// Most files read, pages per file, facts kept.
const MAX_FILES: usize = 400;
const MAX_FACTS: usize = 2_000;
/// Most bytes of text read from a file.
const MAX_TEXT: usize = 8_000_000;

/// One thing a document says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Fact {
    pub trade: String,
    /// What it is (`concrete.strength_psi`, `framing.joists_listed`, `specialties.accessory`).
    pub key: String,
    pub value: String,
    pub number: Option<f64>,
    /// The file and page.
    pub source: String,
    /// The words it was read from.
    pub quote: String,
    /// What it is for: a hardware set's id, an accessory's tag and rooms.
    pub scope: String,
}

impl Fact {
    fn new(key: &str, value: impl Into<String>, number: Option<f64>, source: &str, quote: &str) -> Fact {
        Fact {
            trade: key.split('.').next().unwrap_or_default().to_string(),
            key: key.to_string(),
            value: value.into().chars().take(160).collect(),
            number: number.filter(|n| n.is_finite()),
            source: source.chars().take(200).collect(),
            quote: quote.trim().chars().take(160).collect(),
            scope: String::new(),
        }
    }
    fn scoped(mut self, scope: &str) -> Fact {
        self.scope = scope.chars().take(120).collect();
        self
    }
    pub(crate) fn to_json(&self) -> Value {
        let mut j = json!({"trade": self.trade, "key": self.key, "value": self.value, "source": self.source, "quote": self.quote});
        if let Some(n) = self.number {
            j["number"] = json!(n);
        }
        if !self.scope.is_empty() {
            j["scope"] = json!(self.scope);
        }
        j
    }
    fn from_json(v: &Value) -> Option<Fact> {
        let t = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(200).collect::<String>();
        let key = t("key");
        if key.is_empty() {
            return None;
        }
        Some(Fact {
            trade: t("trade"),
            key,
            value: t("value"),
            number: v.get("number").and_then(Value::as_f64).filter(|n| n.is_finite()),
            source: t("source"),
            quote: t("quote"),
            scope: t("scope"),
        })
    }
    /// Its source as a property's: a submittal, a specification section or a print.
    pub(crate) fn source_of(&self) -> Source {
        let doc = if self.quote.is_empty() { self.source.clone() } else { format!("{} (\"{}\")", self.source, self.quote.chars().take(60).collect::<String>()) };
        Source::from_doc(doc_kind(&self.source), &doc)
    }
}

/// The facts a spec keeps.
pub(crate) fn facts_of(sp: &BuildingSpec) -> Vec<Fact> {
    sp.trades.get("facts").and_then(Value::as_array).map(|a| a.iter().take(MAX_FACTS).filter_map(Fact::from_json).collect()).unwrap_or_default()
}

/// The trade rule a fact's number sets, when it sets one.
pub(crate) fn rule_path(key: &str) -> Option<&'static str> {
    Some(match key {
        "concrete.strand_in" => "concrete.tendon_strand_in",
        "concrete.chair_spacing_ft" => "concrete.chair_spacing_ft",
        "concrete.tendon_spacing_ft" => "concrete.tendon_spacing_ft",
        "concrete.vapor_mil" => "concrete.vapor_barrier_mil",
        "framing.joist_spacing_in" => "framing.joist_spacing_in",
        "framing.joist_depth_in" => "framing.joist_depth_in",
        "framing.truss_spacing_in" => "framing.truss_spacing_in",
        "finishes.paint_coats" => "finishes.paint_coats",
        _ => return None,
    })
}

/// What a read gave.
#[derive(Default)]
pub(crate) struct Read {
    pub files: Vec<Value>,
    pub facts: Vec<Fact>,
    pub unread: Vec<Value>,
}

/// The trades a document is about, by the words of its name (and first page).
fn trades_of(name: &str) -> Vec<&'static str> {
    let up = name.to_ascii_uppercase();
    let mut out = Vec::new();
    let has = |ws: &[&str]| ws.iter().any(|w| up.contains(w));
    if has(&["CONCRETE", "MIX DESIGN", "POST-TENSION", "POST TENSION", "PT SLAB", "TENDON", "REINFORC", "SLAB DESIGN", "UNDERLAYMENT"]) {
        out.push("concrete");
    }
    if has(&["JOIST", "TRUSS", "EWP", "FRAMING", "CARPENTRY", "SHEATHING"]) {
        out.push("framing");
    }
    if has(&["INSULATION"]) {
        out.push("insulation");
    }
    if has(&["PAINT", "COATING", "FLOORING", "FLOOR TILE", "RESILIENT", "CARPET", "CEILING TILE", "ACOUSTICAL"]) {
        out.push("finishes");
    }
    if has(&["GYPSUM", "DRYWALL", "GYP BOARD", "GYP. BOARD"]) {
        out.push("drywall");
    }
    if has(&["DOOR", "HARDWARE", "FRAMES", "WINDOW"]) {
        out.push("openings");
    }
    if has(&["TOILET ACCESSOR", "BATH ACCESSOR", "ACCESSORIES"]) {
        out.push("specialties");
    }
    out
}

/// Every PDF and text file under the paths (folders searched 5 deep), sorted.
fn files_under(paths: &[String]) -> Vec<String> {
    fn walk(dir: &std::path::Path, depth: usize, out: &mut Vec<String>) {
        if depth > 5 || out.len() >= MAX_FILES * 4 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut items: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
        items.sort();
        for p in items {
            if p.is_dir() {
                walk(&p, depth + 1, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf") || x.eq_ignore_ascii_case("txt")) {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    let mut out = Vec::new();
    for given in paths.iter().take(64) {
        let p = std::path::Path::new(given.trim());
        if p.is_dir() {
            walk(p, 0, &mut out);
        } else if p.is_file() {
            out.push(given.trim().to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A file's text: a text file as it is, a PDF's pages (`--- page N of M ---` between them).
fn text_of(path: &str) -> std::result::Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 400_000_000 {
        return Err("file over 400 MB".into());
    }
    if path.to_ascii_lowercase().ends_with(".txt") {
        let mut t = String::from_utf8_lossy(&bytes).to_string();
        t.truncate(t.char_indices().nth(MAX_TEXT).map_or(t.len(), |(i, _)| i));
        return Ok(t);
    }
    super::super::spec_import::pdf_text(&bytes, path).map_err(|e| e.to_string())
}

/// Read the trades' facts from the files at `paths`.
pub(crate) fn read(_s: &mut Session, paths: &[String]) -> Read {
    let mut out = Read::default();
    let files = files_under(paths);
    let mut read_n = 0usize;
    for f in files {
        let name = std::path::Path::new(&f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        // The document's trades by its own name and its folder's (a project's export names the
        // package: `Building Concrete Mix/Rev 1/…`).
        let parent = std::path::Path::new(&f).parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let trades = trades_of(&format!("{name} {parent}"));
        if trades.is_empty() {
            continue;
        }
        if read_n >= MAX_FILES {
            out.unread.push(json!({"file": name, "why": format!("past the {MAX_FILES} files read")}));
            continue;
        }
        read_n += 1;
        let text = match text_of(&f) {
            Ok(t) => t,
            Err(e) => {
                out.unread.push(json!({"file": name, "why": e}));
                continue;
            }
        };
        let label = if parent.is_empty() { name.clone() } else { format!("{parent} / {name}") };
        let before = out.facts.len();
        let title = format!("{name} {parent}");
        facts_from(&text, &label, &title, &trades, &mut out.facts);
        let got = out.facts.len() - before;
        out.files.push(json!({"file": label, "trades": trades, "facts": got}));
    }
    out.facts = keep_latest(std::mem::take(&mut out.facts));
    out.facts.truncate(MAX_FACTS);
    out
}

/// One fact per thing (key and scope): the last file's (the latest revision read).
fn keep_latest(facts: Vec<Fact>) -> Vec<Fact> {
    let mut by: BTreeMap<(String, String), (usize, Fact)> = BTreeMap::new();
    for (i, f) in facts.into_iter().enumerate() {
        let k = (f.key.clone(), f.scope.clone());
        // The same document read twice (a reviewed copy beside it): the later file wins whole.
        by.insert(k, (i, f));
    }
    let mut v: Vec<(usize, Fact)> = by.into_values().collect();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, f)| f).collect()
}

/// Keep the facts in the spec.
pub(crate) fn store(sp: &mut BuildingSpec, read: &Read) {
    let mut t = if sp.trades.is_object() { sp.trades.clone() } else { json!({}) };
    t["facts"] = Value::Array(read.facts.iter().map(Fact::to_json).collect());
    t["read"] = Value::Array(read.files.iter().take(MAX_FILES).cloned().collect());
    sp.trades = t;
}

/// The page a byte offset of the text is on (`--- page N of M ---` markers).
fn page_at(text: &str, at: usize) -> Option<usize> {
    let head = text.get(..at.min(text.len()))?;
    let i = head.rfind("--- page ")?;
    let rest = head.get(i + 9..)?;
    rest.split_whitespace().next()?.parse().ok()
}

/// A number with its fraction (`1/2`, `3 1/2`, `11 7/8`, `4.5`) at the end of `s`.
fn tail_number(s: &str) -> Option<f64> {
    let tail: String = s.trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || matches!(c, '/' | ' ' | '.' | '-')).collect::<String>().chars().rev().collect();
    let tail = tail.trim().replace('-', " ");
    whole_and_fraction(&tail)
}

/// The number the last words of `t` make: a whole number, a fraction, or a whole number and a
/// fraction (`6`, `5/8`, `11 7/8`); a number before a whole number is another number (`1 6`: 6).
pub(super) fn whole_and_fraction(t: &str) -> Option<f64> {
    let parts: Vec<&str> = t.split_whitespace().collect();
    let last = *parts.last()?;
    let frac = |p: &str| -> Option<f64> {
        let (a, b) = p.split_once('/')?;
        let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        (b > 0.0).then(|| a / b)
    };
    let v = match frac(last) {
        Some(f) => f + parts.len().checked_sub(2).and_then(|i| parts.get(i)).and_then(|w| w.parse::<f64>().ok()).filter(|w| w.fract() == 0.0).unwrap_or(0.0),
        None => last.parse::<f64>().ok()?,
    };
    v.is_finite().then_some(v)
}

/// Feet and inches as printed (`4'-0"`, `4'`, `18"`), in feet.
fn feet_inches(s: &str) -> Option<f64> {
    let t = s.trim().replace(['’', '′'], "'").replace(['”', '″'], "\"");
    if let Some((f, rest)) = t.split_once('\'') {
        let ft: f64 = f.trim().parse().ok()?;
        let inch = rest.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
        let inch = if inch.is_empty() { 0.0 } else { tail_number(inch).unwrap_or(0.0) };
        return Some(ft + inch / 12.0);
    }
    let inch = t.strip_suffix('"')?;
    tail_number(inch).map(|v| v / 12.0)
}

/// The most frequent value of a list (ties: the first seen), and how often.
fn most_common(vals: &[(f64, String)]) -> Option<(f64, String, usize)> {
    let mut counts: Vec<(f64, String, usize)> = Vec::new();
    for (v, q) in vals {
        match counts.iter_mut().find(|c| (c.0 - v).abs() < 1e-6) {
            Some(c) => c.2 += 1,
            None => counts.push((*v, q.clone(), 1)),
        }
    }
    counts.into_iter().max_by(|a, b| a.2.cmp(&b.2).then(std::cmp::Ordering::Greater))
}

/// The facts a document's text gives for its trades.
fn facts_from(text: &str, label: &str, title: &str, trades: &[&str], out: &mut Vec<Fact>) {
    let src = |at: usize| match page_at(text, at) {
        Some(p) => format!("submittal {label} p.{p}"),
        None => format!("submittal {label}"),
    };
    // A specification section names itself so.
    let src = |at: usize| {
        let s = src(at);
        if title.to_ascii_uppercase().contains("SUBMITTAL") || label.to_ascii_uppercase().contains("SUBMITTAL") { s } else { s.replacen("submittal ", "specification ", 1) }
    };
    let up = text.to_ascii_uppercase();
    let lines: Vec<(usize, &str)> = {
        let mut v = Vec::new();
        let mut at = 0usize;
        for l in text.split('\n').take(400_000) {
            v.push((at, l));
            at += l.len() + 1;
        }
        v
    };
    let tup = title.to_ascii_uppercase();
    if trades.contains(&"concrete") {
        concrete_facts(text, &up, &lines, &tup, &src, out);
    }
    if trades.contains(&"framing") {
        framing_facts(&lines, &tup, &src, out);
    }
    if trades.contains(&"finishes") {
        finish_facts(&lines, &tup, &src, out);
    }
    if trades.contains(&"insulation") {
        let mut rs: Vec<(f64, String)> = Vec::new();
        for (at, l) in &lines {
            let u = l.to_ascii_uppercase();
            let mut rest = u.as_str();
            while let Some(i) = rest.find("R-") {
                let num: String = rest.get(i + 2..).unwrap_or_default().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                if let Ok(v) = num.parse::<f64>()
                    && (3.0..=60.0).contains(&v)
                    && rs.iter().all(|(x, _)| (x - v).abs() > 1e-6)
                {
                    rs.push((v, src(*at)));
                    out.push(Fact::new("insulation.r_value", format!("R-{num}"), Some(v), &src(*at), l));
                }
                rest = rest.get(i + 2..).unwrap_or_default();
            }
        }
    }
    if trades.contains(&"openings") {
        hardware_facts(&lines, &src, out);
    }
    if trades.contains(&"specialties") {
        accessory_facts(&lines, &tup, &src, out);
    }
}

fn concrete_facts(text: &str, up: &str, lines: &[(usize, &str)], title: &str, src: &dyn Fn(usize) -> String, out: &mut Vec<Fact>) {
    // Strength: the most named `NNNN PSI` of 2500-10000 in a mix document.
    let mixdoc = title.contains("MIX") || up.contains("MIX DESIGN");
    if mixdoc {
        let mut psis: Vec<(f64, String)> = Vec::new();
        let mut mix_line: Option<(usize, String)> = None;
        for (at, l) in lines {
            let u = l.to_ascii_uppercase();
            let compact = u.replace(' ', "");
            let mut rest = compact.as_str();
            while let Some(i) = rest.find("PSI") {
                let num: String = rest.get(..i).unwrap_or_default().chars().rev().take_while(char::is_ascii_digit).collect::<String>().chars().rev().collect();
                if let Ok(v) = num.parse::<f64>()
                    && (2500.0..=10_000.0).contains(&v)
                {
                    psis.push((v, format!("{}|{}", src(*at), l.trim())));
                    if mix_line.is_none() && (u.contains("SLAB") || u.contains("SK") || u.contains("MIX")) {
                        mix_line = Some((*at, l.trim().to_string()));
                    }
                }
                rest = rest.get(i + 3..).unwrap_or_default();
            }
        }
        if let Some((v, q, _)) = most_common(&psis) {
            let (s, quote) = q.split_once('|').unwrap_or((&q, ""));
            out.push(Fact::new("concrete.strength_psi", format!("{} psi", v.round()), Some(v), s, quote));
        }
        if let Some((at, l)) = mix_line {
            out.push(Fact::new("concrete.mix", l.clone(), None, &src(at), &l));
        }
    }
    // Post-tensioning.
    if let Some(i) = up.find("POST-TENSION").or_else(|| up.find("POST TENSION")).or_else(|| up.find("TENDON")) {
        let q = text.get(i..(i + 60).min(text.len())).unwrap_or_default().lines().next().unwrap_or_default().to_string();
        out.push(Fact::new("concrete.pt", "post-tensioned", None, &src(i), &q));
    }
    for (at, l) in lines {
        let u = l.to_ascii_uppercase().replace(['”', '″'], "\"").replace(['’', '′'], "'");
        // Strand: `1/2" DIAMETER, SEVEN WIRE` (a text copy can lose the slash: `1 2"`).
        if (u.contains("SEVEN WIRE") || u.contains("7-WIRE") || u.contains("7 WIRE") || u.contains("STRAND")) && u.contains('"') {
            if let Some(q) = u.find('"') {
                let head = u.get(..q).unwrap_or_default();
                let lost_slash = head.trim_end().ends_with(" 1 2") || head.trim_end() == "1 2";
                let v = if lost_slash { Some(0.5) } else { tail_number(head) };
                if let Some(v) = v.filter(|v| [0.375, 0.5, 0.6].iter().any(|s| (s - v).abs() < 1e-6)) {
                    out.push(Fact::new("concrete.strand_in", format!("{}\" strand", num_text(v)), Some(v), &src(*at), l));
                }
            }
        }
        // Chairs: `CHAIRS @ 4'-0" O.C.`, `CHAIR SPACING SHALL BE 4'-0" MAX`.
        if u.contains("CHAIR") && let Some(v) = spacing_after(&u, &["CHAIRS @", "CHAIR SPACING SHALL BE", "CHAIRS AT", "CHAIR SPACING"]) {
            if (1.0..=10.0).contains(&v) {
                out.push(Fact::new("concrete.chair_spacing_ft", format!("{}' o.c.", num_text(v)), Some(v), &src(*at), l));
            }
        }
        // Tendon spacing: `TENDONS @ 4'-0" O.C.`.
        if u.contains("TENDON") && u.contains("O.C") && let Some(v) = spacing_after(&u, &["TENDONS @", "TENDONS AT", "TENDON SPACING"]) {
            if (1.0..=30.0).contains(&v) {
                out.push(Fact::new("concrete.tendon_spacing_ft", format!("{}' o.c.", num_text(v)), Some(v), &src(*at), l));
            }
        }
        // Vapor barrier: `15 MIL` beside VAPOR.
        if u.contains("VAPOR") && u.contains("MIL") && let Some(i) = u.find("MIL") {
            let num: String = u.get(..i).unwrap_or_default().trim_end().trim_end_matches('-').chars().rev().take_while(char::is_ascii_digit).collect::<String>().chars().rev().collect();
            if let Ok(v) = num.parse::<f64>()
                && (4.0..=40.0).contains(&v)
            {
                out.push(Fact::new("concrete.vapor_mil", format!("{} mil", num_text(v)), Some(v), &src(*at), l));
            }
        }
        // Slab thickness: `5" SLAB`, `5" THICK SLAB`, `SLAB THICKNESS = 5"`.
        if u.contains("SLAB") && u.contains('"') {
            if let Some(v) = slab_inches(&u) {
                out.push(Fact::new("concrete.slab_thickness_in", format!("{}\" slab", num_text(v)), Some(v), &src(*at), l));
            }
        }
        // Welded wire: `6x6-W2.9xW2.9`.
        if let Some(st) = wire_style(&u) {
            out.push(Fact::new("concrete.mesh", st, None, &src(*at), l));
        }
        // Gypsum underlayment thickness.
        if u.contains("UNDERLAYMENT") && u.contains('"') && let Some(q) = u.find('"') {
            if let Some(v) = tail_number(u.get(..q).unwrap_or_default()).filter(|v| (0.25..=3.0).contains(v)) {
                out.push(Fact::new("concrete.underlayment_in", format!("{}\" underlayment", num_text(v)), Some(v), &src(*at), l));
            }
        }
    }
}

/// The spacing printed after one of `leads` (`4'-0"`, `48"`), feet.
fn spacing_after(u: &str, leads: &[&str]) -> Option<f64> {
    for lead in leads {
        if let Some(i) = u.find(lead) {
            let rest = u.get(i + lead.len()..)?.trim_start();
            let tok: String = rest.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '\'' | '"' | '-' | ' ' | '/' | '.')).collect();
            let tok = tok.trim().trim_end_matches('-').trim();
            if let Some(v) = feet_inches(tok).or_else(|| feet_inches(&format!("{tok}\""))) {
                return Some(v);
            }
        }
    }
    None
}

/// A slab thickness in a line (`5" SLAB`, `5" THICK SLAB`, `SLAB THICKNESS: 5"`), inches.
fn slab_inches(u: &str) -> Option<f64> {
    for pat in ["\" SLAB", "\" THICK SLAB", "\" CONCRETE SLAB", "\" PT SLAB", "\" POST-TENSIONED SLAB", "\" THK SLAB"] {
        if let Some(i) = u.find(pat) {
            let v = tail_number(u.get(..i)?)?;
            if (3.0..=16.0).contains(&v) {
                return Some(v);
            }
        }
    }
    let i = u.find("SLAB THICKNESS")?;
    let rest = u.get(i + 14..)?;
    let q = rest.find('"')?;
    let v = tail_number(rest.get(..q)?)?;
    (3.0..=16.0).contains(&v).then_some(v)
}

/// A welded wire style in a line (`6X6-W2.9XW2.9`).
fn wire_style(u: &str) -> Option<String> {
    let c: Vec<char> = u.chars().collect();
    for i in 0..c.len() {
        let rest: String = c.get(i..(i + 24).min(c.len()))?.iter().collect();
        let mut it = rest.splitn(2, 'X');
        let (Some(a), Some(b)) = (it.next(), it.next()) else { continue };
        if a.is_empty() || !a.chars().all(|x| x.is_ascii_digit()) || a.len() > 2 || i > 0 && c.get(i - 1).is_some_and(char::is_ascii_digit) {
            continue;
        }
        let b2: String = b.chars().take_while(|x| x.is_ascii_digit()).collect();
        let after = b.get(b2.len()..).unwrap_or_default();
        if b2.is_empty() || !(after.starts_with("-W") || after.starts_with(" W") || after.starts_with("W")) {
            continue;
        }
        let gauges: String = after.trim_start_matches(['-', ' ']).chars().take_while(|x| x.is_ascii_alphanumeric() || *x == '.').collect();
        if gauges.starts_with('W') && gauges.contains("XW") {
            return Some(format!("{a}x{b2}-{}", gauges.replace("XW", "xW")));
        }
    }
    None
}

fn framing_facts(lines: &[(usize, &str)], title: &str, src: &dyn Fn(usize) -> String, out: &mut Vec<Fact>) {
    let joist_doc = title.contains("JOIST") || title.contains("EWP");
    let truss_doc = title.contains("TRUSS");
    if joist_doc {
        let at = lines.first().map_or(0, |l| l.0);
        out.push(Fact::new("framing.floor_framing", "I-joists", None, &src(at), title.trim()));
        let mut spacings: Vec<(f64, String)> = Vec::new();
        let mut listed: BTreeMap<String, f64> = BTreeMap::new();
        let mut listed_src = String::new();
        let mut depths: Vec<(f64, String)> = Vec::new();
        for (at, l) in lines {
            let u = l.trim().to_ascii_uppercase().replace(['”', '″'], "\"");
            // A layout's label: `J1 - 14 @ 24"`: 14 joists of mark J1 at 24".
            if let Some((mark, rest)) = u.split_once(" - ")
                && mark.len() <= 6
                && mark.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && mark.chars().any(|c| c.is_ascii_digit())
                && let Some((n, sp)) = rest.split_once('@')
                && let Ok(n) = n.trim().parse::<f64>()
                && let Some(spv) = sp.trim().strip_suffix('"').and_then(|x| x.trim().parse::<f64>().ok())
                && (1.0..=500.0).contains(&n)
            {
                *listed.entry(mark.trim().to_string()).or_default() += n;
                spacings.push((spv, format!("{}|{}", src(*at), l.trim())));
                if listed_src.is_empty() {
                    listed_src = src(*at);
                }
                continue;
            }
            if (u.contains("O.C") || u.contains(" OC")) && let Some(q) = u.find('"') {
                if let Some(v) = tail_number(u.get(..q).unwrap_or_default()).filter(|v| [12.0, 16.0, 19.2, 24.0].iter().any(|s| (s - v).abs() < 1e-6)) {
                    spacings.push((v, format!("{}|{}", src(*at), l.trim())));
                }
            }
            for d in [9.5, 11.875, 14.0, 16.0, 18.0, 20.0, 24.0] {
                let txt = inches_text(d);
                if u.contains(&txt) && (u.contains("JOIST") || u.contains("TJI") || u.contains("BCI") || u.contains("LPI") || u.trim() == txt) {
                    depths.push((d, format!("{}|{}", src(*at), l.trim())));
                }
            }
        }
        if let Some((v, q, _)) = most_common(&spacings) {
            let (s, quote) = q.split_once('|').unwrap_or((&q, ""));
            out.push(Fact::new("framing.joist_spacing_in", format!("{}\" o.c.", num_text(v)), Some(v), s, quote));
        }
        if let Some((v, q, _)) = most_common(&depths) {
            let (s, quote) = q.split_once('|').unwrap_or((&q, ""));
            out.push(Fact::new("framing.joist_depth_in", inches_text(v), Some(v), s, quote));
        }
        if !listed.is_empty() {
            let total: f64 = listed.values().sum();
            let marks = listed.iter().map(|(m, n)| format!("{m} {n}")).collect::<Vec<_>>().join(", ");
            out.push(Fact::new("framing.joists_listed", format!("{total} joists ({marks})"), Some(total), &listed_src, "the layout's labels (mark - count @ spacing)"));
        }
    }
    if truss_doc {
        let at = lines.first().map_or(0, |l| l.0);
        out.push(Fact::new("framing.roof_framing", "trusses", None, &src(at), title.trim()));
        let mut spacings: Vec<(f64, String)> = Vec::new();
        let mut qty = 0.0;
        let mut q_src = String::new();
        for (at, l) in lines {
            let u = l.trim().to_ascii_uppercase();
            if let Some(rest) = u.strip_prefix("SPACING:") {
                if let Ok(v) = rest.trim().trim_end_matches('"').trim().parse::<f64>()
                    && (12.0..=96.0).contains(&v)
                {
                    spacings.push((v, format!("{}|{}", src(*at), l.trim())));
                }
            }
            if let Some(rest) = u.strip_prefix("QTY:") {
                if let Ok(v) = rest.trim().parse::<f64>()
                    && (1.0..=500.0).contains(&v)
                {
                    qty += v;
                    if q_src.is_empty() {
                        q_src = src(*at);
                    }
                }
            }
        }
        if let Some((v, q, _)) = most_common(&spacings) {
            let (s, quote) = q.split_once('|').unwrap_or((&q, ""));
            out.push(Fact::new("framing.truss_spacing_in", format!("{}\" o.c.", num_text(v)), Some(v), s, quote));
        }
        if qty > 0.0 {
            out.push(Fact::new("framing.trusses_listed", format!("{qty} trusses"), Some(qty), &q_src, "the truss designs' Qty"));
        }
    }
}

fn finish_facts(lines: &[(usize, &str)], title: &str, src: &dyn Fn(usize) -> String, out: &mut Vec<Fact>) {
    if title.contains("PAINT") || title.contains("COATING") {
        let mut coats: Vec<(f64, String)> = Vec::new();
        let mut primer = false;
        for (at, l) in lines {
            let u = l.to_ascii_uppercase();
            primer |= u.contains("PRIMER");
            for (w, n) in [("ONE", 1.0), ("TWO", 2.0), ("THREE", 3.0), ("1", 1.0), ("2", 2.0), ("3", 3.0)] {
                for pat in [format!("{w} COATS"), format!("{w} (2) COATS"), format!("{w} FINISH COATS"), format!("({w}) COATS")] {
                    if u.contains(&pat) && !(w == "TWO" && pat.contains("(2)") && n != 2.0) {
                        coats.push((n, format!("{}|{}", src(*at), l.trim())));
                    }
                }
            }
        }
        if let Some((v, q, _)) = most_common(&coats) {
            let (s, quote) = q.split_once('|').unwrap_or((&q, ""));
            let total = v + if primer { 1.0 } else { 0.0 };
            out.push(Fact::new("finishes.paint_coats", format!("{} coats{}", num_text(total), if primer { " (a primer and the finish coats)" } else { "" }), Some(total), s, quote));
        }
    }
    if title.contains("FLOOR") || title.contains("RESILIENT") || title.contains("CARPET") {
        for (at, l) in lines {
            let u = l.to_ascii_uppercase().replace(['”', '″'], "\"").replace('×', "X");
            let kind = ["LUXURY VINYL", "LVT", "VINYL PLANK", "VINYL TILE", "CARPET TILE", "SHEET VINYL", "PORCELAIN", "CERAMIC"].into_iter().find(|w| u.contains(w));
            let Some(kind) = kind else { continue };
            // A piece size: `7" X 48"`, `24" X 24"`.
            if let Some(i) = u.find("\" X ").or_else(|| u.find("\"X")) {
                let a = tail_number(u.get(..i).unwrap_or_default());
                let rest = u.get(i + 1..).unwrap_or_default().trim_start_matches([' ', 'X']).trim_start();
                let q = rest.find('"');
                let b = q.and_then(|q| tail_number(rest.get(..q).unwrap_or_default()));
                if let (Some(a), Some(b)) = (a, b)
                    && (2.0..=96.0).contains(&a)
                    && (2.0..=96.0).contains(&b)
                {
                    out.push(Fact::new("finishes.flooring_piece_sf", format!("{kind} {}\"x{}\"", num_text(a), num_text(b)), Some(a * b / 144.0), &src(*at), l).scoped(kind));
                    continue;
                }
            }
            out.push(Fact::new("finishes.flooring", kind.to_ascii_lowercase(), None, &src(*at), l).scoped(kind));
        }
    }
}

fn hardware_facts(lines: &[(usize, &str)], src: &dyn Fn(usize) -> String, out: &mut Vec<Fact>) {
    let mut i = 0;
    let mut total = 0.0;
    let mut first = String::new();
    while i < lines.len() {
        let (at, l) = lines.get(i).copied().unwrap_or((0, ""));
        let u = l.trim().to_ascii_uppercase();
        let Some(id) = u.strip_prefix("HARDWARE SET#:").or_else(|| u.strip_prefix("HARDWARE SET #:")).or_else(|| u.strip_prefix("HARDWARE SET:")).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()) else {
            i += 1;
            continue;
        };
        // The openings it lists (`63 Single`, `2 Pair`) until its item table, its size and frame.
        let mut openings = 0.0;
        let mut size = String::new();
        let mut frame = String::new();
        let mut j = i + 1;
        while j < lines.len() && j < i + 120 {
            let t = lines.get(j).map_or("", |x| x.1).trim();
            let tu = t.to_ascii_uppercase();
            if tu.starts_with("HARDWARE SET") || tu.starts_with("TOT QTY") {
                break;
            }
            let mut it = tu.split_whitespace();
            if let (Some(n), Some(w)) = (it.next(), it.next())
                && (w == "SINGLE" || w == "PAIR")
                && let Ok(n) = n.parse::<f64>()
            {
                // The last count before `Size:` is the set's total (the rows above it are its
                // doors by hand).
                if lines.get(j + 1).is_some_and(|x| x.1.trim().eq_ignore_ascii_case("size:")) {
                    openings = n;
                }
            }
            if tu == "SIZE:" {
                size = lines.get(j + 1).map_or("", |x| x.1).trim().to_string();
                frame = lines.get(j + 2).map_or("", |x| x.1).trim().to_string();
            }
            j += 1;
        }
        if openings > 0.0 {
            total += openings;
            if first.is_empty() {
                first = src(at);
            }
            let mut f = Fact::new("openings.hardware_set", format!("set {id}: {openings} openings {size} {frame}").trim().to_string(), Some(openings), &src(at), &format!("{size} {frame}"));
            f.scope = id.clone();
            out.push(f);
        }
        i = j.max(i + 1);
    }
    if total > 0.0 {
        out.push(Fact::new("openings.doors_listed", format!("{total} openings in the hardware sets"), Some(total), &first, "the hardware schedule's sets"));
    }
}

fn accessory_facts(lines: &[(usize, &str)], title: &str, src: &dyn Fn(usize) -> String, out: &mut Vec<Fact>) {
    let rooms = if ["GUEST", "UNIT", "DWELLING", "APARTMENT", "SUITE"].iter().any(|w| title.contains(w)) {
        "unit rooms"
    } else if ["PUBLIC", "COMMON"].iter().any(|w| title.contains(w)) {
        "public toilets"
    } else {
        "rooms not named"
    };
    let tag_ok = |t: &str| {
        let t = t.trim();
        t.len() <= 16 && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && t.contains('-') && t.chars().any(|c| c.is_ascii_digit())
    };
    for (at, l) in lines {
        let u = l.trim().to_ascii_uppercase();
        // `7 EACH - TAG: MODEL / TAG: MODEL`: each tag that many.
        if let Some((n, rest)) = u.split_once(" EACH - ")
            && let Ok(n) = n.trim().parse::<f64>()
        {
            for part in rest.split('/') {
                if let Some((tag, model)) = part.split_once(':')
                    && tag_ok(tag)
                {
                    out.push(Fact::new("specialties.accessory", format!("{} {}", tag.trim(), model.trim()), Some(n), &src(*at), l).scoped(&format!("{} for {rooms}", tag.trim())));
                }
            }
            continue;
        }
        // `TAG: MODEL QTY-N` (several on a line split by `/`).
        if u.contains("QTY") {
            for part in u.split(" / ") {
                let Some((tag, rest)) = part.split_once(':') else { continue };
                if !tag_ok(tag) {
                    continue;
                }
                let Some(qi) = rest.find("QTY") else { continue };
                let model = rest.get(..qi).unwrap_or_default().trim().to_string();
                let num: String = rest.get(qi + 3..).unwrap_or_default().trim_start_matches(['-', ':', ' ']).chars().take_while(char::is_ascii_digit).collect();
                if let Ok(n) = num.parse::<f64>() {
                    out.push(Fact::new("specialties.accessory", format!("{} {model}", tag.trim()), Some(n), &src(*at), part).scoped(&format!("{} for {rooms}", tag.trim())));
                }
            }
        }
    }
}

/// The documents' quantities beside the model's (from its index).
pub(crate) fn compare(facts: &[Fact], ix: &super::super::mepatoms::Index, level: Option<&str>) -> Vec<Value> {
    let q = |trade: &str, kind: &str, item: Option<&str>| {
        let f = super::super::mepatoms::Filter {
            trade: Some(trade.into()),
            kind: Some(kind.into()),
            item: item.map(str::to_string),
            level: level.map(str::to_string),
            ..super::super::mepatoms::Filter::default()
        };
        ix.query(&f, None).0
    };
    let pct = |doc: f64, model: f64| if doc > 0.0 { Some(((model - doc) / doc * 1000.0).round() / 10.0) } else { None };
    let mut out = Vec::new();
    for f in facts.iter().take(MAX_FACTS) {
        let Some(n) = f.number else { continue };
        let (what, model, query) = match f.key.as_str() {
            "framing.joists_listed" => ("floor joists", q("framing", "joist", None).count as f64, json!({"trade": "framing", "kind": "joist"})),
            "framing.trusses_listed" => ("roof trusses", q("framing", "truss", None).count as f64, json!({"trade": "framing", "kind": "truss"})),
            "openings.doors_listed" => ("doors (hardware sets' openings)", q("openings", "door", None).count as f64, json!({"trade": "openings", "kind": "door"})),
            "specialties.accessory" => {
                let tag = f.value.split_whitespace().next().unwrap_or_default().to_string();
                let t = q("specialties", "accessory", Some(&tag));
                ("toilet accessory", t.pieces(), json!({"trade": "specialties", "kind": "accessory", "item": tag}))
            }
            _ => continue,
        };
        let mut j = json!({"what": what, "documents": n, "documents_say": f.value, "source": f.source, "model": model, "model_query": query});
        if let Some(p) = pct(n, model) {
            j["difference_pct"] = json!(p);
        }
        if level.is_some() {
            j["note"] = json!("the documents' number is the whole job's; the model's is the level asked");
        }
        out.push(j);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(text: &str, title: &str) -> Vec<Fact> {
        let mut out = Vec::new();
        let trades = trades_of(title);
        facts_from(text, "Sample Package / doc.txt", &format!("{title} submittal"), &trades, &mut out);
        out
    }

    fn num(f: &[Fact], key: &str) -> Option<f64> {
        f.iter().find(|x| x.key == key).and_then(|x| x.number)
    }

    #[test]
    fn concrete_mix_and_pt_design_read() {
        let mix = "--- page 1 of 2 ---\nCONCRETE MIX DESIGN\nMIX NUMBER:\n3100 - 5SK 3500PSI - Slab on Grade\nstrength 3500 psi\n--- page 2 of 2 ---\nother 3500 psi";
        let f = facts(mix, "Sample Concrete Mix");
        assert_eq!(num(&f, "concrete.strength_psi"), Some(3500.0));
        assert!(f.iter().any(|x| x.key == "concrete.mix" && x.value.contains("Slab on Grade")));
        let pt = "POST-TENSIONING STEEL SHALL BE 1 2\" DIAMETER, SEVEN WIRE, GRADE 270KSI STRAND\nTENDON SUPPORT\nCHAIRS @ 3'-6\" O.C.\nTENDONS @ 4'-0\" O.C. EACH WAY\n6\" SLAB WITH 10 MIL VAPOR RETARDER\n6x6-W1.4xW1.4 AT TOPPING";
        let f = facts(pt, "Sample Post-Tension Slab Design");
        assert_eq!(num(&f, "concrete.strand_in"), Some(0.5));
        assert_eq!(num(&f, "concrete.chair_spacing_ft"), Some(3.5));
        assert_eq!(num(&f, "concrete.tendon_spacing_ft"), Some(4.0));
        assert_eq!(num(&f, "concrete.slab_thickness_in"), Some(6.0));
        assert_eq!(num(&f, "concrete.vapor_mil"), Some(10.0));
        assert!(f.iter().any(|x| x.key == "concrete.pt"));
        assert_eq!(f.iter().find(|x| x.key == "concrete.mesh").map(|x| x.value.as_str()), Some("6x6-W1.4xW1.4"));
    }

    #[test]
    fn joists_trusses_paint_and_flooring_read() {
        let j = "Joist spacing\nA1 - 10 @ 19.2\"\nA1 - 12 @ 19.2\"\nB2 - 3 @ 19.2\"\n14\" TJI 230\n16\" o.c.";
        let f = facts(j, "Sample I-Joist");
        assert_eq!(num(&f, "framing.joists_listed"), Some(25.0));
        assert_eq!(num(&f, "framing.joist_spacing_in"), Some(19.2));
        assert_eq!(num(&f, "framing.joist_depth_in"), Some(14.0));
        let t = "Truss: A1\nQty: 3\nSpacing: 24.0 \"\nTruss: B1\nQty: 2\nSpacing: 24.0 \"";
        let f = facts(t, "Sample Roof Trusses");
        assert_eq!(num(&f, "framing.trusses_listed"), Some(5.0));
        assert_eq!(num(&f, "framing.truss_spacing_in"), Some(24.0));
        let p = "Interior Primer: latex\nWalls receive two (2) coats of latex\nCeilings: two (2) coats";
        let f = facts(p, "Sample Interior Painting");
        assert_eq!(num(&f, "finishes.paint_coats"), Some(3.0));
        let fl = "LUXURY VINYL PLANK 9\" X 60\" 20 MIL";
        let f = facts(fl, "Sample Flooring");
        let sf = num(&f, "finishes.flooring_piece_sf").unwrap();
        assert!((sf - 3.75).abs() < 1e-9);
    }

    #[test]
    fn hardware_sets_and_accessories_read() {
        let hw = "Hardware Set#: 3\nQty Units\nDoor#\n10 Single\nD1\n5 Single\nD2\n15 Single\nSize:\n3-0 X 7-0 X 1 3/4\nWood Door X HM Frame\nTot Qty\n45\nHardware Set#: 7\n2 Pair\nSize:\n6-0 X 7-0\nHM Door X HM Frame\nTot Qty\n";
        let f = facts(hw, "Sample Doors, Frames and Hardware");
        let sets: Vec<&Fact> = f.iter().filter(|x| x.key == "openings.hardware_set").collect();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].number, Some(15.0));
        assert_eq!(sets[0].scope, "3");
        assert_eq!(num(&f, "openings.doors_listed"), Some(17.0));
        let acc = "Robe Hook\nRH-10:  AB123   QTY-40\n4 EACH - GB-1: CD18 / GB-2: CD24\nTB-5: XY1 QTY-2 / TB-6: XY2 QTY-3";
        let f = facts(acc, "Sample Guest Bath Accessories");
        let a: Vec<&Fact> = f.iter().filter(|x| x.key == "specialties.accessory").collect();
        assert_eq!(a.len(), 5, "{a:?}");
        assert!(a.iter().any(|x| x.value == "RH-10 AB123" && x.number == Some(40.0) && x.scope.contains("unit rooms")));
        assert!(a.iter().any(|x| x.value.starts_with("GB-2") && x.number == Some(4.0)));
        assert!(a.iter().any(|x| x.value.starts_with("TB-6") && x.number == Some(3.0)));
    }

    #[test]
    fn later_revisions_replace_and_texts_parse() {
        let a = Fact::new("framing.trusses_listed", "10", Some(10.0), "rev 1", "");
        let b = Fact::new("framing.trusses_listed", "12", Some(12.0), "rev 2", "");
        let k = keep_latest(vec![a, b]);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].number, Some(12.0));
        assert_eq!(feet_inches("4'-0\""), Some(4.0));
        assert_eq!(feet_inches("3'-6\""), Some(3.5));
        assert_eq!(feet_inches("18\""), Some(1.5));
        assert_eq!(tail_number("x 11 7/8"), Some(11.875));
        assert_eq!(tail_number("PLANK LVT-1 6"), Some(6.0));
        assert_eq!(tail_number("x 5/8"), Some(0.625));
        assert_eq!(wire_style("WWF 6X6-W2.9XW2.9 TYP"), Some("6x6-W2.9xW2.9".into()));
        assert_eq!(wire_style("ROOM 6X6 NOTHING"), None);
        assert!(trades_of("Sample Toilet Accessories").contains(&"specialties"));
        assert!(trades_of("Electrical Gear").is_empty());
        let f = Fact::new("concrete.vapor_mil", "10 mil", Some(10.0), "submittal X p.2", "q");
        assert_eq!(Fact::from_json(&f.to_json()), Some(f));
        assert_eq!(page_at("--- page 3 of 9 ---\nabc", 22), Some(3));
    }
}
