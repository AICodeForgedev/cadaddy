//! The shared project page (Q350): a building model anyone with the link can look at in a
//! browser, read-only, with nothing to install.
//!
//! A share bundle is a folder a web server serves as it is: the web viewer, a manifest
//! ([`ShareManifest`], `model/share.json`), the model split into a base file (levels, rooms,
//! trades, types and issues: a [`FieldPackage`] without its elements) and element parts the page
//! fetches one after another with a progress bar, and plan thumbnails. Every file name in it is
//! relative, so the bundle works from any sub-path.
//!
//! Before anything is written the package is stripped ([`strip`]): project and file names,
//! addresses, people, company names, title blocks and sheet stamps leave its properties and texts,
//! and a [`StripReport`] says what went (the report is written beside the bundle, never in it).
//!
//! A view of the model (level, mode, camera, trades shown, section box, room, element) travels as
//! a `#view=` link ([`ViewLink`]).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::field::{FieldElement, FieldPackage, FieldSpec, MAX_ELEMENTS, MAX_PACKAGE_BYTES, MAX_TEXT};

pub const SHARE_SCHEMA: &str = "cadaddy-share/1";
/// Elements per part file by default (a part is 100-200 KB).
pub const DEFAULT_CHUNK: usize = 1_000;
/// Most part files a manifest lists.
pub const MAX_PARTS: usize = 2_000;
/// Largest manifest read (hostile servers).
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;
/// The folder of the model files in a bundle, and the manifest's name in it.
pub const MODEL_DIR: &str = "model";
pub const MANIFEST: &str = "share.json";
/// Longest `#view=` text read.
pub const MAX_VIEW_TEXT: usize = 2_000;

/// One file of a bundle: its name relative to the manifest's folder, and its size.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareFile {
    pub file: String,
    pub bytes: usize,
}

/// An element part: the file and how many elements it holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharePart {
    pub file: String,
    pub bytes: usize,
    pub elements: usize,
}

/// `model/share.json`: what the page loads, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareManifest {
    pub schema: String,
    /// The title the publisher gave the page.
    pub title: String,
    /// The package without its elements.
    pub base: ShareFile,
    pub parts: Vec<SharePart>,
    /// Elements in all parts.
    pub elements: usize,
    /// Level names, for the loading screen.
    pub levels: Vec<String>,
    /// The model file offered for download (`--allow-download`); none: no download.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<ShareFile>,
    /// Whether the bundle was stripped of names (`--strip`, the default).
    pub stripped: bool,
}

/// Whether `name` is a plain relative file name inside the bundle: no scheme, no absolute path,
/// no `..`, no backslash, no query.
pub fn safe_rel(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('/')
        && !name.contains("//")
        && !name.contains(':')
        && !name.contains('\\')
        && !name.contains('?')
        && !name.contains('#')
        && name.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

impl ShareManifest {
    /// Reads a manifest, refusing one too large, from a newer CADaddy or naming files outside
    /// the bundle.
    pub fn from_json(bytes: &[u8]) -> Result<ShareManifest, String> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("the share manifest is too large".into());
        }
        let mut m: ShareManifest = serde_json::from_slice(bytes).map_err(|e| format!("not a share manifest: {e}"))?;
        if m.schema != SHARE_SCHEMA {
            return Err(if m.schema.starts_with("cadaddy-share/") {
                format!("the page `{}` is from a newer CADaddy (this one reads {SHARE_SCHEMA})", m.schema)
            } else {
                "not a share manifest (no `cadaddy-share` schema)".into()
            });
        }
        m.parts.truncate(MAX_PARTS);
        m.levels.truncate(crate::field::MAX_LEVELS);
        clip(&mut m.title, 200);
        let files = std::iter::once(&m.base.file).chain(m.parts.iter().map(|p| &p.file)).chain(m.download.iter().map(|d| &d.file));
        if let Some(bad) = files.into_iter().find(|f| !safe_rel(f)) {
            return Err(format!("the share manifest names a file outside the page: {bad:?}"));
        }
        Ok(m)
    }

    /// Bytes the page fetches for the model (base and parts).
    pub fn total_bytes(&self) -> usize {
        self.parts.iter().fold(self.base.bytes, |n, p| n.saturating_add(p.bytes))
    }
}

/// The package split for a bundle: the base JSON (no elements) and the element parts' JSON.
pub fn split(pkg: &FieldPackage, chunk: usize) -> Result<(String, Vec<String>), String> {
    let chunk = chunk.clamp(100, 50_000);
    let base = FieldPackage { elements: Vec::new(), notes: Vec::new(), ..pkg.clone() };
    let base = base.to_json()?;
    let parts = pkg.elements.chunks(chunk).take(MAX_PARTS).map(|c| serde_json::to_string(c).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    Ok((base, parts))
}

/// A part file: its elements (capped, numbers cleaned when the package is assembled).
pub fn parse_part(bytes: &[u8]) -> Result<Vec<FieldElement>, String> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err("a model part is too large".into());
    }
    let mut v: Vec<FieldElement> = serde_json::from_slice(bytes).map_err(|e| format!("not a model part: {e}"))?;
    v.truncate(MAX_ELEMENTS);
    Ok(v)
}

