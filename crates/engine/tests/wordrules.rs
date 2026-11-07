//! Q6210: building knowledge is rule data (`crates/engine/src/cmd/wordrules.json`), not word lists
//! in the reading code.
//! - every list the code asks for is in the data with the shape it asks for, and every list in the
//!   data is one the code asks for;
//! - a new word list of 4+ entries in the reading code paths fails here unless it is on the allow
//!   list below with its reason (format vocabulary, our own output, grammar...);
//! - a job adds words through a `words:<list>` project rule; an unknown list or a bad value is refused.
//!
//! The job-addition test changes the process's word lists, so this file holds the only test that
//! sets a `words:` rule (each tests/*.rs file is its own process).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_mepfix::wordrules::{self as wr, Shape};
use serde_json::json;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Non-test source: the text before a file's `#[cfg(test)] mod … {`; none for a test file.
fn source(p: &Path) -> String {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name == "tests.rs" || name.ends_with("_tests.rs") || p.components().any(|c| c.as_os_str() == "tests") {
        return String::new();
    }
    let s = std::fs::read_to_string(p).unwrap_or_default();
    match s.find("#[cfg(test)]\nmod ").or_else(|| s.find("#[cfg(test)]\npub mod ")) {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Str(String),
    Ident(String),
    Punct(char),
}

/// Tokens of Rust source: string literals (raw ones too), identifiers, punctuation; comments,
/// char literals, lifetimes and numbers dropped.
fn tokens(s: &str) -> Vec<Tok> {
    let b: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == 'r' && (next == Some('"') || next == Some('#')) && !(i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&'"') {
                out.push(Tok::Ident("r".into()));
                i += 1;
                continue;
            }
            j += 1;
            let start = j;
            loop {
                if j >= b.len() {
                    break;
                }
                if b[j] == '"' && (0..hashes).all(|h| b.get(j + 1 + h) == Some(&'#')) {
                    break;
                }
                j += 1;
            }
            out.push(Tok::Str(b[start..j.min(b.len())].iter().collect()));
            i = j + 1 + hashes;
        } else if c == '"' {
            let mut j = i + 1;
            let mut v = String::new();
            while j < b.len() && b[j] != '"' {
                if b[j] == '\\' {
                    j += 1;
                }
                if let Some(ch) = b.get(j) {
                    v.push(*ch);
                }
                j += 1;
            }
            out.push(Tok::Str(v));
            i = j + 1;
        } else if c == '\'' {
            // A char literal ('x', '\n', '\'') or a lifetime ('a).
            if next == Some('\\') {
                let mut j = i + 2;
                while j < b.len() && b[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if b.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                j += 1;
            }
            out.push(Tok::Ident(b[i..j].iter().collect()));
            i = j;
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '.') {
                i += 1;
            }
        } else {
            if !c.is_whitespace() {
                out.push(Tok::Punct(c));
            }
            i += 1;
        }
    }
    out
}

