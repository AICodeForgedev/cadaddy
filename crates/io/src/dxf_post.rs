//! Writing a DXF without holding it (a large drawing's file can be gigabytes). The writer
//! ([`crate::dxf_write::emit`]) hands its records over as it makes them; the passes that need the
//! whole file take what they need from earlier passes over the records instead:
//!
//! 1. Only for a drawing with foreign data, entity xdata or entities of types CADaddy doesn't
//!    model: a scan of the records for putting that back ([`dxf_raw::Scan`]).
//! 2. A count: the records as they will be written (foreign data put back), counted, with what
//!    the parts before them need: the records by type (the classes' instance counts), the
//!    applications with xdata (APPID records), the largest handle (the header's `$HANDSEED`).
//!    A file too big to reopen is refused from this count ([`Plan::groups`]), before anything is
//!    written.
//! 3. The write: the records again, finished as they go by and handed on one group at a time
//!    ([`Plan::write`]).
//!
//! The writer runs once per pass; what each pass keeps is the size of the drawing's tables,
//! block headers and dictionaries, never its entities' groups.

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_doc::Drawing;
use cadaddy_dxf::{Tag, Version};

use crate::dxf_raw::{self, DataStore, Finisher, Scan};
use crate::dxf_write::{Views, emit};

/// What is written: the version declared in `$ACADVER`, the version the file ends up in
/// (`version`, or the DWG version the bridge writes), whether for a DWG writer, and whether for
/// the DWG bridge.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Job {
    pub version: Version,
    pub target: Version,
    pub dwg: bool,
    pub bridge: bool,
}

/// The records of `tags`, each from its `0` group.
fn records(tags: &[Tag]) -> impl Iterator<Item = &[Tag]> {
    tags.chunk_by(|_, b| b.code != 0)
}

/// One pass of the writer: each record as it will be written, foreign data put back (`fin`),
/// to `each` (`false` stops the pass). The next free handle the writer left is returned.
fn finished(v: &Views, job: Job, mut fin: Option<&mut Finisher>, each: &mut dyn FnMut(&[Tag]) -> bool) -> u64 {
    if let Some(f) = fin.as_deref_mut() {
        f.rewind();
    }
    let mut buf: Vec<Tag> = Vec::new();
    let mut go = |chunk: Vec<Tag>| -> bool {
        for rec in records(&chunk) {
            match fin.as_deref_mut() {
                Some(f) => {
                    buf.clear();
                    f.record(rec, &mut buf);
                    if !records(&buf).all(&mut *each) {
                        return false;
                    }
                }
                None => {
                    if !each(rec) {
                        return false;
                    }
                }
            }
        }
        true
    };
    emit(v, job.version, job.target, job.dwg, job.bridge, &mut go)
}

/// Follows the section the records are in.
#[derive(Default)]
struct Sections {
    sec: String,
}

impl Sections {
    fn next(&mut self, kind: &str, body: &[Tag]) -> String {
        if kind == "SECTION" {
            self.sec = body.iter().find(|t| t.code == 2).map(Tag::str).unwrap_or_default();
        }
        let sec = self.sec.clone();
        if kind == "ENDSEC" {
            self.sec.clear();
        }
        sec
    }
}

/// A record's first group `code`, before its xdata.
fn val(body: &[Tag], code: i32) -> Option<String> {
    body.iter().take_while(|t| t.code < 1000).find(|t| t.code == code).map(Tag::str)
}

/// Where the APPID table is, as the records go by: its TABLE record (the first of TABLES), the
/// APPID records right after it, and its ENDTAB.
#[derive(Default)]
struct AppidTable {
    /// 0: not seen; 1: its APPID records going by; 2: past them; 3: its ENDTAB passed.
    state: u8,
}