/// A file of the model, read: the base package (no elements yet), or a part's elements.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    Base(FieldPackage),
    Part(Vec<FieldElement>),
}

/// A page loading its model: the manifest, then the base, then the parts in order.
#[derive(Clone, Debug, Default)]
pub struct ShareLoad {
    pub manifest: ShareManifest,
    pub base_done: bool,
    /// Parts loaded so far.
    pub parts_done: usize,
    pub bytes_done: usize,
}

impl ShareLoad {
    pub fn new(manifest: ShareManifest) -> ShareLoad {
        ShareLoad { manifest, ..ShareLoad::default() }
    }

    /// The next file to fetch (relative to the manifest), none when everything is in.
    pub fn next_file(&self) -> Option<&str> {
        if !self.base_done {
            return Some(&self.manifest.base.file);
        }
        self.manifest.parts.get(self.parts_done).map(|p| p.file.as_str())
    }

    /// Reads the bytes of [`Self::next_file`].
    pub fn add(&mut self, bytes: &[u8]) -> Result<Loaded, String> {
        let loaded = if self.base_done {
            let els = parse_part(bytes)?;
            self.parts_done += 1;
            Loaded::Part(els)
        } else {
            let mut pkg = FieldPackage::from_json(bytes)?;
            pkg.elements.clear();
            pkg.notes.clear();
            self.base_done = true;
            Loaded::Base(pkg)
        };
        self.bytes_done = self.bytes_done.saturating_add(bytes.len());
        Ok(loaded)
    }

    pub fn done(&self) -> bool {
        self.next_file().is_none()
    }

    /// 0..=1, by bytes (the manifest's sizes).
    pub fn progress(&self) -> f32 {
        let total = self.manifest.total_bytes();
        if self.done() {
            1.0
        } else if total == 0 {
            0.0
        } else {
            (self.bytes_done as f64 / total as f64).clamp(0.0, 0.99) as f32
        }
    }

    /// The whole package, every file read through `read` (a name relative to the manifest).
    pub fn read_all(manifest: ShareManifest, mut read: impl FnMut(&str) -> Result<Vec<u8>, String>) -> Result<FieldPackage, String> {
        let mut load = ShareLoad::new(manifest);
        let mut pkg: Option<FieldPackage> = None;
        while let Some(f) = load.next_file().map(str::to_string) {
            match (load.add(&read(&f)?)?, &mut pkg) {
                (Loaded::Base(b), _) => pkg = Some(b),
                (Loaded::Part(els), Some(p)) => {
                    let room = MAX_ELEMENTS.saturating_sub(p.elements.len());
                    p.elements.extend(els.into_iter().take(room));
                }
                (Loaded::Part(_), None) => return Err("a part came before the base".into()),
            }
        }
        let mut pkg = pkg.ok_or("no base")?;
        pkg.clean();
        Ok(pkg)
    }
}

/// Writes the model files of a bundle into `dir`: `model/base.json`, `model/part-NNN.json`, the
/// download (`model/download/<name>`, when given), `thumbs/<level>.svg` and `thumbs/overview.svg`,
/// and last `model/share.json`. Returns the manifest and the files written (relative to `dir`).
pub fn write_bundle(
    dir: &std::path::Path,
    pkg: &FieldPackage,
    title: &str,
    chunk: usize,
    stripped: bool,
    download: Option<(&str, &[u8])>,
) -> Result<(ShareManifest, Vec<String>), String> {
    let model = dir.join(MODEL_DIR);
    let thumbs = dir.join("thumbs");
    for d in [dir.to_path_buf(), model.clone(), thumbs.clone()] {
        std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let put = |path: std::path::PathBuf, bytes: &[u8]| std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()));
    let (base, parts) = split(pkg, chunk)?;
    let mut files = vec![format!("{MODEL_DIR}/base.json")];
    put(model.join("base.json"), base.as_bytes())?;
    let mut manifest = ShareManifest {
        schema: SHARE_SCHEMA.into(),
        title: title.chars().take(200).collect(),
        base: ShareFile { file: "base.json".into(), bytes: base.len() },
        parts: Vec::new(),
        elements: pkg.elements.len(),
        levels: pkg.levels.iter().map(|l| l.name.clone()).collect(),
        download: None,
        stripped,
    };
    let per = chunk.clamp(100, 50_000);
    for (i, text) in parts.iter().enumerate() {
        let name = format!("part-{i:03}.json");
        put(model.join(&name), text.as_bytes())?;
        let elements = pkg.elements.len().saturating_sub(i * per).min(per);
        manifest.parts.push(SharePart { file: name.clone(), bytes: text.len(), elements });
        files.push(format!("{MODEL_DIR}/{name}"));
    }
    let mut seen = BTreeSet::new();
    for l in &pkg.levels {
        let mut name = slug(&l.name);
        while !seen.insert(name.clone()) {
            name.push('x');
        }
        put(thumbs.join(format!("{name}.svg")), plan_svg(pkg, Some(&l.name), 256).as_bytes())?;
        files.push(format!("thumbs/{name}.svg"));
    }
    put(thumbs.join("overview.svg"), plan_svg(pkg, None, 512).as_bytes())?;
    files.push("thumbs/overview.svg".into());
    if let Some((name, bytes)) = download {
        let file = format!("download/{name}");
        if !safe_rel(&file) {
            return Err(format!("{name:?} isn't a plain file name"));
        }
        std::fs::create_dir_all(model.join("download")).map_err(|e| format!("{}: {e}", model.display()))?;
        put(model.join(&file), bytes)?;
        manifest.download = Some(ShareFile { file: file.clone(), bytes: bytes.len() });
        files.push(format!("{MODEL_DIR}/{file}"));
    }
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    put(model.join(MANIFEST), text.as_bytes())?;
    files.push(format!("{MODEL_DIR}/{MANIFEST}"));
    Ok((manifest, files))
}

