//! Product data onto the model's elements (`bim.products`, Q6136).
//!
//! Once the model's objects are made and the submittals are on them (`bim.submittals`), each
//! device, fixture and piece of equipment gets a `product` record: what the specifications require
//! for its kind (approved makers and whether others may be offered, rating, configuration,
//! colour, features: each with its section and page), what a submittal approved for it (maker,
//! catalog number and which submittal; or that no submittal covers its kind), and what that
//! resolves to. Kinds no section and no submittal speak for are listed with why.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::Handle;
use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.products";
/// Specification PDFs read in one call.
const MAX_SPEC_FILES: usize = 400;
/// Elements given a record, at most.
const MAX_ELEMENTS: usize = 500_000;
/// Requirements kept from one section.
const MAX_REQUIREMENTS: usize = 400;
/// Pages read of a file holding many sections (a project manual), at most.
const MAX_BOOK_PAGES: u32 = 3_000;
/// Pages read of a file to tell whether it is a specification.
const SNIFF_PAGES: u32 = 12;
/// Files opened to tell whether they are specifications, at most (a job's search).
const MAX_SNIFFED: usize = 40;
/// Bigger files aren't opened to tell what they are (drawings; a manual is named or smaller).
const MAX_SNIFF_BYTES: u64 = 64 * 1024 * 1024;
/// Folder entries a job's search looks at, at most.
const MAX_SEARCHED: usize = 5_000;
/// A folder holding more entries than this is no project's folder (a downloads or temporary
/// folder): the search beside a folder in it goes by the names beside it only.
const MAX_BESIDE: usize = 60;
/// The CSI divisions whose sections speak for the model's devices, fixtures and equipment.
const PRODUCT_DIVISIONS: &[&str] = &["22", "23", "26", "27", "28"];

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Apply Product Data", run)
        .params("{specs: [path] (specification section PDFs or text, a project manual, or folders of them, searched below), sections?: [\"26 27 26\"…] (only these)} → {sections: [{section, file, requirements}] (those with products), sections_read: [\"26 27 26 WIRING DEVICES\"…] (every section read; a project manual's product-division sections each), files: n, requirements: n, elements: n, by_kind: {kind: {elements, with_spec, with_submittal}}, no_product_data: {kind: why}, unread: [{file, error}]}: each device, fixture and equipment element gets a `product` record (JSON text) {required (spec: makers, substitution, rating, configuration, colour, features; section + page), approved (submittal maker/catalog + which, or \"no submittal for this kind\"), resolved}, and its approved_makers, color, configuration, rating and required_features as properties with their sources; a person's values stay")
        .menu(&["Tools", "Building", "Apply Product Data"])]
}

/// What a specification section requires of one product.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Requirement {
    /// The element kind it is for (`receptacle`, `gfci_receptacle`, `switch`, `ptac`…).
    pub kind: String,
    /// The product as the section names it (`TOGGLE SWITCHES: Switches, 120/277 V, 20 A: Three Way`).
    pub product: String,
    /// `26 27 26 WIRING DEVICES`.
    pub section: String,
    /// Where in the section (`262726 - 2`, the page's footer; else the file's page).
    pub page: String,
    pub makers: Vec<String>,
    /// Whether others may be offered (`listed makers only`, `listed makers or equal`).
    pub substitution: String,
    pub rating: Option<String>,
    pub config: Option<String>,
    pub features: Vec<String>,
    pub color: Option<String>,
    /// The colour rule as printed (`white unless otherwise indicated or required by NFPA 70`).
    pub color_rule: Option<String>,
}

/// A heading's level in a section's outline: `2.2`, `A.`, `1.`, `a.`, `1)`.
fn outline_level(line: &str) -> Option<(usize, &str)> {
    let t = line.trim();
    let (head, rest) = t.split_once(char::is_whitespace)?;
    let rest = rest.trim();
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let level = if let Some((a, b)) = head.split_once('.').filter(|(a, b)| digits(a) && digits(b)) {
        let _ = (a, b);
        0
    } else if let Some(x) = head.strip_suffix('.') {
        if x.len() == 1 && x.chars().all(|c| c.is_ascii_uppercase()) {
            1
        } else if digits(x) && x.len() <= 2 {
            2
        } else if x.len() == 1 && x.chars().all(|c| c.is_ascii_lowercase()) {
            3
        } else {
            return None;
        }
    } else if head.strip_suffix(')').is_some_and(|x| digits(x) && x.len() <= 2) {
        4
    } else {
        return None;
    };
    Some((level, rest))
}

/// `125 V, 20 A` (or `120/277 V, 20 A`) in a text.
fn rating_in(t: &str) -> Option<String> {
    let u = t.to_ascii_uppercase();
    let words: Vec<&str> = u.split([',', ';', ':']).map(str::trim).collect();
    let volts = words.iter().position(|w| w.ends_with(" V") && w.trim_end_matches(" V").chars().all(|c| c.is_ascii_digit() || c == '/'))?;
    let amps = words.get(volts + 1).filter(|w| w.ends_with(" A") && w.trim_end_matches(" A").chars().all(|c| c.is_ascii_digit()))?;
    Some(format!("{}, {}", words.get(volts)?, amps))
}