impl AppidTable {
    /// Move past a record; what it is to the table.
    fn next(&mut self, sec: &str, kind: &str, body: &[Tag]) -> AppidPart {
        match self.state {
            0 if sec == "TABLES" && kind == "TABLE" && val(body, 2).is_some_and(|n| n.trim().eq_ignore_ascii_case("APPID")) => {
                self.state = 1;
                AppidPart::Table
            }
            1 if kind == "APPID" => AppidPart::Record,
            1 | 2 if kind == "ENDTAB" => {
                self.state = 3;
                AppidPart::End
            }
            1 => {
                self.state = 2;
                AppidPart::Other
            }
            _ => AppidPart::Other,
        }
    }
}

#[derive(PartialEq)]
enum AppidPart {
    Table,
    Record,
    End,
    Other,
}

/// What the count pass learns of the finished records.
#[derive(Default)]
struct Summary {
    groups: usize,
    /// Records by type (upper case), CLASS records aside.
    kinds: HashMap<String, i64>,
    /// Applications with xdata: upper case → as written (the last).
    appids: BTreeMap<String, String>,
    /// The APPID table's handle, its records (upper case), and whether its ENDTAB came.
    appid_table: Option<String>,
    have: HashSet<String>,
    appid_end: bool,
    max_handle: u64,
    /// CLASS records without a group 91 that have a 90 (one is inserted).
    classes_without_count: usize,
    sections: Sections,
    table: AppidTable,
}

impl Summary {
    fn note(&mut self, rec: &[Tag]) {
        let Some(zero) = rec.first() else { return };
        let body = rec.get(1..).unwrap_or(&[]);
        let kind = zero.str();
        let sec = self.sections.next(&kind, body);
        self.groups += rec.len();
        if kind != "CLASS" {
            *self.kinds.entry(kind.to_ascii_uppercase()).or_default() += 1;
        } else if !body.iter().any(|t| t.code == 91) && body.iter().any(|t| t.code == 90) {
            self.classes_without_count += 1;
        }
        for t in rec.iter().filter(|t| t.code == 1001) {
            let s = t.str();
            self.appids.insert(s.trim().to_ascii_uppercase(), s.trim().to_string());
        }
        match self.table.next(&sec, &kind, body) {
            AppidPart::Table => {
                self.appid_table = Some(dxf_raw::record_handle(body).unwrap_or_else(|| "0".into()));
            }
            AppidPart::Record => {
                self.have.extend(val(body, 2).map(|n| n.trim().to_ascii_uppercase()));
            }
            AppidPart::End => self.appid_end = true,
            AppidPart::Other => {}
        }
        if let Some(m) = dxf_raw::max_handle(body) {
            self.max_handle = self.max_handle.max(m);
        }
    }
}

/// A DXF write planned from its count: refused (or not) on [`Self::groups`], then written with
/// [`Self::write`].
pub(crate) struct Plan<'d> {
    views: &'d Views<'d>,
    job: Job,
    fin: Option<Finisher<'d>>,
    /// Groups of the finished file.
    groups: usize,
    /// Records by type, for the classes' instance counts.
    kinds: HashMap<String, i64>,
    /// The header's `$HANDSEED`.
    seed: u64,
    /// APPID records added to the APPID table (before its ENDTAB), with the table's new count.
    appids: Vec<Tag>,
    added_appids: i64,
}