// ---------------- stripping names ----------------

/// What [`strip`] removed: where, and the text that went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripReport {
    pub removed: Vec<(String, String)>,
}

impl StripReport {
    fn note(&mut self, place: &str, what: &str) {
        if self.removed.len() < 100_000 {
            self.removed.push((place.to_string(), what.chars().take(200).collect()));
        }
    }

    /// `strip-report.txt`: one line per removal.
    pub fn text(&self, title: &str) -> String {
        let mut s = format!(
            "Strip report for the shared page \"{title}\"\nThis file is NOT part of the page; keep it private.\n{} item(s) removed.\n\n",
            self.removed.len()
        );
        for (place, what) in &self.removed {
            let _ = writeln!(s, "{place}\t{what}");
        }
        s
    }
}

/// Property keys whose values name people, firms, places or the project: dropped whole.
const PRIVATE_KEYS: [&str; 40] = [
    "author",
    "owner",
    "client",
    "customer",
    "tenant",
    "occupant",
    "person",
    "people",
    "contact",
    "architect",
    "engineer",
    "contractor",
    "designer",
    "drawn",
    "checked",
    "approved",
    "reviewed",
    "manufacturer",
    "vendor",
    "supplier",
    "installer",
    "company",
    "firm",
    "organization",
    "organisation",
    "address",
    "street",
    "city",
    "postal",
    "zip",
    "phone",
    "telephone",
    "fax",
    "email",
    "e-mail",
    "website",
    "url",
    "project",
    "job",
    "permit",
];

/// Words that end a company name.
const COMPANY_WORDS: [&str; 24] = [
    "inc",
    "incorporated",
    "llc",
    "l.l.c",
    "ltd",
    "limited",
    "corp",
    "corporation",
    "co",
    "company",
    "gmbh",
    "pllc",
    "llp",
    "plc",
    "architects",
    "architecture",
    "engineers",
    "engineering",
    "associates",
    "partners",
    "consultants",
    "contractors",
    "construction",
    "builders",
];

/// Words that end a street address.
const STREET_WORDS: [&str; 26] = [
    "st", "street", "ave", "avenue", "rd", "road", "blvd", "boulevard", "ln", "lane", "dr", "drive", "way", "ct", "court", "pkwy", "parkway",
    "hwy", "highway", "pl", "place", "ter", "terrace", "cir", "circle", "trail",
];

/// Words that mark a title block or a sheet stamp: a text holding one goes whole.
const STAMP_WORDS: [&str; 10] =
    ["seal", "stamp", "license", "licence", "registered", "issued for", "not for construction", "copyright", "©", "all rights reserved"];

/// Removes names from texts: the known ones (the drawing's properties, title blocks, file and
/// project names, an IFC file's people, organisations and addresses), and what looks like an
/// e-mail address, a phone number, a web address, a street address or a company name.
#[derive(Clone, Debug, Default)]
pub struct Scrubber {
    /// Lower case, longest first.
    deny: Vec<String>,
}

impl Scrubber {
    /// A scrubber for `known` names (shorter than 4 characters, or plain numbers, are ignored:
    /// they would cut ordinary words).
    pub fn new<I: IntoIterator<Item = String>>(known: I) -> Scrubber {
        let mut deny: Vec<String> = known
            .into_iter()
            .take(100_000)
            .map(|s| s.trim().to_lowercase())
            .filter(|s| s.chars().count() >= 4 && s.chars().any(char::is_alphabetic) && s.len() <= MAX_TEXT)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        deny.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Scrubber { deny }
    }

    /// Names it knows.
    pub fn known(&self) -> usize {
        self.deny.len()
    }

    /// `text` with names taken out; what went is noted in `report` under `place`.
    pub fn scrub(&self, text: &str, place: &str, report: &mut StripReport) -> String {
        if text.is_empty() {
            return String::new();
        }
        let lower = text.to_lowercase();
        if STAMP_WORDS.iter().any(|w| has_phrase(&lower, w)) {
            report.note(place, text);
            return String::new();
        }
        let mut out = text.to_string();
        for d in &self.deny {
            out = remove_ci(&out, d, place, report);
        }
        let out = patterns(&out, place, report);
        let tidy = tidy(&out);
        if tidy.is_empty() && !text.trim().is_empty() && report.removed.last().is_none_or(|(p, _)| p != place) {
            report.note(place, text);
        }
        tidy
    }
}