/// The features a product's text requires.
fn features_in(t: &str) -> Vec<String> {
    let u = t.to_ascii_uppercase();
    let mut out = Vec::new();
    for (key, name) in [
        ("TAMPER-RESISTANT", "tamper-resistant"),
        ("TAMPER RESISTANT", "tamper-resistant"),
        ("WEATHER-RESISTANT", "weather-resistant"),
        ("GFCI", "GFCI"),
        ("GROUND-FAULT", "GFCI"),
        ("NON-FEED-THROUGH", "non-feed-through"),
        ("LED-LIGHTED", "lighted handle"),
        ("USB", "USB"),
        ("DUAL TECHNOLOGY", "dual technology"),
        ("PASSIVE INFRARED", "passive infrared"),
    ] {
        if u.contains(key) && !out.iter().any(|o: &String| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// A product's configuration from its name (`duplex`, `three way`…).
fn config_in(t: &str) -> Option<String> {
    let u = t.to_ascii_uppercase();
    [
        ("THREE WAY", "three way"),
        ("3-WAY", "three way"),
        ("FOUR WAY", "four way"),
        ("TWO POLE", "two pole"),
        ("SINGLE POLE", "single pole"),
        ("QUAD", "quad"),
        ("DUPLEX", "duplex"),
        ("SIMPLEX", "simplex"),
        ("SINGLE RECEPTACLE", "single"),
    ]
    .iter()
    .find(|(k, _)| u.contains(k))
    .map(|(_, v)| (*v).to_string())
}

/// The element kind a specified product is for, from its name (its article's and its own).
pub(crate) fn kind_of_product(name: &str) -> Option<&'static str> {
    let u = name.to_ascii_uppercase();
    let has = |w: &str| u.contains(w);
    Some(if has("GFCI") && has("RECEPTACLE") {
        "gfci_receptacle"
    } else if has("RECEPTACLE") {
        "receptacle"
    } else if has("SENSOR") || has("OCCUPANCY") {
        "occupancy_sensor"
    } else if has("SWITCH") && !has("ENCLOSED") && !has("DISCONNECT") && !has("TRANSFER") {
        "switch"
    } else if has("WALL PLATE") || has("COVER PLATE") {
        "wall_plate"
    } else if has("PACKAGED TERMINAL") || has("PTAC") {
        "ptac"
    } else if has("POWER VENTILATOR") || has("EXHAUST FAN") || has("CEILING-MOUNTED VENTILATOR") || has("CEILING MOUNTED VENTILATOR") {
        "exhaust_fan"
    } else if has("REGISTER") || has("GRILLE") {
        "grille"
    } else if has("DIFFUSER") {
        "diffuser"
    } else if has("WATER CLOSET") || has("TOILET") {
        "water_closet"
    } else if has("LAVATOR") {
        "lavatory"
    } else if has("SINK") {
        "sink"
    } else if has("SHOWER") || has("BATHTUB") || has("TUB") {
        "shower"
    } else if has("SMOKE") {
        "smoke_detector"
    } else if has("HEAT DETECTOR") {
        "heat_detector"
    } else if has("NOTIFICATION") || has("HORN") || has("STROBE") {
        "fire_alarm_notification"
    } else if has("PANELBOARD") {
        "panel"
    } else if has("LUMINAIRE") || has("LED ") || has("LIGHTING FIXTURE") {
        "luminaire"
    } else if has("WATER HEATER") {
        "water_heater"
    } else {
        return None;
    })
}

/// The section's number and title from its header (`SECTION 262726 - WIRING DEVICES`).
fn section_header(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("SECTION ")?;
    let (num, title) = rest.split_once(" - ").unwrap_or((rest, ""));
    let digits: String = num.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    let main: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if main.len() != 6 {
        return None;
    }
    let tail = digits.get(6..).unwrap_or("");
    Some(format!("{} {} {}{tail} {}", &main[..2], &main[2..4], &main[4..6], title.trim()).trim().to_string())
}

/// A specification text's sections in order: each one's number and title (as
/// [`section_header`] gives it) and its lines. A project manual holds hundreds; a section file
/// one. The text before the first header (a cover, the contents) is no section; a header printed
/// again (a page header) goes on with its section.
pub(crate) fn split_sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    // A page break seen before a header: it goes with the section the header starts.
    let mut pending = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("--- page ") {
            pending.push_str(line);
            pending.push('\n');
            continue;
        }
        if let Some(sec) = section_header(line.trim()) {
            let num = |s: &str| s.split(' ').take(3).collect::<Vec<_>>().join(" ");
            if out.last().is_none_or(|(cur, _)| num(cur) != num(&sec)) {
                out.push((sec, std::mem::take(&mut pending)));
            }
        }
        if let Some((_, body)) = out.last_mut() {
            body.push_str(&pending);
            pending.clear();
            body.push_str(line);
            body.push('\n');
        } else {
            pending.clear();
        }
    }
    out
}

/// Whether a file or folder name says it is a specification or a project manual: a word that
/// starts `spec` (`03_Specifications`, `Spec Book`, `ProjectSpecs`; not `Inspection`), or
/// `project manual`.
pub(crate) fn names_a_spec(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let up = name.to_uppercase();
    if up.contains("PROJECT MANUAL") || up.contains("PROJECT_MANUAL") || up.contains("PROJECTMANUAL") {
        return true;
    }
    (0..chars.len()).any(|i| {
        let word = chars.get(i..i + 4).is_some_and(|w| w.iter().collect::<String>().eq_ignore_ascii_case("spec"));
        let starts = match i.checked_sub(1).and_then(|j| chars.get(j)) {
            None => true,
            Some(prev) if !prev.is_alphabetic() => true,
            // `ProjectSpecs`: a capital after a small letter starts a word.
            Some(prev) => prev.is_lowercase() && chars.get(i).is_some_and(|c| c.is_uppercase()),
        };
        word && starts
    })
}

/// A file name's section number when it starts with one (`26 27 26 - Wiring Devices.pdf`,
/// `262726.pdf` → `26 27 26`).
fn section_of_name(name: &str) -> Option<String> {
    let digits: String = name.chars().take(12).filter(char::is_ascii_digit).take(6).collect();
    (digits.len() == 6 && name.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then(|| format!("{} {} {}", &digits[..2], &digits[2..4], &digits[4..6]))
}

/// Whether a text reads as a specification: a CSI section header (`SECTION 26 05 19 - …`) with
/// its `PART 1`, or two sections' headers.
fn reads_as_spec(text: &str) -> bool {
    let mut headers: BTreeSet<String> = BTreeSet::new();
    let mut part1 = false;
    for line in text.lines().map(str::trim) {
        if let Some(h) = section_header(line) {
            headers.insert(h.split(' ').take(3).collect::<Vec<_>>().join(" "));
        }
        part1 |= line.to_ascii_uppercase().starts_with("PART 1");
    }
    headers.len() >= 2 || (part1 && !headers.is_empty())
}

/// Where a job's specifications were found when it was given none: the paths (folders or
/// files) and how each was told.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SpecsFound {
    pub paths: Vec<String>,
    pub how: Vec<String>,
}