impl<'d> Plan<'d> {
    /// Count what writing `views` for `job` gives (the writer's first one or two passes).
    pub(crate) fn new(views: &'d Views<'d>, job: Job) -> Self {
        let d = views.saved();
        let mut fin = Scan::of(d).map(|mut scan| {
            let seed = emit(views, job.version, job.target, job.dwg, job.bridge, &mut |chunk: Vec<Tag>| {
                records(&chunk).for_each(|r| scan.record(r));
                true
            });
            Finisher::new(scan, seed)
        });
        let mut sum = Summary::default();
        let mut data = DataStore::of(d, job.target >= Version::R2013);
        let raw_seed = finished(views, job, fin.as_mut(), &mut |rec: &[Tag]| {
            if rec.first().is_some_and(|z| z.code == 0 && z.str() == "EOF")
                && let Some(ds) = &data
            {
                records(&ds.section()).for_each(|r| sum.note(r));
            }
            if let Some(ds) = data.as_mut() {
                ds.note(rec);
            }
            sum.note(rec);
            true
        });
        // Foreign data put back: APPID records for the applications with xdata, numbered above
        // every handle, and the seed past them.
        let mut appids = Vec::new();
        let mut seed = raw_seed;
        if fin.is_some() {
            let top = sum.max_handle.max(raw_seed);
            let mut next = raw_seed.max(top.saturating_add(1));
            if let Some(table) = sum.appid_table.as_ref().filter(|_| sum.appid_end) {
                for app in sum.appids.iter().filter(|(k, _)| !sum.have.contains(*k)).map(|(_, v)| v) {
                    appids.extend([
                        Tag::s(0, "APPID"),
                        Tag::s(5, format!("{next:X}")),
                        Tag::s(330, table.clone()),
                        Tag::s(100, "AcDbSymbolTableRecord"),
                        Tag::s(100, "AcDbRegAppTableRecord"),
                        Tag::s(2, app.clone()),
                        Tag::i(70, 0),
                    ]);
                    next = next.saturating_add(1);
                }
            }
            let top = if appids.is_empty() { top } else { top.max(next.saturating_sub(1)) };
            let top = top.saturating_add(1);
            seed = if raw_seed < top { top } else { raw_seed };
        }
        let added = i64::try_from(appids.iter().filter(|t| t.code == 0).count()).unwrap_or(0);
        if added > 0 {
            *sum.kinds.entry("APPID".into()).or_default() += added;
        }
        Plan { views, job, fin, groups: sum.groups + appids.len() + sum.classes_without_count, kinds: sum.kinds, seed, added_appids: added, appids }
    }

    /// Groups the finished file holds (as reading it counts them).
    pub(crate) fn groups(&self) -> usize {
        self.groups
    }

    /// Write the file: each finished group to `out` in order (`false` stops). `false` when it
    /// stopped.
    pub(crate) fn write(&mut self, out: &mut dyn FnMut(&Tag) -> bool) -> bool {
        let views = self.views;
        let d = views.saved();
        let job = self.job;
        let mut data = DataStore::of(d, job.target >= Version::R2013);
        let xrefs = xrefs(d);
        let (seed, kinds, appids, added) = (self.seed, &self.kinds, &self.appids, self.added_appids);
        let mut sections = Sections::default();
        let mut table = AppidTable::default();
        let mut ok = true;
        let mut rec_buf: Vec<Tag> = Vec::new();
        let mut put = |rec: &mut Vec<Tag>, sec: &str| -> bool {
            let kind = rec.first().map(Tag::str).unwrap_or_default();
            match kind.as_str() {
                "SECTION" if sec == "HEADER" => set_seed(rec, seed),
                "CLASS" => set_class_count(rec, kinds),
                "BLOCK" if !xrefs.is_empty() => mark_xref(rec, &xrefs),
                _ => {}
            }
            rec.iter().all(&mut *out)
        };
        let mut each = |rec: &[Tag]| -> bool {
            let Some(zero) = rec.first() else { return true };
            let body = rec.get(1..).unwrap_or(&[]);
            let kind = zero.str();
            let sec = sections.next(&kind, body);
            let part = table.next(&sec, &kind, body);
            if part == AppidPart::End
                && !appids.is_empty()
                && !records(appids).all(|r| {
                    rec_buf.clear();
                    rec_buf.extend_from_slice(r);
                    put(&mut rec_buf, &sec)
                })
            {
                return false;
            }
            if kind == "EOF"
                && let Some(ds) = &data
            {
                let section = ds.section();
                let mut s = Sections::default();
                for r in records(&section) {
                    rec_buf.clear();
                    rec_buf.extend_from_slice(r);
                    let sec = s.next(&rec_buf.first().map(Tag::str).unwrap_or_default(), r.get(1..).unwrap_or(&[]));
                    if !put(&mut rec_buf, &sec) {
                        return false;
                    }
                }
            }
            if let Some(ds) = data.as_mut() {
                ds.note(rec);
            }
            rec_buf.clear();
            rec_buf.extend_from_slice(rec);
            if part == AppidPart::Table && added > 0 {
                set_appid_count(&mut rec_buf, added);
            }
            put(&mut rec_buf, &sec)
        };
        finished(views, job, self.fin.as_mut(), &mut |rec: &[Tag]| {
            ok = each(rec);
            ok
        });
        ok
    }
}