/// `text` without any case-insensitive occurrence of `needle` (lower case).
fn remove_ci(text: &str, needle: &str, place: &str, report: &mut StripReport) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    for _ in 0..1_000 {
        let lower = rest.to_lowercase();
        // Lower-casing can change byte lengths; only ASCII-safe positions are cut.
        let Some(at) = lower.find(needle).filter(|_| lower.len() == rest.len()) else { break };
        let end = at + needle.len();
        let (Some(head), Some(found), Some(tail)) = (rest.get(..at), rest.get(at..end), rest.get(end..)) else { break };
        report.note(place, found);
        out.push_str(head);
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn bare(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '@').trim_end_matches('.').to_lowercase()
}

/// E-mail, web and street addresses, phone numbers and company names out of `text`.
fn patterns(text: &str, place: &str, report: &mut StripReport) -> String {
    let words: Vec<&str> = text.split_whitespace().take(2_000).collect();
    let mut drop = vec![false; words.len()];
    fn flag(from: usize, to: usize, drop: &mut [bool]) {
        for d in drop.iter_mut().take(to + 1).skip(from) {
            *d = true;
        }
    }
    for (i, w) in words.iter().enumerate() {
        let b = bare(w);
        let lw = w.to_lowercase();
        // E-mail and web addresses.
        if (b.contains('@') && b.contains('.')) || lw.contains("://") || lw.starts_with("www.") {
            flag(i, i, &mut drop);
        }
        // Phone numbers: ten or more digits in a run of digit groups.
        if w.chars().any(|c| c.is_ascii_digit()) {
            let mut digits = 0;
            let mut j = i;
            while let Some(x) = words.get(j) {
                if !x.chars().all(|c| c.is_ascii_digit() || "()-.+/".contains(c)) {
                    break;
                }
                digits += x.chars().filter(char::is_ascii_digit).count();
                j += 1;
            }
            if digits >= 10 && j > i {
                flag(i, j - 1, &mut drop);
            }
        }
        // A street address: a house number, a street name and a street word; the rest of the
        // text (city, postcode) goes with it.
        if !b.is_empty() && b.len() <= 6 && b.chars().all(|c| c.is_ascii_digit()) {
            let street = words.iter().enumerate().skip(i + 1).take(5).find(|(_, x)| STREET_WORDS.contains(&bare(x).as_str()));
            if let Some((j, _)) = street
                && words.get(i + 1..j).is_some_and(|between| !between.is_empty() && between.iter().all(|x| x.chars().next().is_some_and(char::is_uppercase)))
            {
                flag(i, words.len().saturating_sub(1), &mut drop);
            }
        }
        // A company: capitalised words ending in a company word.
        if i > 0 && COMPANY_WORDS.contains(&b.as_str()) && words.get(i - 1).is_some_and(|x| x.chars().next().is_some_and(char::is_uppercase)) {
            let mut from = i;
            while from > 0 && words.get(from - 1).is_some_and(|x| x.chars().next().is_some_and(|c| c.is_uppercase() || c == '&')) {
                from -= 1;
            }
            flag(from, i, &mut drop);
        }
    }
    if !drop.iter().any(|d| *d) {
        return text.to_string();
    }
    let gone: Vec<&str> = words.iter().zip(&drop).filter(|(_, d)| **d).map(|(w, _)| *w).collect();
    report.note(place, &gone.join(" "));
    words.iter().zip(&drop).filter(|(_, d)| !**d).map(|(w, _)| *w).collect::<Vec<_>>().join(" ")
}

/// Spaces and dangling separators tidied after removals.
fn tidy(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_matches(|c: char| c.is_whitespace() || ",;:-–—|/()".contains(c)).to_string()
}

fn clip(s: &mut String, n: usize) {
    if s.chars().count() > n {
        *s = s.chars().take(n).collect();
    }
}

/// Whether `lower` holds `phrase` as whole words (`seal`, not `sealant`).
fn has_phrase(lower: &str, phrase: &str) -> bool {
    let mut from = 0;
    while let Some(at) = lower.get(from..).and_then(|r| r.find(phrase)).map(|a| a + from) {
        let end = at + phrase.len();
        let before = lower.get(..at).and_then(|h| h.chars().next_back()).is_none_or(|c| !c.is_alphanumeric());
        let after = lower.get(end..).and_then(|t| t.chars().next()).is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = end;
    }
    false
}

/// The words of a property key: `Pset_X.ManufacturerName` → `pset`, `x`, `manufacturer`, `name`.
fn key_words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut w = String::new();
    let mut prev_lower = false;
    for c in key.chars().take(200) {
        if !c.is_alphanumeric() {
            prev_lower = false;
            words.push(std::mem::take(&mut w));
            continue;
        }
        if c.is_uppercase() && prev_lower {
            words.push(std::mem::take(&mut w));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        w.extend(c.to_lowercase());
    }
    words.push(w);
    words.retain(|w| !w.is_empty());
    words
}

/// Whether a property key names people, firms, places or the project (its value goes).
pub fn private_key(key: &str) -> bool {
    // The name part of `Set.Name` keys: `Pset_ManufacturerTypeInformation.ModelLabel` keeps.
    let name = key.rsplit(['.', ':']).next().unwrap_or(key);
    key_words(name).iter().any(|w| PRIVATE_KEYS.contains(&w.as_str()) || w == "mail")
}