/// The project's specifications near a job's documents: in the prints folder and the folders
/// beside it, and beside the submittals' folder. A folder or file named like a specification or
/// project manual (`03_Specifications`, `Project Manual.pdf`; a folder over files named like
/// one elsewhere, an RFI's attachment); else, with `sniff`, the PDF and
/// text files there (not the prints themselves, nor the submittals) whose first pages carry CSI
/// section headers. Searched three folders down, capped; beside a folder that sits in a crowded
/// one (more than [`MAX_BESIDE`] entries: downloads, a temporary folder), by name only.
pub(crate) fn find_project_specs(prints: &str, submittals: &[String], sniff: bool) -> SpecsFound {
    use std::path::{Path, PathBuf};
    let is_doc = |p: &Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf") || x.eq_ignore_ascii_case("txt"));
    let dir_of = |p: &Path| if p.is_dir() { Some(p.to_path_buf()) } else { p.parent().map(Path::to_path_buf) };
    let key = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let entries = |d: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let prints_dir = dir_of(Path::new(prints.trim()));
    let sub_dirs: Vec<PathBuf> = submittals.iter().filter_map(|s| dir_of(Path::new(s.trim()))).collect();
    let sub_keys: Vec<PathBuf> = sub_dirs.iter().map(|d| key(d)).collect();
    let prints_key = prints_dir.as_deref().map(key);
    // (root, whether its own name counts, whether its files may be opened, how deep to go)
    let mut roots: Vec<(PathBuf, bool, bool, usize)> = Vec::new();
    if let Some(d) = &prints_dir {
        roots.push((d.clone(), false, false, 3));
    }
    for d in prints_dir.iter().chain(sub_dirs.iter()) {
        let Some(parent) = d.parent() else { continue };
        let me = key(d);
        let beside = entries(parent);
        let crowded = beside.len() > MAX_BESIDE;
        for e in beside.into_iter().take(MAX_SEARCHED) {
            if key(&e) != me {
                roots.push((e, true, !crowded, if crowded { 0 } else { 3 }));
            }
        }
    }
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut out = SpecsFound::default();
    // Named like a specification: folders (a project's specifications folder), and files.
    let (mut folders, mut files): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut looked = 0usize;
    // Folders and files to walk: (path, depth, its name counts, may it be opened, how deep).
    let mut stack: Vec<(PathBuf, usize, bool, bool, usize)> =
        roots.into_iter().rev().map(|(p, named, open, deep)| (p, 0, named, open, deep)).collect();
    while let Some((p, depth, named, open, deep)) = stack.pop() {
        looked += 1;
        if looked > MAX_SEARCHED || folders.len() + files.len() >= MAX_SPEC_FILES {
            break;
        }
        let k = key(&p);
        // The submittals are not the specification; the prints folder is walked once.
        if sub_keys.iter().any(|s| k.starts_with(s)) || (depth > 0 && prints_key.as_ref() == Some(&k)) || !seen.insert(k) {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if named && names_a_spec(&name) {
            let has_docs = if p.is_dir() {
                let mut found = Vec::new();
                spec_walk(&p, &mut found, 0);
                found.iter().any(|f| is_doc(f))
            } else {
                is_doc(&p)
            };
            if has_docs {
                let to = if p.is_dir() { &mut folders } else { &mut files };
                to.push(p.to_string_lossy().to_string());
                continue;
            }
        }
        if p.is_dir() {
            if depth < deep {
                for e in entries(&p).into_iter().rev() {
                    stack.push((e, depth + 1, true, open, deep));
                }
            }
        } else if open && is_doc(&p) && candidates.len() < MAX_SNIFFED * 4 {
            candidates.push(p);
        }
    }
    // A specifications folder is the project's specification: files named like one elsewhere
    // (an RFI's attachment, a product's spec sheet) count only without one.
    let named = if folders.is_empty() { files } else { folders };
    if !named.is_empty() || !sniff {
        out.how = named.iter().map(|p| format!("{p}: named like a specification")).collect();
        out.paths = named;
        return out;
    }
    let mut opened = 0usize;
    for c in candidates {
        if opened >= MAX_SNIFFED {
            break;
        }
        if std::fs::metadata(&c).map_or(true, |m| m.len() > MAX_SNIFF_BYTES) {
            continue;
        }
        opened += 1;
        let Ok(bytes) = crate::readfile::read(&c) else { continue };
        let text = if c.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            super::spec_import::pdf_text_pages(&bytes, &c.to_string_lossy(), SNIFF_PAGES).unwrap_or_default()
        } else {
            String::from_utf8_lossy(bytes.get(..bytes.len().min(256 * 1024)).unwrap_or_default()).to_string()
        };
        if reads_as_spec(&text) {
            out.paths.push(c.to_string_lossy().to_string());
            out.how.push(format!("{}: its pages carry CSI section headers", c.to_string_lossy()));
        }
    }
    out
}

/// Every file below `dir` (sorted, four folders down, capped).
fn spec_walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 4 || out.len() >= MAX_SPEC_FILES * 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            spec_walk(&p, out, depth + 1);
        } else {
            out.push(p);
        }
    }
}