/// The header's `$HANDSEED` set to `seed` (the group 5 after it).
fn set_seed(rec: &mut [Tag], seed: u64) {
    let at = rec.windows(2).position(|w| matches!(w, [a, b] if a.code == 9 && a.str() == "$HANDSEED" && b.code == 5)).map(|i| i + 1);
    if let Some(t) = at.and_then(|i| rec.get_mut(i)) {
        *t = Tag::s(5, format!("{seed:X}"));
    }
}

/// The APPID table record's count (its group 70) raised by `added`.
fn set_appid_count(rec: &mut [Tag], added: i64) {
    if let Some(count) = rec.get_mut(1..).and_then(|b| b.iter_mut().find(|x| x.code == 70)) {
        *count = Tag::i(70, count.i64().saturating_add(added));
    }
}

/// DXF Reference group 91 on a CLASS record: how many records of the class the file holds
/// (`kinds`, by upper-case type), in place of its 91 or after its 90.
fn set_class_count(rec: &mut Vec<Tag>, kinds: &HashMap<String, i64>) {
    let name = rec.iter().skip(1).find(|t| t.code == 1).map(Tag::str).unwrap_or_default().to_ascii_uppercase();
    let count = kinds.get(&name).copied().unwrap_or(0);
    if let Some(i) = rec.iter().skip(1).position(|t| t.code == 91) {
        if let Some(t) = rec.get_mut(i + 1) {
            *t = Tag::i(91, count);
        }
    } else if let Some(i) = rec.iter().skip(1).position(|t| t.code == 90) {
        rec.insert(i + 2, Tag::i(91, count));
    }
}

/// Xref blocks of `d` by upper-case name: the external drawing's path, and the flags (DXF
/// Reference, BLOCK group 70) they add: 4, and 8 for an overlay.
fn xrefs(d: &Drawing) -> HashMap<String, (String, i64)> {
    d.blocks.values().filter_map(|b| Some((b.name.to_ascii_uppercase(), (b.xref_path.clone()?, if b.xref_overlay { 12 } else { 4 })))).collect()
}

/// A written BLOCK record of an xref: the flag bits in group 70 and the path in group 1, once
/// its name (2) says it is one.
fn mark_xref(rec: &mut [Tag], xrefs: &HashMap<String, (String, i64)>) {
    let mut block: Option<Option<&(String, i64)>> = None;
    for t in rec.iter_mut() {
        match (t.code, block) {
            (0, _) => block = (t.str() == "BLOCK").then_some(None),
            (2, Some(None)) => block = Some(xrefs.get(&t.str().to_ascii_uppercase())),
            (70, Some(Some((_, flags)))) => *t = Tag::i(70, t.i64() | flags),
            (1, Some(Some((p, _)))) => *t = Tag::s(1, p.clone()),
            _ => {}
        }
    }
}

/// The finished groups of `views` written for `job`, in memory.
pub(crate) fn tags(views: &Views, job: Job) -> Vec<Tag> {
    let mut plan = Plan::new(views, job);
    let mut out = Vec::with_capacity(plan.groups());
    plan.write(&mut |t| {
        out.push(t.clone());
        true
    });
    out
}
