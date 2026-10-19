//! ATTOUT / ATTIN: block attribute values out to a tab-separated text file and back in, to be
//! edited in a spreadsheet between.
//!
//! The file's first row names the columns: `HANDLE`, `BLOCKNAME`, then every attribute tag of the
//! exported references (in the order first met). Each following row is one block reference: its
//! handle after an apostrophe (so spreadsheets keep it text), its block name and its values; `<>`
//! marks a tag that block doesn't have. ATTIN finds each reference by its handle and sets the
//! values of the tags it has, leaving `<>` cells alone.

use cadaddy_doc::{EntityKind, Handle};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// A tag a block reference doesn't have.
const NONE: &str = "<>";
/// Largest file ATTIN reads, and most rows and columns.
const MAX_FILE: u64 = 64 << 20;
const MAX_ROWS: usize = 1_000_000;
const MAX_COLUMNS: usize = 10_000;

/// A cell: tabs and line breaks would break the row.
fn cell(v: &str) -> String {
    v.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect()
}

/// The file next to the drawing, named after it.
fn default_path(s: &Session) -> String {
    match s.state() {
        Ok(st) => {
            let stem = st.title.rsplit_once('.').map_or(st.title.as_str(), |(a, _)| a).to_string();
            format!("{}{stem}.txt", crate::sysvars::dwg_prefix(st.path.as_deref()))
        }
        Err(_) => "attributes.txt".into(),
    }
}

/// The block references with attributes among `hs`, or in the current space when `hs` is empty.
fn references(s: &Session, hs: &[Handle]) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    let has = |h: &Handle| matches!(d.entity(*h).map(|e| &e.kind), Some(EntityKind::Insert(i)) if !i.attribs.is_empty());
    if !hs.is_empty() {
        return Ok(hs.iter().copied().filter(has).take(MAX_ROWS).collect());
    }
    let sp = s.space();
    Ok(d.space(&sp).map(|st| st.iter().map(|e| e.handle).filter(has).take(MAX_ROWS).collect()).unwrap_or_default())
}

/// The file's text for `refs`.
pub fn attout_text(s: &Session, refs: &[Handle]) -> Result<(String, usize)> {
    let d = s.doc()?;
    let mut tags: Vec<String> = Vec::new();
    for h in refs {
        if let Some(EntityKind::Insert(i)) = d.entity(*h).map(|e| &e.kind) {
            for a in &i.attribs {
                if tags.len() < MAX_COLUMNS && !tags.iter().any(|t| t.eq_ignore_ascii_case(&a.tag)) {
                    tags.push(a.tag.to_ascii_uppercase());
                }
            }
        }
    }
    let mut out = String::from("HANDLE\tBLOCKNAME");
    for t in &tags {
        out.push('\t');
        out.push_str(&cell(t));
    }
    out.push_str("\r\n");
    let mut rows = 0;
    for h in refs {
        let Some(EntityKind::Insert(i)) = d.entity(*h).map(|e| &e.kind) else { continue };
        out.push('\'');
        out.push_str(&h.hex());
        out.push('\t');
        out.push_str(&cell(&i.block));
        for t in &tags {
            out.push('\t');
            match i.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(t)) {
                Some(a) => out.push_str(&cell(&a.text.value)),
                None => out.push_str(NONE),
            }
        }
        out.push_str("\r\n");
        rows += 1;
    }
    Ok((out, rows))
}