/// What a specification section's PART 2 requires of each product it lists makers for: one
/// requirement per product with a makers list, its kind from its name, its rating and features
/// from its own text or its article's, the section's device colour rule. `text`: the section's
/// lines (`--- page N of M ---` between pages); `file`: what to call it when it has no header.
pub(crate) fn read_spec_section(text: &str, file: &str) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = Vec::new();
    let mut section = String::new();
    let mut page = String::new();
    let mut in_products = false;
    // The open headings: (level, text), the deepest last.
    let mut heads: Vec<(usize, String)> = Vec::new();
    // The article's whole text (for a rating said once for all its products).
    let mut article_text = String::new();
    let mut color: Option<(String, String)> = None;
    // A makers list being read: its requirement, the level of its items.
    let mut open: Option<(Requirement, usize)> = None;
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let close = |open: &mut Option<(Requirement, usize)>, out: &mut Vec<Requirement>| {
        if let Some((r, _)) = open.take().filter(|(r, _)| !r.makers.is_empty())
            && out.len() < MAX_REQUIREMENTS
        {
            out.push(r);
        }
    };
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines.get(i).copied().unwrap_or_default();
        i += 1;
        if line.starts_with("--- page ") {
            continue;
        }
        if let Some(sec) = section_header(line) {
            section = sec;
            continue;
        }
        // A page's footer (`WIRING DEVICES 262726 - 2`): where the next lines are.
        if let Some((_, tail)) = line.rsplit_once(' ')
            && tail.chars().all(|c| c.is_ascii_digit())
            && line.contains(" - ")
            && line
                .split(" - ")
                .next()
                .is_some_and(|h| h.split_whitespace().last().is_some_and(|w| w.len() >= 6 && w.chars().all(|c| c.is_ascii_digit() || c == '.')))
        {
            // The footer ends the page it is printed on: the next page is one more.
            let num = line.split(" - ").next().and_then(|h| h.split_whitespace().last()).unwrap_or("");
            page = format!("{num} - {}", tail.parse::<u32>().map_or(0, |p| p + 1));
            continue;
        }
        let up = line.to_ascii_uppercase();
        if up.starts_with("PART 2") {
            in_products = true;
            if page.is_empty() {
                page = format!("{} - ?", section.split(' ').take(3).collect::<String>());
            }
            continue;
        }
        if up.starts_with("PART 3") {
            close(&mut open, &mut out);
            in_products = false;
            continue;
        }
        if !in_products {
            continue;
        }
        let Some((level, rest)) = outline_level(line) else {
            // A line carried on (a wrapped list item or heading).
            if let Some((r, _)) = open.as_mut()
                && let Some(last) = r.makers.last_mut()
                && !up.contains("MANUFACTURERS")
            {
                last.push(' ');
                last.push_str(line.trim_end_matches('.'));
            } else if let Some((_, h)) = heads.last_mut() {
                h.push(' ');
                h.push_str(line);
            }
            article_text.push(' ');
            article_text.push_str(line);
            continue;
        };
        if let Some((r, item_level)) = open.as_mut() {
            if level == *item_level && !rest.to_ascii_uppercase().starts_with("MANUFACTURERS") {
                r.makers.push(rest.trim_end_matches('.').trim().to_string());
                continue;
            }
            close(&mut open, &mut out);
        }
        if level == 0 {
            article_text.clear();
        }
        heads.retain(|(l, _)| *l < level);
        heads.push((level, rest.to_string()));
        article_text.push(' ');
        article_text.push_str(rest);
        let rest_up = rest.to_ascii_uppercase();
        // The section's device colour.
        if rest_up.contains("DEVICE COLOR")
            || rest_up.contains("WIRING DEVICES CONNECTED TO NORMAL POWER")
            || (heads.iter().any(|(_, h)| h.to_ascii_uppercase().contains("FINISHES")) && rest_up.contains("COLOR"))
        {
            let mut said = rest.to_string();
            let mut j = i;
            while let Some(next) = lines.get(j).filter(|l| outline_level(l).is_none() && !l.starts_with("--- page")) {
                said.push(' ');
                said.push_str(next);
                j += 1;
            }
            let lower = said.to_ascii_lowercase();
            if let Some(c) =
                ["white", "ivory", "light almond", "almond", "gray", "grey", "black", "brown", "red", "stainless"].iter().find(|c| lower.contains(*c))
            {
                color = Some(((*c).to_string(), said.split_once(':').map_or(said.as_str(), |(_, r)| r).trim().to_string()));
            }
        }
        if rest_up.starts_with("MANUFACTURERS") {
            // Its product: the headings over it, its article's title first.
            let names: Vec<&str> = heads
                .iter()
                .filter(|(l, _)| *l < level)
                .map(|(_, h)| h.split(':').next().unwrap_or(h).trim())
                .filter(|h| !h.is_empty() && !h.to_ascii_uppercase().starts_with("GENERAL DESCRIPTION"))
                .collect();
            let product = names.join(": ");
            // Its own text: the headings' full text (a rating said by its paragraph).
            let own: String = heads.iter().filter(|(l, _)| *l < level).map(|(_, h)| h.as_str()).collect::<Vec<_>>().join(" ");
            let mut said = rest.to_string();
            let mut j = i;
            while let Some(next) = lines.get(j).filter(|l| outline_level(l).is_none() && !l.starts_with("--- page")) {
                said.push(' ');
                said.push_str(next);
                j += 1;
            }
            i = j;
            let said_up = said.to_ascii_uppercase();
            let substitution = if said_up.contains("NOT LIMITED TO") || said_up.contains("OR EQUAL") || said_up.contains("OR APPROVED EQUAL") {
                "listed makers or equal"
            } else if said_up.contains("ONE OF THE FOLLOWING") || said_up.contains("BY THE FOLLOWING") {
                "listed makers only (subject to compliance)"
            } else {
                "as listed"
            };
            let Some(kind) = kind_of_product(&product) else { continue };
            let item_level = if level >= 4 { 4 } else { level + 1 };
            // A list on the same line (`Manufacturers: Acuity; Wattstopper.`).
            let inline: Vec<String> = said
                .split_once(':')
                .map(|(_, r)| r)
                .filter(|r| !r.to_ascii_uppercase().contains("FOLLOWING") && !r.to_ascii_uppercase().contains("SUBJECT TO"))
                .map(|r| r.split(';').map(|m| m.trim().trim_end_matches('.').to_string()).filter(|m| !m.is_empty()).collect())
                .unwrap_or_default();
            open = Some((
                Requirement {
                    kind: kind.to_string(),
                    product,
                    section: if section.is_empty() { file.to_string() } else { section.clone() },
                    page: page.clone(),
                    makers: inline,
                    substitution: substitution.to_string(),
                    rating: rating_in(&own).or_else(|| rating_in(&article_text)),
                    config: config_in(&own),
                    features: features_in(&own),
                    color: None,
                    color_rule: None,
                },
                item_level,
            ));
        }
    }
    close(&mut open, &mut out);
    // The section's colour is every product's (its finishes article comes after them).
    if let Some((c, rule)) = color {
        for r in &mut out {
            r.color = Some(c.clone());
            r.color_rule = Some(rule.clone());
        }
    }
    out
}

/// The specification files under `paths` (files or folders, searched below): in a folder, the
/// files whose name starts with a section number of the product divisions (`26 27 26 - Wiring
/// Devices.pdf`); a folder with no section files gives its own PDF and text files and those below
/// named like a specification (a project manual, read section by section); any PDF/text file
/// given by name.
fn spec_files(paths: &[String], only: &[String]) -> Vec<String> {
    let is_doc = |p: &std::path::Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf") || x.eq_ignore_ascii_case("txt"));
    let mut out = Vec::new();
    for given in paths {
        let path = std::path::Path::new(given.trim());
        if path.is_dir() {
            let mut found = Vec::new();
            spec_walk(path, &mut found, 0);
            let docs: Vec<std::path::PathBuf> = found.into_iter().filter(|p| is_doc(p)).collect();
            let name_of = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let numbered: Vec<(&std::path::PathBuf, String)> = docs.iter().filter_map(|p| section_of_name(&name_of(p)).map(|s| (p, s))).collect();
            if numbered.is_empty() {
                // A manual: the folder's own files, and the ones below named like one.
                for p in docs.iter().filter(|p| p.parent() == Some(path) || names_a_spec(&name_of(p))) {
                    out.push(p.to_string_lossy().to_string());
                }
                continue;
            }
            for (p, sec) in numbered {
                let wanted =
                    if only.is_empty() { PRODUCT_DIVISIONS.iter().any(|d| sec.starts_with(d)) } else { only.iter().any(|o| sec == o.trim()) };
                if wanted {
                    out.push(p.to_string_lossy().to_string());
                }
            }
        } else if is_doc(path) {
            out.push(given.trim().to_string());
        }
    }
    out.dedup();
    out.truncate(MAX_SPEC_FILES);
    out
}