/// File names in a source text become `file` (`A-201 Lobby.pdf` → `A-201 file`... only the name).
fn no_file_names(s: &str) -> String {
    const EXT: [&str; 10] = [".pdf", ".dwg", ".dxf", ".ifc", ".cdy", ".rvt", ".png", ".jpg", ".tif", ".xlsx"];
    let lower = s.to_lowercase();
    match EXT.iter().filter_map(|e| lower.find(e).map(|at| at + e.len())).min() {
        // The whole text up to the extension is the file name (names have spaces).
        Some(end) => {
            let tail = s.get(end..).unwrap_or_default().trim();
            if tail.is_empty() { "file".to_string() } else { format!("file {tail}") }
        }
        None => s.to_string(),
    }
}

/// Strips `pkg` for sharing: the project's name becomes `title`, field notes (people wrote them)
/// go, private properties go, and every remaining text is scrubbed.
pub fn strip(pkg: &mut FieldPackage, title: &str, scrubber: &Scrubber) -> StripReport {
    let mut r = StripReport::default();
    if !pkg.project.is_empty() && pkg.project != title {
        r.note("project", &pkg.project);
    }
    pkg.project = title.to_string();
    for n in pkg.notes.drain(..) {
        r.note(&format!("field note {}", n.id), &format!("{} {}", n.author, n.text).trim().to_string());
    }
    let sc = |s: &mut String, place: &str, r: &mut StripReport| *s = scrubber.scrub(s, place, r);
    for l in &mut pkg.levels {
        let place = format!("level {}", l.name);
        sc(&mut l.name, &place, &mut r);
    }
    for room in &mut pkg.rooms {
        let place = format!("room {}", room.number);
        sc(&mut room.name, &place, &mut r);
        sc(&mut room.number, &place, &mut r);
        sc(&mut room.unit, &place, &mut r);
        sc(&mut room.level, &place, &mut r);
    }
    for t in &mut pkg.types {
        let place = format!("type {}", t.mark);
        strip_fields(&mut t.fields, &place, scrubber, &mut r);
        sc(&mut t.category, &place, &mut r);
    }
    for e in &mut pkg.elements {
        let place = format!("element {}", e.id);
        e.source = no_file_names(&e.source);
        sc(&mut e.source, &place, &mut r);
        sc(&mut e.status, &place, &mut r);
        sc(&mut e.room, &place, &mut r);
        sc(&mut e.level, &place, &mut r);
        sc(&mut e.mark, &place, &mut r);
        strip_fields(&mut e.props, &place, scrubber, &mut r);
    }
    for i in &mut pkg.issues {
        let place = format!("issue {}", i.id);
        sc(&mut i.title, &place, &mut r);
        sc(&mut i.room, &place, &mut r);
        sc(&mut i.level, &place, &mut r);
    }
    r
}

fn strip_fields(fields: &mut Vec<FieldSpec>, place: &str, scrubber: &Scrubber, r: &mut StripReport) {
    fields.retain(|f| {
        let keep = !private_key(&f.key);
        if !keep {
            r.note(&format!("{place} {}", f.key), &f.value);
        }
        keep
    });
    for f in fields.iter_mut() {
        let p = format!("{place} {}", f.key);
        f.value = scrubber.scrub(&f.value, &p, r);
        f.source = scrubber.scrub(&no_file_names(&f.source), &p, r);
    }
    fields.retain(|f| !f.value.is_empty());
}

// ---------------- views as links ----------------

/// A view of the model as a `#view=` link: `l=2;m=3d;o=235,35,60;t=10,20,4;h=mech,elec;...`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewLink {
    /// The level, by index.
    pub level: usize,
    /// `plan`, `3d` or `walk`.
    pub mode: String,
    /// Orbit: yaw, pitch (degrees), distance (feet).
    pub orbit: Option<[f64; 3]>,
    /// The point orbited (feet), when moved off the room or level centre.
    pub target: Option<[f64; 3]>,
    /// Walk: where (feet) and heading (degrees).
    pub walk: Option<[f64; 3]>,
    /// Plan: centre (feet) and pixels per foot.
    pub plan: Option<[f64; 3]>,
    /// Trades hidden.
    pub hidden: Vec<String>,
    /// Section box: min and max corners, feet.
    pub section: Option<[f64; 6]>,
    /// The room picked, by number.
    pub room: String,
    /// The element picked, by id.
    pub element: String,
}

fn nums<const N: usize>(s: &str) -> Option<[f64; N]> {
    let mut out = [0.0; N];
    let mut it = s.split(',');
    for o in out.iter_mut() {
        let v: f64 = it.next()?.trim().parse().ok()?;
        if !v.is_finite() || v.abs() > 1e7 {
            return None;
        }
        *o = v;
    }
    it.next().is_none().then_some(out)
}

fn fmt_nums(v: &[f64]) -> String {
    v.iter().map(|x| format!("{}", (x * 100.0).round() / 100.0)).collect::<Vec<_>>().join(",")
}