/// The word lists of 4+ strings in a file's tokens, each by its signature: `const NAME` for a
/// constant (or static), the first two words for an inline list or a `"a" | "b" | …` alternation.
fn word_lists(t: &[Tok]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        // const / static NAME: <type with str> = <initialiser>;
        if let (Some(Tok::Ident(k)), Some(Tok::Ident(name)), Some(Tok::Punct(':'))) = (t.get(i), t.get(i + 1), t.get(i + 2))
            && (k == "const" || k == "static")
        {
            let mut j = i + 3;
            let mut depth = 0i32;
            let mut has_str = false;
            let mut lits = 0;
            while let Some(tok) = t.get(j) {
                match tok {
                    Tok::Punct('(' | '[' | '{') => depth += 1,
                    Tok::Punct(')' | ']' | '}') => depth -= 1,
                    Tok::Punct(';') if depth <= 0 => break,
                    Tok::Ident(x) if x == "str" => has_str = true,
                    Tok::Str(_) => lits += 1,
                    _ => {}
                }
                j += 1;
            }
            if has_str && lits >= 4 {
                out.push((format!("const {name}"), lits));
            }
            i = j + 1;
            continue;
        }
        // [ "a", "b", … ]
        if t.get(i) == Some(&Tok::Punct('[')) {
            let mut j = i + 1;
            let mut words = Vec::new();
            while let Some(Tok::Str(w)) = t.get(j) {
                words.push(w.clone());
                j += 1;
                if t.get(j) == Some(&Tok::Punct(',')) {
                    j += 1;
                }
            }
            if t.get(j) == Some(&Tok::Punct(']')) && words.len() >= 4 {
                out.push((format!("[{} {}]", words[0], words[1]), words.len()));
                i = j + 1;
                continue;
            }
        }
        // "a" | "b" | "c" | "d"
        if let Some(Tok::Str(w0)) = t.get(i) {
            let mut words = vec![w0.clone()];
            let mut j = i + 1;
            while t.get(j) == Some(&Tok::Punct('|')) {
                match t.get(j + 1) {
                    Some(Tok::Str(w)) => {
                        words.push(w.clone());
                        j += 2;
                    }
                    _ => break,
                }
            }
            if words.len() >= 4 {
                out.push((format!("{} | {}", words[0], words[1]), words.len()));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The reading code paths: how a job's documents are read, its model placed and named.
const READING: &[&str] = &[
    "crates/engine/src/cmd/guided.rs",
    "crates/engine/src/cmd/readers.rs",
    "crates/engine/src/cmd/perceive/geometry.rs",
    "crates/engine/src/cmd/perceive/mod.rs",
    "crates/engine/src/cmd/unittypes.rs",
    "crates/engine/src/cmd/unitmarks.rs",
    "crates/engine/src/cmd/rooms3d.rs",
    "crates/engine/src/cmd/levels.rs",
    "crates/engine/src/cmd/printset.rs",
    "crates/engine/src/cmd/autospec.rs",
    "crates/engine/src/cmd/spec_import.rs",
    "crates/engine/src/cmd/submittals.rs",
    "crates/engine/src/cmd/products.rs",
    "crates/engine/src/cmd/mountlist.rs",
    "crates/engine/src/cmd/keynotes.rs",
    "crates/engine/src/cmd/lightsched.rs",
    "crates/engine/src/cmd/panelsched.rs",
    "crates/engine/src/cmd/printmep.rs",
    "crates/engine/src/cmd/printcond.rs",
    "crates/engine/src/cmd/archread.rs",
    "crates/engine/src/cmd/wallgen.rs",
    "crates/engine/src/cmd/stairs.rs",
    "crates/engine/src/cmd/exterior.rs",
    "crates/engine/src/cmd/concrete.rs",
    "crates/engine/src/cmd/sitetakeoff.rs",
    "crates/engine/src/cmd/import_check.rs",
    "crates/engine/src/cmd/propfill.rs",
    "crates/engine/src/cmd/mep.rs",
    "crates/engine/src/cmd/ugplan.rs",
    "crates/engine/src/cmd/lighting.rs",
    "crates/engine/src/cmd/symbol.rs",
    "crates/engine/src/cmd/sheetfacts.rs",
    "crates/engine/src/cmd/schedules.rs",
    "crates/engine/src/cmd/walltypes.rs",
    "crates/engine/src/cmd/dimreader.rs",
    "crates/engine/src/cmd/bspec.rs",
    "crates/mepfix/src/sheet_facts.rs",
    "crates/mepfix/src/sheet_reader.rs",
    "crates/mepfix/src/printlinks.rs",
    "crates/mepfix/src/printset.rs",
    "crates/mepfix/src/device_types.rs",
    "crates/mepfix/src/fixture_schedule.rs",
    "crates/mepfix/src/arch_reader.rs",
    "crates/mepfix/src/unit_reader.rs",
    "crates/mepfix/src/casework_reader.rs",
    "crates/mepfix/src/equip_reader.rs",
    "crates/mepfix/src/duct_reader.rs",
    "crates/mepfix/src/elec_reader.rs",
    "crates/mepfix/src/framing.rs",
    "crates/mepfix/src/asbuilt.rs",
    "crates/mepfix/src/tagid.rs",
    "crates/mepfix/src/finishes.rs",
    "crates/mepfix/src/wall_reader.rs",
];

/// Word lists that stay in the reading code, each with its reason: (file, signature, reason).
/// A list's signature is `const NAME`, or its first two words.
const ALLOWED: &[(&str, &str, &str)] = &[
    ("crates/mepfix/src/finishes.rs", "[flooring painting]", "our trade sections of the finish takeoff (flooring, painting...): our own names"),
    (
        "crates/mepfix/src/wall_reader.rs",
        "const NOT_LEADERS",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/guided.rs",
        "[ WALL ELEVATION]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/engine/src/cmd/guided.rs",
        "switch | 3way_switch",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/guided.rs",
        "[type fixture_description]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/guided.rs",
        "const UNDERGROUND_SHEETS",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
    (
        "crates/engine/src/cmd/guided.rs",
        "[WALL ELEVATION]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    ("crates/engine/src/cmd/guided.rs", "[ST ND]", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/guided.rs",
        "[ELEVATION SECTION]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/engine/src/cmd/perceive/geometry.rs",
        "NUMBER | NUM",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/perceive/geometry.rs",
        "[SHEET DWG]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/unittypes.rs",
        "const CATEGORIES",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/levels.rs",
        "EL | ELEV",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/levels.rs",
        "[.dwg .dxf]",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
    (
        "crates/engine/src/cmd/printset.rs",
        "detail_bubble | detail_ref",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "[IDEN DIMS]",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "POWR | COMM",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "[DIM DIMS]",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "[NAME NAMES]",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "[LIGHT LIGHTS]",
        "part of an ordered if-chain whose arms combine words with other tests (layer codes by position, a shape and a tag): the words move with the chain, not alone",
    ),
    (
        "crates/engine/src/cmd/autospec.rs",
        "[OUTLETS RECEPTACLES]",
        "part of an ordered if-chain whose arms combine words with other tests (layer codes by position, a shape and a tag): the words move with the chain, not alone",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "const PREFIXES",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "[model: catalog no:]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "const SCHEDULE_COLUMNS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "const MARK_LABELS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "const CUT_SHEET_FIELDS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "const CONTROL_WORDS",
        "rows with numbers or extra fields (sizes, defaults, flags, exclusion words): a table, not a word list; a later item can give it a table shape",
    ),
    (
        "crates/engine/src/cmd/spec_import.rs",
        "[manufacturer: mfr:]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/submittals.rs",
        "const PRODUCT_KINDS",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/products.rs",
        "receptacle | quad_receptacle",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/products.rs",
        "OS | OS1",
        "part of an ordered if-chain whose arms combine words with other tests (layer codes by position, a shape and a tag): the words move with the chain, not alone",
    ),
    (
        "crates/engine/src/cmd/products.rs",
        "device | light_fixture",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    ("crates/engine/src/cmd/mountlist.rs", "• | ·", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/mountlist.rs",
        "device | low_voltage_outlet",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    ("crates/engine/src/cmd/keynotes.rs", "const VIA_WORDS", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/keynotes.rs",
        "HOUR | HOURS",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    ("crates/engine/src/cmd/keynotes.rs", "INTO | TO", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/engine/src/cmd/keynotes.rs", "THE | A", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/keynotes.rs",
        "[AMP AMPS]",
        "drafting notation: the ampere unit's spellings (A, AMP, AMPS, AMPERE), the same on every set",
    ),
    (
        "crates/engine/src/cmd/panelsched.rs",
        "[SCHEDULE SCHEDULES]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/engine/src/cmd/panelsched.rs",
        "[DEMAND CALCULATION]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/engine/src/cmd/panelsched.rs",
        "const PHASE_HEADS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/panelsched.rs",
        "device | light_fixture",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/panelsched.rs",
        "[CKT DESCRIPTION]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/wallgen.rs",
        "const PLAIN",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
    (
        "crates/engine/src/cmd/wallgen.rs",
        "[layer. view.]",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
    (
        "crates/engine/src/cmd/exterior.rs",
        "EL | EL.",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    ("crates/engine/src/cmd/concrete.rs", "const BARS", "standard size tables (rebar, steel shapes): catalogue data, not reading words"),
    (
        "crates/engine/src/cmd/concrete.rs",
        "[Scope Level]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/concrete.rs",
        "pad decision | pad owner",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/concrete.rs",
        "footing | grade_beam",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    ("crates/engine/src/cmd/concrete.rs", "n | s", "compass sides (n, s, e, w): a direction value, not building knowledge"),
    (
        "crates/engine/src/cmd/concrete.rs",
        "pad | pier",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/concrete.rs",
        "[ufer conduit]",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    ("crates/engine/src/cmd/sitetakeoff.rs", "EX | EG", "civil notation: spot-grade surface codes (EX, EG, FG, TC)"),
    ("crates/engine/src/cmd/sitetakeoff.rs", "FG | FS", "civil notation: spot-grade surface codes (EX, EG, FG, TC)"),
    (
        "crates/engine/src/cmd/sitetakeoff.rs",
        "[STMH SDMH]",
        "part of an ordered if-chain whose arms combine words with other tests (layer codes by position, a shape and a tag): the words move with the chain, not alone",
    ),
    (
        "crates/engine/src/cmd/import_check.rs",
        "const DO_NOT_SCALE_PATTERNS",
        "standard title-block boilerplate (DO NOT SCALE...), the same on every set",
    ),
    (
        "crates/engine/src/cmd/propfill.rs",
        "const ENVELOPE_PARTS",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/propfill.rs",
        "device | switch",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/propfill.rs",
        "duct | pipe",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/propfill.rs",
        "[design_value corrected_value]",
        "our own output: the property keys of a corrected value (design value, corrected value, its reason and rule), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/propfill.rs",
        "[keynote receptacle_for]",
        "our own output: the property keys a keynoted equipment receptacle carries (keynote, rating, configuration, poles, placement...), not words read off a document",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "[BOT BOD]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "[CL  CL@]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "[EL AFF]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "INVERT | INV",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "const PIPE_SYSTEMS",
        "rows with numbers or extra fields (sizes, defaults, flags, exclusion words): a table, not a word list; a later item can give it a table shape",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "const SHORT_ABBR",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    ("crates/engine/src/cmd/mep.rs", "& | AND", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/mep.rs",
        "A-WALL | HATCH",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/engine/src/cmd/mep.rs",
        "const TRIAGE_FIELDS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/lighting.rs",
        "const FIELDS",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/engine/src/cmd/lighting.rs",
        "W | VA",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/lighting.rs",
        "[WAY POLE]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/engine/src/cmd/lighting.rs",
        "[luminaire light]",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    ("crates/engine/src/cmd/walltypes.rs", "EACH | BOTH", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/engine/src/cmd/bspec.rs",
        "[replace auto]",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
    (
        "crates/engine/src/cmd/bspec.rs",
        "wall | curtainwall",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/sheet_facts.rs",
        "[SHEET NO SHEET NUMBER]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/sheet_facts.rs",
        "LIFE SAFETY | OCCUPANCY LOAD",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    ("crates/mepfix/src/sheet_reader.rs", "const WORDS", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/sheet_reader.rs", "ST | ND", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/sheet_reader.rs", "- | THRU", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/mepfix/src/sheet_reader.rs",
        "[PLAN SCHEDULE]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "[SEE  REFER TO ]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "[DETAILS DETAIL]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "[SHEETS SHEET]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "[A.F.F. A.F.F]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "[SEE SHEET]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "plan | rcp",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "enlarged_plan | enlarged_ref",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/printlinks.rs",
        "detail_bubble | detail_ref",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/printset.rs",
        "gap | guessed",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[TAG MARK]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[MANUFACTURER MANUFACTURERS]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[MODEL CATALOG]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[REMARKS NOTES]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[CW C W]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[HW H W]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[WASTE W]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/fixture_schedule.rs",
        "[AMPS AMP]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/arch_reader.rs",
        "[DOOR SCHEDULE HARDWARE]",
        "drafting convention: view, reference and table types (ELEVATION, SECTION, DETAIL, SEE SHEET, SCHEDULE), how any set is organised",
    ),
    (
        "crates/mepfix/src/unit_reader.rs",
        "[devices terminals]",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/casework_reader.rs",
        "sink | lavatory",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "const SYSTEM_WORDS",
        "rows with numbers or extra fields (sizes, defaults, flags, exclusion words): a table, not a word list; a later item can give it a table shape",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "[TEXT ANNO]",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "TAG | MARK",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "[CFM MAX CFM]",
        "document format: the field and column labels of schedules, cut sheets, panel schedules and title blocks",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "pump | water_heater",
        "model kinds and property values: our own names (Q6211 kindenum owns them), not words read off a document",
    ),
    (
        "crates/mepfix/src/equip_reader.rs",
        "BOD | BOT",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/mepfix/src/duct_reader.rs",
        "A-WALL | HATCH",
        "CAD layer-name parts (NCS codes such as IDEN, DIMS, POWR, A-WALL): drafting convention of layer naming",
    ),
    (
        "crates/mepfix/src/duct_reader.rs",
        "[BOT BOD]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/mepfix/src/duct_reader.rs",
        "[CL  CL@]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    (
        "crates/mepfix/src/duct_reader.rs",
        "[EL AFF]",
        "drafting notation: height, elevation and dimension abbreviations (EL, AFF, BOT, CL, INV, HR), the same on every set",
    ),
    ("crates/mepfix/src/duct_reader.rs", "[SA RA]", "the four air systems' codes (SA, RA, EA, OA), the same codes as equip_reader's system table"),
    ("crates/mepfix/src/framing.rs", "const W", "standard size tables (rebar, steel shapes): catalogue data, not reading words"),
    ("crates/mepfix/src/asbuilt.rs", "cloud | rectangle", "markup shapes of the as-built format (cloud, rectangle, ink...): format vocabulary"),
    ("crates/mepfix/src/asbuilt.rs", "[delete deleted]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[verify verified]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[raise raised]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[move moved]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[upsize upsized]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[type change to]", "grammar: ordinal, number, joining and markup words of the language"),
    ("crates/mepfix/src/asbuilt.rs", "[add added]", "grammar: ordinal, number, joining and markup words of the language"),
    (
        "crates/mepfix/src/asbuilt.rs",
        "[ARROW: moved (old to new) X: deleted, not built]",
        "our own output and command vocabulary (generated sheet names, file types, command ids, CAD entity types, layer names we write)",
    ),
];

#[test]
fn every_list_the_code_asks_for_is_in_the_data_and_every_list_there_is_asked_for() {
    let data = wr::builtin();
    assert!(data.problems().is_empty(), "{:?}", data.problems());
    let mut files = Vec::new();
    rust_files(&root().join("crates/engine/src"), &mut files);
    rust_files(&root().join("crates/mepfix/src"), &mut files);
    let call = regex_lite_calls();
    let mut asked: BTreeMap<String, (Shape, String)> = BTreeMap::new();
    let mut wrong = Vec::new();
    for f in &files {
        if f.ends_with("mepfix/src/wordrules.rs") {
            continue;
        }
        let s = source(f);
        for (func, key) in call(&s) {
            let shape = match func.as_str() {
                "words" => Shape::Words,
                "pairs" | "pair_value" => Shape::Pairs,
                "groups" | "group" => Shape::Groups,
                _ => Shape::Rules,
            };
            match data.shape(&key) {
                Some(have) if have == shape => {}
                have => wrong.push(format!("{}: `{func}(\"{key}\")` but the data has {:?}", f.display(), have)),
            }
            asked.insert(key, (shape, f.display().to_string()));
        }
    }
    assert!(wrong.is_empty(), "lists the code asks for that the data lacks or shapes otherwise:\n{}", wrong.join("\n"));
    let unasked: Vec<&str> = data.keys().into_iter().map(|(k, _)| k).filter(|k| !asked.contains_key(*k)).collect();
    assert!(unasked.is_empty(), "lists in wordrules.json no code asks for: {unasked:?}");
    assert!(asked.len() >= 90, "{}", asked.len());
}

/// `wordrules::words("key")`-style calls in a file: (function, key).
fn regex_lite_calls() -> impl Fn(&str) -> Vec<(String, String)> {
    |s: &str| {
        let mut out = Vec::new();
        let t = tokens(s);
        for i in 0..t.len() {
            if let (Some(Tok::Ident(m)), Some(Tok::Punct(':')), Some(Tok::Punct(':')), Some(Tok::Ident(f)), Some(Tok::Punct('(')), Some(Tok::Str(k))) =
                (t.get(i), t.get(i + 1), t.get(i + 2), t.get(i + 3), t.get(i + 4), t.get(i + 5))
                && m == "wordrules"
                && matches!(f.as_str(), "words" | "pairs" | "groups" | "group" | "rules" | "first_rule" | "pair_value")
            {
                out.push((f.clone(), k.clone()));
            }
        }
        out
    }
}

#[test]
fn no_new_word_list_creeps_into_the_reading_code() {
    let allowed: BTreeSet<(&str, &str)> = ALLOWED.iter().map(|(f, s, _)| (*f, *s)).collect();
    for (f, s, why) in ALLOWED {
        assert!(!why.trim().is_empty(), "{f} {s}: no reason");
    }
    let mut new = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for f in READING {
        let p = root().join(f);
        assert!(p.exists(), "{f} is gone: update READING");
        for (sig, n) in word_lists(&tokens(&source(&p))) {
            seen.insert((f.to_string(), sig.clone()));
            if !allowed.contains(&(*f, sig.as_str())) {
                new.push(format!("    (\"{f}\", \"{}\", \"…\"), // {n} words", sig.replace('\\', "\\\\").replace('"', "\\\"")));
            }
        }
    }
    assert!(
        new.is_empty(),
        "word lists of 4+ in the reading code: move them to crates/engine/src/cmd/wordrules.json (cadaddy_mepfix::wordrules), or allow one with its reason:\n{}",
        new.join("\n")
    );
    let stale: Vec<String> =
        ALLOWED.iter().filter(|(f, s, _)| !seen.contains(&(f.to_string(), s.to_string()))).map(|(f, s, _)| format!("{f} {s}")).collect();
    assert!(stale.is_empty(), "allowed lists no longer in the code (take them off ALLOWED): {stale:?}");
}

#[test]
fn the_scanner_finds_each_form_of_word_list() {
    let src = r#"
        const A: &[&str] = &["a", "b", "c", "d"];
        static B: [&str; 3] = ["a", "b", "c"];
        fn f(x: &str) -> bool {
            // ["no", "not", "in", "comments"]
            let c = '"';
            ["p", "q", "r", "s"].contains(&x) || matches!(x, "w" | "x" | "y" | "z") || x == "one"
        }
    "#;
    let found: Vec<String> = word_lists(&tokens(src)).into_iter().map(|(s, _)| s).collect();
    assert_eq!(found, vec!["const A", "[p q]", "w | x"]);
}

#[test]
fn a_job_adds_a_word_and_an_unknown_list_is_refused() {
    let mut s = Session::new();
    // Before: the shipped list.
    assert_eq!(cadaddy_mepfix::casework_reader::kind_from_description("SLOP HOPPER 24 IN"), "");
    let r = s
        .execute(
            "project.rules.set",
            &json!({"domain": "custom", "key": "words:fixtures.kind_by_description", "value": "SLOP HOPPER=mop_sink", "tier": "project_rule", "reason": "the job's schedule calls it so"}),
        )
        .expect("set");
    assert_eq!(r["wordLists"]["added"]["fixtures.kind_by_description"][0], "SLOP HOPPER=mop_sink", "{r}");
    // The reader below the engine reads the job's word now; the shipped words still come first.
    assert_eq!(cadaddy_mepfix::casework_reader::kind_from_description("SLOP HOPPER 24 IN"), "mop_sink");
    assert_eq!(cadaddy_mepfix::casework_reader::kind_from_description("MOP SINK"), "mop_sink");
    let rules = s.execute("project.rules.list", &json!({})).expect("list");
    assert!(rules.to_string().contains("words:fixtures.kind_by_description"), "{rules}");

    // An unknown list, a pair that doesn't read, the wrong domain: refused, nothing changes.
    for (domain, key, value) in [
        ("custom", "words:fixtures.no_such_list", "X"),
        ("custom", "words:fixtures.kind_by_description", "NO EQUALS SIGN"),
        ("naming", "words:rooms.sleeping", "DORM"),
    ] {
        let e = s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "tier": "project_rule", "reason": "test"}));
        assert!(e.is_err(), "{key} {value}: {e:?}");
    }
    assert!(!wr::words("rooms.sleeping").contains(&"DORM"));

    // A drawing without the rule: the shipped lists again.
    s.execute("new", &json!({})).expect("new");
    s.execute("document.list", &json!({})).expect("list");
    assert_eq!(cadaddy_mepfix::casework_reader::kind_from_description("SLOP HOPPER 24 IN"), "");
    assert!(wr::report().added.is_empty());
}