/// An element's kind for its product: its own (a device's as read), refined by its mark (a GFCI
/// receptacle, a three-way switch, an occupancy sensor switch), its name (a plumbing fixture's).
fn product_kind(e: &cadaddy_doc::Entity, store: &props::Props) -> Option<(String, Option<String>)> {
    let base = super::elemprops::element_kind(e)?;
    let kind = super::elemprops::reported_kind(e, base.clone());
    let text = |k: &str| store.get(k).filter(|p| p.has_value()).map(|p| p.text().to_ascii_uppercase()).unwrap_or_default();
    let mark = {
        let m = text("type_mark");
        if m.is_empty() { text("type") } else { m }
    };
    let name = format!("{} {} {}", text("description"), text("name"), text("device_type"));
    let k = kind.as_str();
    // The code rules' decisions (bim.coderules): GFCI by circuit, tamper-resistant in guest rooms.
    let decided = text("receptacle_type");
    let tr = (text("tamper_resistant") == "YES").then(|| TAMPER_RESISTANT.to_string());
    Some(match k {
        "receptacle" | "quad_receptacle" | "ceiling_receptacle" | "gfci_receptacle" if decided == "GFCI" => ("gfci_receptacle".into(), tr),
        "receptacle" | "quad_receptacle" | "ceiling_receptacle" | "gfci_receptacle" if decided == "STANDARD" => ("receptacle".into(), tr),
        "receptacle" | "quad_receptacle" | "ceiling_receptacle" if mark.contains("GFCI") || mark.contains("GFI") || name.contains("GFCI") => {
            ("gfci_receptacle".into(), tr)
        }
        "receptacle" | "quad_receptacle" | "ceiling_receptacle" | "gfci_receptacle" => (k.to_string(), tr),
        "switch" | "3way_switch" => {
            let config = match mark.as_str() {
                "S3" | "3W" => Some("three way"),
                "S2" => Some("two pole"),
                "S4" => Some("four way"),
                "OS" | "OS1" | "OS2" | "VS" => return Some(("occupancy_sensor".into(), None)),
                _ if k == "3way_switch" => Some("three way"),
                _ => Some("single pole"),
            };
            ("switch".into(), config.map(str::to_string))
        }
        "plumbing_fixture" | "fixture" => {
            let k2 = kind_of_product(&name).unwrap_or("plumbing_fixture");
            (k2.to_string(), None)
        }
        "equipment" | "equipment_connection" => match mark.as_str() {
            m if m.starts_with("PTAC") => ("ptac".into(), None),
            m if m.starts_with("EF") => ("exhaust_fan".into(), None),
            _ => (kind_of_product(&name).unwrap_or(k).to_string(), None),
        },
        "light_fixture" | "luminaire" | "wall_luminaire" => ("luminaire".into(), None),
        _ => (k.to_string(), None),
    })
}

/// The requirement for an element of `kind` (and `config`, a switch's): the one of its kind and
/// configuration, else the first of its kind.
fn requirement_for<'a>(reqs: &'a [Requirement], kind: &str, config: Option<&str>) -> Option<&'a Requirement> {
    let mine: Vec<&Requirement> = reqs.iter().filter(|r| r.kind == kind).collect();
    let tr =
        |r: &&Requirement| r.product.to_ascii_uppercase().contains("TAMPER") || r.features.iter().any(|f| f.to_ascii_uppercase().contains("TAMPER"));
    match config {
        // A tamper-resistant receptacle: the section's tamper-resistant product, else its kind's.
        Some(TAMPER_RESISTANT) => mine.iter().copied().find(tr).or_else(|| mine.first().copied()),
        Some(c) => mine.iter().copied().find(|r| r.config.as_deref() == Some(c)).or_else(|| mine.first().copied()),
        // Not one: a product that isn't the tamper-resistant one first.
        None => mine.iter().copied().find(|r| !tr(r)).or_else(|| mine.first().copied()),
    }
}