/// Characters a link keeps as they are; others are `%XX`.
fn link_safe(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"-_.~,".contains(&c)
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if link_safe(b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = (c == b'%').then(|| b.get(i + 1..i + 3)).flatten().and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl ViewLink {
    /// The `#view=` fragment text (without `#view=`).
    pub fn encode(&self) -> String {
        let mut parts = vec![format!("l={}", self.level), format!("m={}", enc(&self.mode))];
        if let Some(o) = self.orbit {
            parts.push(format!("o={}", fmt_nums(&o)));
        }
        if let Some(t) = self.target {
            parts.push(format!("t={}", fmt_nums(&t)));
        }
        if let Some(w) = self.walk {
            parts.push(format!("w={}", fmt_nums(&w)));
        }
        if let Some(p) = self.plan {
            parts.push(format!("p={}", fmt_nums(&p)));
        }
        if !self.hidden.is_empty() {
            parts.push(format!("h={}", self.hidden.iter().map(|h| enc(h)).collect::<Vec<_>>().join(",")));
        }
        if let Some(s) = self.section {
            parts.push(format!("s={}", fmt_nums(&s)));
        }
        if !self.room.is_empty() {
            parts.push(format!("r={}", enc(&self.room)));
        }
        if !self.element.is_empty() {
            parts.push(format!("e={}", enc(&self.element)));
        }
        parts.join(";")
    }

    /// Reads a `#view=` text (or a whole fragment `#view=…`, or a URL ending in one); none when
    /// it isn't a view. Unknown or broken parts are skipped.
    pub fn decode(text: &str) -> Option<ViewLink> {
        let text = text.get(..text.len().min(MAX_VIEW_TEXT))?;
        let text = match text.find("view=") {
            Some(at) => text.get(at + 5..)?,
            None if text.contains("l=") => text,
            None => return None,
        };
        let text = text.split('&').next().unwrap_or_default();
        let mut v = ViewLink { mode: "plan".into(), ..ViewLink::default() };
        for part in text.split(';').take(32) {
            let Some((k, val)) = part.split_once('=') else { continue };
            match k {
                "l" => v.level = val.parse::<usize>().ok().filter(|l| *l < crate::field::MAX_LEVELS).unwrap_or(0),
                "m" => {
                    let m = dec(val);
                    if ["plan", "3d", "walk"].contains(&m.as_str()) {
                        v.mode = m;
                    }
                }
                "o" => v.orbit = nums::<3>(val),
                "t" => v.target = nums::<3>(val),
                "w" => v.walk = nums::<3>(val),
                "p" => v.plan = nums::<3>(val),
                "h" => v.hidden = val.split(',').take(64).map(dec).filter(|h| !h.is_empty() && h.len() <= 40).collect(),
                "s" => v.section = nums::<6>(val),
                "r" => v.room = dec(val).chars().take(80).collect(),
                "e" => v.element = dec(val).chars().take(120).collect(),
                _ => {}
            }
        }
        Some(v)
    }
}

// ---------------- thumbnails ----------------

/// A level's plan as a small SVG picture: room outlines and the elements as boxes or lines in
/// their trade's colour, no text (no names leave in a picture either). `level` none: the first.
pub fn plan_svg(pkg: &FieldPackage, level: Option<&str>, size: u32) -> String {
    let level = level.map(str::to_string).or_else(|| pkg.levels.first().map(|l| l.name.clone())).unwrap_or_default();
    let on = |l: &str| l.eq_ignore_ascii_case(&level);
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut grow = |p: [f64; 2]| {
        if p.iter().all(|v| v.is_finite()) {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    };
    for r in pkg.rooms.iter().filter(|r| on(&r.level)) {
        r.outline.iter().for_each(|q| grow(*q));
    }
    for e in pkg.elements.iter().filter(|e| on(&e.level)).take(200_000) {
        grow([e.min[0], e.min[1]]);
        grow([e.max[0], e.max[1]]);
    }
    let size = size.clamp(32, 2_048);
    let s = f64::from(size);
    let mut out = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" viewBox=\"0 0 {size} {size}\">");
    let _ = write!(out, "<rect width=\"{size}\" height=\"{size}\" fill=\"#22282f\"/>");
    if !(lo[0].is_finite() && hi[0].is_finite()) {
        return out + "</svg>";
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);
    let k = s * 0.92 / span;
    let (ox, oy) = (s * 0.04 + (span - (hi[0] - lo[0])) * k * 0.5, s * 0.04 + (span - (hi[1] - lo[1])) * k * 0.5);
    let x = |v: f64| ox + (v - lo[0]) * k;
    let y = |v: f64| s - (oy + (v - lo[1]) * k);
    for r in pkg.rooms.iter().filter(|r| on(&r.level) && r.outline.len() >= 3).take(20_000) {
        let pts: Vec<String> = r.outline.iter().map(|q| format!("{:.1},{:.1}", x(q[0]), y(q[1]))).collect();
        let _ = write!(out, "<polygon points=\"{}\" fill=\"none\" stroke=\"#56606b\" stroke-width=\"0.6\"/>", pts.join(" "));
    }
    for e in pkg.elements.iter().filter(|e| on(&e.level)).take(50_000) {
        let c = pkg.trade(&e.trade).color;
        let col = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        match (e.path.first(), e.path.last()) {
            (Some(a), Some(b)) if e.path.len() >= 2 => {
                let _ = write!(
                    out,
                    "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{col}\" stroke-width=\"0.8\"/>",
                    x(a[0]),
                    y(a[1]),
                    x(b[0]),
                    y(b[1])
                );
            }
            _ => {
                let (w, h) = (((e.max[0] - e.min[0]) * k).max(0.8), ((e.max[1] - e.min[1]) * k).max(0.8));
                let _ = write!(out, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" fill=\"{col}\"/>", x(e.min[0]), y(e.max[1]));
            }
        }
    }
    out + "</svg>"
}

/// A file-name-safe slug of a level name (`Level 2` → `level-2`).
pub fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(40).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "level".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{FieldIssue, FieldLevel, FieldNote, FieldRoom, FieldType, trade_info};

    fn pkg() -> FieldPackage {
        let mut p = FieldPackage::new("Lakeside Example Inn");
        p.levels = vec![FieldLevel { name: "Level 1".into(), elevation_ft: 0.0, height_ft: 12.0 }];
        p.trades = vec![trade_info("arch"), trade_info("mech")];
        p.rooms = vec![FieldRoom {
            number: "101".into(),
            name: "Office of Jane Q. Example".into(),
            level: "Level 1".into(),
            outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            at: [5.0, 5.0],
            unit: String::new(),
        }];
        p.types = vec![FieldType {
            mark: "SD-1".into(),
            category: "duct".into(),
            fields: vec![
                FieldSpec { key: "material".into(), value: "galvanized steel".into(), source: "Lakeside Example Inn Mech.pdf".into(), confidence: "stated".into() },
                FieldSpec { key: "manufacturer".into(), value: "Northwind Sheet Metal".into(), ..FieldSpec::default() },
                FieldSpec { key: "note".into(), value: "by Acme Fictional Fabricators LLC, call 555-010-0199".into(), ..FieldSpec::default() },
            ],
        }];
        p.elements = (0..7)
            .map(|i| FieldElement {
                id: format!("E{i}"),
                trade: if i % 2 == 0 { "arch".into() } else { "mech".into() },
                kind: "wall".into(),
                level: "Level 1".into(),
                min: [f64::from(i), 0.0, 0.0],
                max: [f64::from(i) + 1.0, 1.0, 10.0],
                source: "A-101 1234 Imaginary Lane, Elmbrook".into(),
                ..FieldElement::default()
            })
            .collect();
        p.issues = vec![FieldIssue { id: "RFI-1".into(), title: "ask jane.example@example.invalid".into(), ..FieldIssue::default() }];
        p.notes = vec![FieldNote { id: "n1".into(), author: "Jane Q. Example".into(), text: "seen".into(), ..FieldNote::default() }];
        p
    }

    #[test]
    fn a_split_package_loads_back_part_by_part() {
        let p = pkg();
        let (base, parts) = split(&p, 100).unwrap();
        assert_eq!(parts.len(), 1);
        let (_, parts3) = split(&FieldPackage { elements: (0..50).flat_map(|_| p.elements.clone()).collect(), ..p.clone() }, 100).unwrap();
        assert_eq!(parts3.len(), 4);
        let manifest = ShareManifest {
            schema: SHARE_SCHEMA.into(),
            title: "T".into(),
            base: ShareFile { file: "base.json".into(), bytes: base.len() },
            parts: parts3.iter().enumerate().map(|(i, t)| SharePart { file: format!("part-{i:03}.json"), bytes: t.len(), elements: 0 }).collect(),
            ..ShareManifest::default()
        };
        let m = ShareManifest::from_json(&serde_json::to_vec(&manifest).unwrap()).unwrap();
        let mut load = ShareLoad::new(m.clone());
        assert_eq!(load.next_file(), Some("base.json"));
        assert!(matches!(load.add(base.as_bytes()).unwrap(), Loaded::Base(b) if b.elements.is_empty() && b.notes.is_empty()));
        assert!(load.progress() > 0.0 && load.progress() < 1.0);
        let mut n = 0;
        for t in &parts3 {
            assert!(load.next_file().is_some());
            let Loaded::Part(els) = load.add(t.as_bytes()).unwrap() else { panic!("a part") };
            n += els.len();
        }
        assert!(load.done());
        assert_eq!((load.progress(), n), (1.0, 350));
        let files: Vec<String> = std::iter::once(base.clone()).chain(parts3.iter().cloned()).collect();
        let mut it = files.into_iter();
        let pkg = ShareLoad::read_all(m, |_| it.next().map(String::into_bytes).ok_or_else(|| "none".to_string())).unwrap();
        assert_eq!(pkg.elements.len(), 350);
        assert!(pkg.notes.is_empty(), "notes never go in a share bundle");
    }

    #[test]
    fn manifests_naming_files_outside_the_page_are_refused() {
        for bad in ["/etc/passwd", "../x.json", "https://example.invalid/x.json", "a\\b.json", "a/../b", "x.json?y", ""] {
            assert!(!safe_rel(bad), "{bad}");
            let m = ShareManifest { schema: SHARE_SCHEMA.into(), base: ShareFile { file: bad.into(), bytes: 1 }, ..ShareManifest::default() };
            assert!(ShareManifest::from_json(&serde_json::to_vec(&m).unwrap()).is_err(), "{bad}");
        }
        assert!(safe_rel("part-001.json") && safe_rel("download/model.field.json"));
        assert!(ShareManifest::from_json(br#"{"schema":"cadaddy-share/9"}"#).unwrap_err().contains("newer"));
        assert!(ShareManifest::from_json(b"junk").is_err());
        assert!(parse_part(b"{\"no\":1}").is_err());
    }

    #[test]
    fn names_addresses_firms_and_notes_are_stripped() {
        let mut p = pkg();
        let sc = Scrubber::new(["Jane Q. Example".to_string(), "Lakeside Example Inn".to_string(), "ab".to_string(), "1234".to_string()]);
        assert_eq!(sc.known(), 2);
        let r = strip(&mut p, "Shared Model", &sc);
        let text = p.to_json().unwrap().to_lowercase();
        for planted in ["jane", "lakeside", "imaginary lane", "elmbrook", "acme", "northwind", "555-010-0199", "example.invalid", ".pdf"] {
            assert!(!text.contains(planted), "{planted} left in {text}");
        }
        assert_eq!(p.project, "Shared Model");
        assert!(p.notes.is_empty());
        let ty = p.types.first().unwrap();
        assert!(ty.fields.iter().any(|f| f.key == "material" && f.value == "galvanized steel"), "{:?}", ty.fields);
        assert!(!ty.fields.iter().any(|f| f.key == "manufacturer"));
        assert_eq!(p.rooms.first().unwrap().name, "Office of");
        assert_eq!(p.elements.first().unwrap().source, "A-101");
        assert!(r.removed.len() >= 8, "{:?}", r.removed);
        let report = r.text("Shared Model");
        assert!(report.contains("NOT part of the page") && report.contains("Jane Q. Example"));
    }

    #[test]
    fn stamps_go_whole_and_ordinary_texts_stay() {
        let sc = Scrubber::default();
        let mut r = StripReport::default();
        assert_eq!(sc.scrub("ISSUED FOR CONSTRUCTION 2026-01-01", "x", &mut r), "");
        assert_eq!(sc.scrub("Professional seal of A. Person", "x", &mut r), "");
        for keep in ["24x12 supply duct", "Guest Room 214", "M-201 Supply Air", "3/4\" C BOT @ 8'-0\"", "2 hr rated", "Level 2"] {
            assert_eq!(sc.scrub(keep, "x", &mut r), keep);
        }
        assert_eq!(sc.scrub("Duct by Example Ductworks Co.", "x", &mut r), "Duct by");
        assert_eq!(sc.scrub("call (555) 010-0199 now", "x", &mut r), "call now");
        assert_eq!(sc.scrub("fire sealant at sleeves", "x", &mut r), "fire sealant at sleeves");
        assert!(private_key("Pset_ManufacturerTypeInformation.Manufacturer") && private_key("ContactEmail") && private_key("drawn by"));
        assert!(!private_key("Pset_ManufacturerTypeInformation.ModelLabel") && !private_key("Capacity") && !private_key("material"));
    }

    #[test]
    fn view_links_round_trip_and_junk_is_ignored() {
        let v = ViewLink {
            level: 2,
            mode: "3d".into(),
            orbit: Some([235.0, 35.5, 60.0]),
            target: Some([10.0, -4.25, 16.0]),
            hidden: vec!["mech".into(), "elec".into()],
            section: Some([0.0, 0.0, 12.0, 40.0, 30.0, 24.0]),
            room: "2 14".into(),
            element: "wall:L2;3".into(),
            ..ViewLink::default()
        };
        let text = v.encode();
        assert!(!text.contains(' ') && !text.contains('#') && !text.contains('&'), "{text}");
        assert_eq!(ViewLink::decode(&text), Some(v.clone()));
        assert_eq!(ViewLink::decode(&format!("#view={text}")), Some(v.clone()));
        assert_eq!(ViewLink::decode(&format!("https://example.invalid/project/a/#view={text}")), Some(v));
        assert_eq!(ViewLink::decode("#nothing"), None);
        let junk = ViewLink::decode("view=l=99999999;m=evil;o=NaN,1,2;s=1,2;h=").unwrap();
        assert_eq!((junk.level, junk.mode.as_str(), junk.orbit, junk.section), (0, "plan", None, None));
        assert!(ViewLink::decode(&"view=l=1;".repeat(10_000)).is_some());
    }

    #[test]
    fn thumbnails_have_no_text() {
        let p = pkg();
        let svg = plan_svg(&p, None, 256);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>") && svg.contains("<rect x="));
        assert!(!svg.contains("<text") && !svg.to_lowercase().contains("jane"));
        assert!(plan_svg(&FieldPackage::default(), None, 0).ends_with("</svg>"));
        assert_eq!(slug("Level 2 / Mezz"), "level-2-mezz");
        assert_eq!(slug("..."), "level");
    }
}