fn run_attout(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "attout";
    let picked = match p.get("handles") {
        Some(_) => targets(s, p)?,
        None => s.selection(),
    };
    let refs = references(s, &picked)?;
    if refs.is_empty() {
        return Err(bad(cmd, "no block references with attributes"));
    }
    let (text, rows) = attout_text(s, &refs)?;
    let path = str_param(p, "path").map_or_else(|| default_path(s), str::to_string);
    write_file(cmd, &path, &text)?;
    Ok(json!({ "path": path, "rows": rows }))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_file(cmd: &str, path: &str, text: &str) -> Result<()> {
    crate::writefile::write(path, text).map_err(|e| bad(cmd, format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn write_file(cmd: &str, _path: &str, _text: &str) -> Result<()> {
    Err(bad(cmd, "files are not available in the web version"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file(cmd: &str, path: &str) -> Result<String> {
    use std::io::Read;
    let f = crate::readfile::open(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    let mut bytes = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(bad(cmd, format!("{path} is larger than 64 MB")));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_arch = "wasm32")]
fn read_file(cmd: &str, _path: &str) -> Result<String> {
    Err(bad(cmd, "files are not available in the web version"))
}

/// Apply a file's text: `(updated values, rows read, rows whose handle wasn't found)`.
pub fn attin_text(s: &mut Session, text: &str) -> Result<(usize, usize, usize)> {
    let cmd = "attin";
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .ok_or_else(|| bad(cmd, "the file is empty"))?
        .split('\t')
        .take(MAX_COLUMNS)
        .map(|c| c.trim().trim_start_matches('\u{feff}').to_string())
        .collect();
    let col = |name: &str| header.iter().position(|c| c.eq_ignore_ascii_case(name));
    let (Some(hcol), Some(bcol)) = (col("HANDLE"), col("BLOCKNAME")) else {
        return Err(bad(cmd, "the first row must name the HANDLE and BLOCKNAME columns"));
    };
    let (mut updated, mut rows, mut missing) = (0, 0, 0);
    for line in lines.take(MAX_ROWS) {
        let cells: Vec<&str> = line.split('\t').take(MAX_COLUMNS).collect();
        rows += 1;
        let Some(h) = cells.get(hcol).map(|c| c.trim().trim_start_matches('\'')).and_then(Handle::parse_hex) else {
            missing += 1;
            continue;
        };
        let is_ref = matches!(s.doc()?.entity(h).map(|e| &e.kind), Some(EntityKind::Insert(_)));
        if !is_ref || super::super::curves::is_locked(s, h) {
            missing += 1;
            continue;
        }
        let mut changed = 0;
        s.doc_mut()?.modify_entity(h, |e| {
            let EntityKind::Insert(i) = &mut e.kind else { return };
            for (k, tag) in header.iter().enumerate() {
                if k == hcol || k == bcol {
                    continue;
                }
                let Some(v) = cells.get(k).map(|v| v.trim_end_matches('\r')) else { continue };
                if v == NONE {
                    continue;
                }
                for a in i.attribs.iter_mut().filter(|a| a.tag.eq_ignore_ascii_case(tag) && !a.constant) {
                    if a.text.value != v {
                        a.text.value = v.to_string();
                        changed += 1;
                    }
                }
            }
        })?;
        updated += changed;
    }
    Ok((updated, rows, missing))
}

fn run_attin(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map_or_else(|| default_path(s), str::to_string);
    let text = read_file("attin", &path)?;
    let (updated, rows, missing) = attin_text(s, &text)?;
    Ok(json!({ "path": path, "updated": updated, "rows": rows, "notFound": missing }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("attout", "Export Attribute Information", run_attout)
            .menu(&["Tools", "Express Tools", "Blocks", "Export Attribute Information"])
            .params("{path? (default: the drawing's name with .txt, by the drawing), handles? (block references; default the selection, else every one with attributes in the current space)} → {path, rows}")
            .noundo()
            .interactive(|s| Ok(Box::new(AttFileM::new(s, true)))),
        CommandSpec::new("attin", "Import Attribute Information", run_attin)
            .menu(&["Tools", "Express Tools", "Blocks", "Import Attribute Information"])
            .params("{path? (default as ATTOUT's)} → {path, updated (values changed), rows, notFound (rows whose handle is not a block reference here)}")
            .interactive(|s| Ok(Box::new(AttFileM::new(s, false)))),
    ]
}

/// The file name, then (ATTOUT) the block references: Enter takes them all.
struct AttFileM {
    out: bool,
    path: Option<String>,
    sel: SelectPhase,
    default: String,
}

impl AttFileM {
    fn new(s: &Session, out: bool) -> AttFileM {
        AttFileM { out, path: None, sel: SelectPhase::default(), default: default_path(s) }
    }

    fn finish(&self, s: &mut Session, hs: Vec<Handle>) -> Result<Step> {
        let path = self.path.clone().unwrap_or_else(|| self.default.clone());
        let r = if self.out {
            run_attout(s, &json!({ "path": path, "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
                .map(|r| format!("{} block reference(s) written to {path}", r["rows"]))
        } else {
            run_attin(s, &json!({ "path": path })).map(|r| format!("{} attribute value(s) updated from {} row(s).", r["updated"], r["rows"]))
        };
        match r {
            Ok(m) => s.echo(m),
            Err(e) => s.echo(e.to_string()),
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

impl Interactive for AttFileM {
    fn name(&self) -> &'static str {
        if self.out { "ATTOUT" } else { "ATTIN" }
    }
    fn prompt(&self, _: &Session) -> Prompt {
        match &self.path {
            None if self.out => Prompt::new("Enter output filename", Accept::TEXT).default(self.default.clone()),
            None => Prompt::new("Enter input filename", Accept::TEXT).default(self.default.clone()),
            Some(_) => Prompt::new("Select objects (Enter for all)", Accept::SELECT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.path.is_none() {
            let t = match &i {
                Input::Text(t) => t.trim().trim_matches('"').to_string(),
                _ => String::new(),
            };
            self.path = Some(if t.is_empty() { self.default.clone() } else { t });
            if !self.out {
                return self.finish(s, Vec::new());
            }
            self.sel = SelectPhase::begin(s);
            if self.sel.done {
                let hs = self.sel.picked.clone();
                return self.finish(s, hs);
            }
            return Ok(Step::Continue);
        }
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => self.finish(s, Vec::new()),
            SelOutcome::Done(hs) => self.finish(s, hs),
        }
    }
}