/// A receptacle's configuration when the code rules make it tamper-resistant.
const TAMPER_RESISTANT: &str = "tamper-resistant";

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let mut given: Vec<String> = Vec::new();
    match p.get("specs") {
        None | Some(Value::Null) => {}
        Some(Value::String(t)) => given.push(t.clone()),
        Some(Value::Array(a)) => {
            for v in a.iter().take(MAX_SPEC_FILES) {
                let t = v.as_str().ok_or_else(|| bad(CMD, "`specs` takes paths (strings)"))?;
                given.push(t.to_string());
            }
        }
        Some(_) => return Err(bad(CMD, "`specs` takes a path or a list of paths")),
    }
    let only: Vec<String> =
        p.get("sections").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).take(200).collect();
    let files = spec_files(&given, &only);
    // Every section's requirements.
    let mut reqs: Vec<Requirement> = Vec::new();
    let mut sections = Vec::new();
    let mut unread = Vec::new();
    // Every section read (those with no product list too), in order.
    let mut read: Vec<String> = Vec::new();
    for f in &files {
        let name = std::path::Path::new(f).file_stem().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        // A file not named for one section may hold many (a project manual): all its pages.
        let book = section_of_name(&name).is_none();
        let pages = if book { MAX_BOOK_PAGES } else { 200 };
        let text = if f.to_ascii_lowercase().ends_with(".pdf") {
            match crate::readfile::read(f)
                .map_err(|e| e.to_string())
                .and_then(|b| super::spec_import::pdf_text_pages(&b, f, pages).map_err(|e| e.to_string()))
            {
                Ok(t) => t,
                Err(e) => {
                    unread.push(json!({"file": name, "error": e}));
                    continue;
                }
            }
        } else {
            match crate::readfile::read(f).map_err(|e| e.to_string()).and_then(|b| String::from_utf8(b).map_err(|e| e.to_string())) {
                Ok(t) => t,
                Err(e) => {
                    unread.push(json!({"file": name, "error": e}));
                    continue;
                }
            }
        };
        let parts = split_sections(&text);
        if parts.len() <= 1 {
            let got = read_spec_section(&text, &name);
            read.push(parts.first().map_or_else(|| name.clone(), |(sec, _)| sec.clone()));
            if !got.is_empty() {
                sections.push(json!({"section": got.first().map(|r| r.section.clone()), "file": name, "requirements": got.len()}));
            }
            reqs.extend(got);
            continue;
        }
        // A manual: its sections of the product divisions (or those asked for), each on its own.
        for (sec, body) in parts {
            let num = sec.split(' ').take(3).collect::<Vec<_>>().join(" ");
            let wanted = if only.is_empty() { PRODUCT_DIVISIONS.iter().any(|d| num.starts_with(d)) } else { only.iter().any(|o| num == o.trim()) };
            if !wanted || read.len() >= MAX_SPEC_FILES * 4 {
                continue;
            }
            let got = read_spec_section(&body, &name);
            read.push(sec.clone());
            if !got.is_empty() {
                sections.push(json!({"section": sec, "file": name, "requirements": got.len()}));
            }
            reqs.extend(got);
        }
    }
    // Each product element: its record.
    let d = s.doc()?;
    let mut changes: Vec<(Handle, props::Props)> = Vec::new();
    let mut by_kind: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (e, base) in super::elemprops::elements(d).take(MAX_ELEMENTS) {
        if !matches!(base.as_str(), "device" | "light_fixture" | "fixture" | "equipment") {
            continue;
        }
        let mut store = props::read(&e.common.xdata);
        let before = store.clone();
        let Some((kind, config)) = product_kind(e, &store) else { continue };
        let req = requirement_for(&reqs, &kind, config.as_deref());
        let maker = store.get("manufacturer").filter(|p| p.has_value()).map(|p| (p.text(), p.source.clone()));
        let catalog = store.get("catalog_number").filter(|p| p.has_value()).map(|p| p.text());
        let submittal = store.get("submittal").filter(|p| p.has_value()).map(|p| p.text());
        let tally = by_kind.entry(kind.clone()).or_default();
        tally.0 += 1;
        tally.1 += usize::from(req.is_some());
        tally.2 += usize::from(maker.is_some() || submittal.is_some());
        if req.is_none() && maker.is_none() && submittal.is_none() {
            continue;
        }
        let spec_src = |r: &Requirement| Source::from_doc(SourceKind::Spec, &format!("{} p.{} ({})", r.section, r.page, r.product));
        let required = req.map_or(Value::Null, |r| {
            json!({"section": r.section, "page": r.page, "product": r.product, "makers": r.makers, "substitution": r.substitution,
                   "rating": r.rating, "configuration": r.config, "features": r.features, "color": r.color, "color_rule": r.color_rule})
        });
        let approved = match (&maker, &submittal) {
            (None, None) => json!({"none": "no submittal for this kind"}),
            _ => json!({"maker": maker.as_ref().map(|m| m.0.clone()), "catalog": catalog, "submittal": submittal}),
        };
        // Resolved: the submittal's maker and catalog; else the spec's one maker when it names one.
        let resolved_maker =
            maker.as_ref().map(|m| m.0.clone()).or_else(|| req.filter(|r| r.makers.len() == 1).and_then(|r| r.makers.first().cloned()));
        let tr = config.as_deref() == Some(TAMPER_RESISTANT);
        let config = config.filter(|c| c != TAMPER_RESISTANT);
        let mut features = req.map(|r| r.features.clone()).unwrap_or_default();
        if tr && !features.iter().any(|f| f == "TR") {
            features.push("TR".into());
        }
        let resolved = json!({
            "maker": resolved_maker, "catalog": catalog,
            "color": req.and_then(|r| r.color.clone()), "configuration": config.clone().or_else(|| req.and_then(|r| r.config.clone())),
            "rating": req.and_then(|r| r.rating.clone()), "features": features,
        });
        let src = req.map_or_else(Source::computed, spec_src);
        let mut set = |key: &str, value: Value, source: Source| {
            let keep = store.get(key).is_some_and(|old| old.source.kind == SourceKind::User);
            if !keep && !value.is_null() {
                store.set(key, Prop::new(value, source));
            }
        };
        // The record whole (as JSON text: a property holds a string, number or boolean).
        set("product", json!(json!({"required": required, "approved": approved, "resolved": resolved}).to_string()), src.clone());
        if let Some(r) = req {
            if !r.makers.is_empty() {
                set("approved_makers", json!(format!("{} ({})", r.makers.join("; "), r.substitution)), spec_src(r));
            }
            if let Some(c) = &r.color {
                set("color", json!(c), spec_src(r));
            }
            if let Some(c) = config.clone().or_else(|| r.config.clone()) {
                set("configuration", json!(c), spec_src(r));
            }
            if let Some(v) = &r.rating {
                set("rating", json!(v), spec_src(r));
            }
            if !r.features.is_empty() {
                set("required_features", json!(r.features.join(", ")), spec_src(r));
            }
            // The section's own tamper-resistant product: where the documents say TR.
            if tr && r.product.to_ascii_uppercase().contains("TAMPER") {
                set("tr_documented", json!(format!("{} p.{} ({})", r.section, r.page, r.product)), spec_src(r));
            }
        }
        if store != before {
            changes.push((e.handle, store));
        }
    }
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    if changed > 0 {
        s.touch();
    }
    let no_data: BTreeMap<&String, String> = by_kind
        .iter()
        .filter(|(_, (_, spec, sub))| *spec == 0 && *sub == 0)
        .map(|(k, (n, _, _))| (k, format!("{n} elements: no specification section read speaks for this kind and no submittal covers it")))
        .collect();
    let kinds: BTreeSet<&String> = reqs.iter().map(|r| &r.kind).collect();
    Ok(json!({
        "sections": sections,
        "sections_read": read,
        "files": files.len(),
        "requirements": reqs.len(),
        "kinds_specified": kinds,
        "elements": changed,
        "by_kind": by_kind.iter().map(|(k, (n, sp, sb))| (k.clone(), json!({"elements": n, "with_spec": sp, "with_submittal": sb}))).collect::<serde_json::Map<String, Value>>(),
        "no_product_data": no_data,
        "unread": unread,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Section 26 27 26 of the test hotel's specifications, as its pages read.
    const WIRING_DEVICES: &str = "--- page 1 of 8 ---
SECTION 262726 - WIRING DEVICES
PART 1 - GENERAL
1.1 RELATED DOCUMENTS
WIRING DEVICES 262726 - 1
--- page 2 of 8 ---
PART 2 - PRODUCTS
2.1 GENERAL WIRING-DEVICE REQUIREMENTS
A. Wiring Devices, Components, and Accessories: Listed and labeled as defined in NFPA 70,
2.2 STRAIGHT-BLADE RECEPTACLES
A. Duplex Convenience Receptacles: 125 V, 20 A; comply with NEMA WD 1, NEMA WD 6
Configuration 5-20R, UL 498, and FS W-C-596.
1. Manufacturers: Subject to compliance with requirements, provide products by one of
the following:
a. Bryant.
b. GE.
c. Hubbell Incorporated; Wiring Device-Kellems.
d. Leviton Manufacturing Co., Inc.
e. Pass & Seymour/Legrand (Pass & Seymour).
B. Tamper-Resistant Convenience Receptacles: 125 V, 20 A; comply with NEMA WD 1,
NEMA WD 6 Configuration 5-20R, UL 498, and FS W-C-596.
1. Manufacturers: Subject to compliance with requirements, provide products by one of
the following:
a. Bryant.
b. Eaton (Arrow Hart).
WIRING DEVICES 262726 - 2
--- page 3 of 8 ---
2.3 GFCI RECEPTACLES
A. General Description:
1. 125 V, 20 A, straight blade, non-feed-through type.
B. Duplex GFCI Convenience Receptacles:
1. Manufacturers: Subject to compliance with requirements, provide products by one of
the following:
a. Bryant.
b. GE.
2.4 TOGGLE SWITCHES
A. Comply with NEMA WD 1, UL 20, and FS W-S-896.
B. Switches, 120/277 V, 20 A:
1. Single Pole:
a. Manufacturers: Subject to compliance with requirements, available
manufacturers offering products that may be incorporated into the Work
include, but are not limited to the following:
1) Bryant.
2) GE.
WIRING DEVICES 262726 - 3
--- page 4 of 8 ---
3. Three Way:
a. Manufacturers: Subject to compliance with requirements, provide products by
one of the following:
1) Bryant.
2) Leviton Manufacturing Co., Inc.
2.5 WALL SWITCH SENSOR LIGHT SWITCH, DUAL TECHNOLOGY
A. Manufacturers: Subject to compliance with requirements, provide products by the following:
1. Acuity nLight
2. Legrand Wattstopper
2.8 FINISHES
A. Device Color:
1. Wiring Devices Connected to Normal Power System: white unless otherwise
indicated or required by NFPA 70 or device listing.
PART 3 - EXECUTION
3.1 INSTALLATION
A. Manufacturers: not a product list.
1. Nobody.
";

    #[test]
    fn a_wiring_devices_section_gives_each_products_makers_rating_and_colour() {
        let r = read_spec_section(WIRING_DEVICES, "26 27 26 - Wiring Devices");
        let of =
            |kind: &str, config: Option<&str>| requirement_for(&r, kind, config).cloned().unwrap_or_else(|| panic!("no {kind} {config:?} in {r:#?}"));
        let dup = r.iter().find(|q| q.product.contains("Duplex Convenience")).unwrap();
        assert_eq!(dup.kind, "receptacle");
        assert_eq!(dup.section, "26 27 26 WIRING DEVICES");
        assert_eq!(dup.page, "262726 - 2");
        assert_eq!(dup.makers.len(), 5, "{dup:?}");
        assert!(dup.makers.iter().any(|m| m.starts_with("Pass & Seymour")));
        assert_eq!(dup.rating.as_deref(), Some("125 V, 20 A"));
        assert_eq!(dup.config.as_deref(), Some("duplex"));
        assert_eq!(dup.substitution, "listed makers only (subject to compliance)");
        assert_eq!(dup.color.as_deref(), Some("white"));
        assert!(dup.color_rule.as_deref().is_some_and(|c| c.contains("unless otherwise")));
        let tr = r.iter().find(|q| q.product.contains("Tamper-Resistant")).unwrap();
        assert!(tr.features.iter().any(|f| f == "tamper-resistant"), "{tr:?}");
        let gfci = of("gfci_receptacle", None);
        assert_eq!(gfci.rating.as_deref(), Some("125 V, 20 A"), "said once for the article: {gfci:?}");
        assert_eq!(gfci.page, "262726 - 3");
        let single = of("switch", Some("single pole"));
        assert_eq!(single.rating.as_deref(), Some("120/277 V, 20 A"));
        assert_eq!(single.substitution, "listed makers or equal");
        assert_eq!(of("switch", Some("three way")).makers, ["Bryant", "Leviton Manufacturing Co., Inc"]);
        let sensor = of("occupancy_sensor", None);
        assert_eq!(sensor.makers, ["Acuity nLight", "Legrand Wattstopper"]);
        assert!(sensor.features.iter().any(|f| f == "dual technology"));
        // PART 3 lists no products.
        assert!(!r.iter().any(|q| q.makers.iter().any(|m| m == "Nobody")));
    }

    #[test]
    fn products_are_known_by_their_names() {
        assert_eq!(kind_of_product("PACKAGED TERMINAL AIR-CONDITIONERS"), Some("ptac"));
        assert_eq!(kind_of_product("STRAIGHT-BLADE RECEPTACLES: Duplex GFCI Convenience Receptacles"), Some("gfci_receptacle"));
        assert_eq!(kind_of_product("WATER CLOSETS: Water Closets, Floor Mounted"), Some("water_closet"));
        assert_eq!(kind_of_product("ENCLOSED SWITCHES: Fusible"), None);
        assert_eq!(
            section_header("SECTION 238113.11 - PACKAGED TERMINAL AIR-CONDITIONERS").as_deref(),
            Some("23 81 13.11 PACKAGED TERMINAL AIR-CONDITIONERS")
        );
    }

    /// A made-up project manual: a cover and contents, then three sections, one of them with its
    /// header printed again at the top of its next page.
    const MANUAL: &str = "--- page 1 of 6 ---
PROJECT MANUAL
SAMPLE OFFICE BUILDING
--- page 2 of 6 ---
TABLE OF CONTENTS
099123 Interior Painting
262726 Wiring Devices
--- page 3 of 6 ---
SECTION 099123 - INTERIOR PAINTING
PART 1 - GENERAL
PART 2 - PRODUCTS
2.1 MANUFACTURERS
A. Manufacturers: Subject to compliance with requirements, provide products by one of the following:
1. Paint Maker A.
INTERIOR PAINTING 099123 - 1
--- page 4 of 6 ---
SECTION 262726 - WIRING DEVICES
PART 1 - GENERAL
PART 2 - PRODUCTS
2.2 STRAIGHT-BLADE RECEPTACLES
A. Duplex Convenience Receptacles: 125 V, 20 A.
1. Manufacturers: Subject to compliance with requirements, provide products by one of the following:
a. Maker One.
WIRING DEVICES 262726 - 1
--- page 5 of 6 ---
SECTION 262726 - WIRING DEVICES
b. Maker Two.
PART 3 - EXECUTION
--- page 6 of 6 ---
SECTION 283111 - DIGITAL, ADDRESSABLE FIRE-ALARM SYSTEM
PART 1 - GENERAL
";

    #[test]
    fn a_project_manual_splits_into_its_sections() {
        let parts = split_sections(MANUAL);
        let names: Vec<&str> = parts.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(names, ["09 91 23 INTERIOR PAINTING", "26 27 26 WIRING DEVICES", "28 31 11 DIGITAL, ADDRESSABLE FIRE-ALARM SYSTEM"]);
        // The cover and contents are no section; a header printed again goes on with its section.
        assert!(!parts.iter().any(|(_, b)| b.contains("TABLE OF CONTENTS")));
        let wiring = &parts[1].1;
        assert!(wiring.starts_with("--- page 4 of 6 ---"), "{wiring}");
        let reqs = read_spec_section(wiring, "manual");
        assert_eq!(reqs.len(), 1, "{reqs:?}");
        assert_eq!(reqs[0].makers, ["Maker One", "Maker Two"], "the list runs on over the page");
        assert_eq!(reqs[0].section, "26 27 26 WIRING DEVICES");
        // One section's file is one part.
        assert_eq!(split_sections(WIRING_DEVICES).len(), 1);
        assert!(split_sections("no headers here\nat all").is_empty());
    }

    #[test]
    fn specification_names_are_told_from_others() {
        for yes in ["03_Specifications", "Spec Book", "ProjectSpecs", "SPECS", "Project Manual.pdf", "project_manual", "Specifications - Rev 2"] {
            assert!(names_a_spec(yes), "{yes}");
        }
        for no in ["Inspection Reports", "Respect", "Drawings", "02_Submittals", "Prints", "aspect ratio.pdf"] {
            assert!(!names_a_spec(no), "{no}");
        }
        assert!(reads_as_spec(MANUAL));
        assert!(reads_as_spec("SECTION 26 05 19 - LOW-VOLTAGE CONDUCTORS\nPART 1 - GENERAL\n"));
        assert!(!reads_as_spec("SUBMITTAL COVER SHEET\nSpec Section 262726\nSECTION 262726 - WIRING DEVICES\n"), "one header and no PART 1");
        assert!(!reads_as_spec("GENERAL NOTES\n1. PROVIDE BOXES.\n"));
    }

    fn tree(tag: &str, files: &[&str]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("cadaddy-findspecs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let body = if f.contains("book") { MANUAL } else { "made-up text" };
            std::fs::write(&p, body).unwrap();
        }
        root
    }

    fn found(root: &std::path::Path, prints: &str, subs: &[&str]) -> Vec<String> {
        let subs: Vec<String> = subs.iter().map(|s| root.join(s).to_string_lossy().to_string()).collect();
        let f = find_project_specs(&root.join(prints).to_string_lossy(), &subs, true);
        assert_eq!(f.paths.len(), f.how.len());
        f.paths.iter().map(|p| p.strip_prefix(&*root.to_string_lossy()).unwrap_or(p).trim_start_matches(['/', '\\']).replace('\\', "/")).collect()
    }

    #[test]
    fn a_jobs_specification_is_found_near_its_prints_and_submittals() {
        // A folder beside the prints, named like one (any number prefix, any depth of sections).
        let r = tree("beside", &["01 Drawings/E.pdf", "03 Specifications/By Division/26 Electrical/26 27 26 - Wiring Devices.pdf", "RFI/rfi 1.pdf"]);
        assert_eq!(found(&r, "01 Drawings", &[]), ["03 Specifications"]);
        // A specifications folder over files named like one elsewhere (an RFI's attachment).
        let r = tree("rfi", &["01 Drawings/E.pdf", "RFI/Open/Attachment A - Spec 26 27 26.pdf", "RFI/pole_spec.pdf", "Specs/26 27 26.pdf"]);
        assert_eq!(found(&r, "01 Drawings", &[]), ["Specs"]);
        let r = tree("rfionly", &["01 Drawings/E.pdf", "RFI/Open/Attachment A - Spec 26 27 26.pdf"]);
        assert_eq!(found(&r, "01 Drawings", &[]), ["RFI/Open/Attachment A - Spec 26 27 26.pdf"], "without a folder, the file named like one");
        // A manual in the prints folder itself, named like one.
        let r = tree("inprints", &["Prints/E.pdf", "Prints/Project Manual.pdf"]);
        assert_eq!(found(&r, "Prints", &[]), ["Prints/Project Manual.pdf"]);
        // Submittals given as files: the folders beside theirs (the prints elsewhere).
        let r = tree(
            "subs",
            &["job/drawings/E.pdf", "office/Submittals/lights.pdf", "office/Submittals/gear.pdf", "office/Specs/26 05 19 - Conductors.txt"],
        );
        assert_eq!(found(&r, "job/drawings", &["office/Submittals/lights.pdf", "office/Submittals/gear.pdf"]), ["office/Specs"]);
        // A folder of submittals is no specification, whatever its name.
        let r = tree("subnamed", &["P/E.pdf", "Spec Submittals/26 27 26 - Wiring Devices.pdf"]);
        assert!(found(&r, "P", &["Spec Submittals"]).is_empty());
        // Not named like one: a file beside the prints whose pages carry CSI section headers.
        let r = tree("sniff", &["Plans/E.txt", "Docs/notes.txt", "Docs/Owner/book.txt", "Inspection/report.txt"]);
        assert_eq!(found(&r, "Plans", &[]), ["Docs/Owner/book.txt"]);
        // The prints themselves aren't opened; an empty folder named like one is no specification.
        let r = tree("none", &["Plans/book.txt", "Plans/E.pdf", "Specs/readme.md"]);
        assert!(found(&r, "Plans", &[]).is_empty());
        // Prints in a crowded folder (downloads): the names beside them only, nothing opened
        // or searched below.
        let mut crowd: Vec<String> = (0..70).map(|i| format!("other {i:02}/x.txt")).collect();
        crowd.extend(["Plans/E.pdf".to_string(), "other 05/Specs/26 27 26.txt".into(), "loose book.txt".into()]);
        let r = tree("crowded", &crowd.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(found(&r, "Plans", &[]).is_empty());
        std::fs::create_dir_all(r.join("Job Specs")).unwrap();
        std::fs::write(r.join("Job Specs").join("26 27 26.txt"), "made-up text").unwrap();
        assert_eq!(found(&r, "Plans", &[]), ["Job Specs"]);
        // A single print file's folder is the prints folder.
        let r = tree("onefile", &["set/E.pdf", "Specifications/26 27 26.pdf"]);
        assert_eq!(found(&r, "set/E.pdf", &[]), ["Specifications"]);
    }

    #[test]
    fn a_folder_of_a_manual_gives_the_manual() {
        let r = tree("manualdir", &["Specs/Full Specifications.txt", "Specs/Brand/guide.txt", "Specs/Brand/Spec book.txt"]);
        let files = spec_files(&[r.join("Specs").to_string_lossy().to_string()], &[]);
        let names: Vec<String> = files.iter().filter_map(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string())).collect();
        assert_eq!(names, ["Spec book.txt", "Full Specifications.txt"], "the folder's own files and those named like one, not every file below");
        // Section files there: only they (of the product divisions).
        let r = tree("sectiondir", &["Specs/Full Specifications.txt", "Specs/26 27 26 - Wiring Devices.txt", "Specs/09 91 23 - Painting.txt"]);
        let files = spec_files(&[r.join("Specs").to_string_lossy().to_string()], &[]);
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].ends_with("26 27 26 - Wiring Devices.txt"));
    }
}
