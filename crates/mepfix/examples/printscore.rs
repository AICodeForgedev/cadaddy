//! Scores the print readers on a print set that isn't ours to ship: a folder of PDF sheets (one
//! sheet per page or a merged set) and, optionally, revision folders whose sheets replace the
//! same sheet number. It prints only counts, rates, timings and error classes, never a sheet's
//! name, number, title or text, so its output can be shared when the prints can't.
//!
//! `cargo run --release -p cadaddy-mepfix --example printscore -- [--pages N] [--detail] <pdf or folder>…`
//!
//! Stages: every page imported (`pdf_read`), each recognised as a sheet (number, discipline,
//! level, scale, grid), newer revisions replacing older sheets of the same number, the sheet
//! map assembled, then the readers on the sheets they read: ceilings, structure, walls,
//! architecture, ducts, pipes, sprinklers, conduits and callouts, and the heights of the
//! conduit runs (from a note on the print vs a stacking rule).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cadaddy_doc::Drawing;
use cadaddy_mepfix::sheet_reader::{self, RecognisedSheet};

fn pdfs_in(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_file() {
        if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            out.push(p.to_path_buf());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        pdfs_in(&e, out);
    }
}

/// An error message reduced to its class: digits and quoted parts dropped.
fn class(e: &str) -> String {
    let mut s: String = e.chars().take(80).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    if let Some(i) = s.find('"') {
        s.truncate(i);
    }
    s
}

struct Page {
    /// Which input file (index) and page.
    file: usize,
    page: u32,
    rec: RecognisedSheet,
}

fn import(bytes: &[u8], page: u32) -> Result<cadaddy_io::pdf_read::PdfImport, String> {
    let opts = cadaddy_io::pdf_read::PdfOptions { page, images: false, markups: false, layout: false, ..Default::default() };
    cadaddy_io::pdf_read::read(bytes, &opts).map_err(|e| e.to_string())
}

fn main() {
    let mut max_pages = usize::MAX;
    let mut detail = false;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pages" => max_pages = args.next().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX),
            "--detail" => detail = true,
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    let mut files = Vec::new();
    for i in &inputs {
        pdfs_in(i, &mut files);
    }
    println!("files: {}", files.len());

    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut pages: Vec<Page> = Vec::new();
    let mut stats: BTreeMap<&str, f64> = BTreeMap::new();
    let t_all = Instant::now();
    for (fi, f) in files.iter().enumerate() {
        let Ok(bytes) = std::fs::read(f) else {
            *errors.entry("unreadable file".into()).or_default() += 1;
            continue;
        };
        let n = match cadaddy_io::pdf_read::page_count(&bytes) {
            Ok(n) => n as usize,
            Err(e) => {
                *errors.entry(format!("page count: {}", class(&e.to_string()))).or_default() += 1;
                continue;
            }
        };
        for pg in 1..=n.min(max_pages) {
            let t = Instant::now();
            match import(&bytes, pg as u32) {
                Ok(imp) => {
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    *stats.entry("import_ms").or_default() += ms;
                    *stats.entry("pages").or_default() += 1.0;
                    *stats.entry("entities").or_default() += imp.drawing.model.len() as f64;
                    let t = Instant::now();
                    let rec = sheet_reader::recognise_dxf(&imp.drawing);
                    *stats.entry("recognise_ms").or_default() += t.elapsed().as_secs_f64() * 1000.0;
                    if detail {
                        println!(
                            "  file {fi} page {pg}: {} ents, {:.0} ms, number {}, disc {:?}, level {}, scale {:.4} ({}), grid {}x{}, bubbles {}",
                            imp.drawing.model.len(),
                            ms,
                            if rec.number.is_empty() { "-" } else { "yes" },
                            rec.discipline,
                            rec.level.is_some(),
                            rec.scale,
                            imp.scale_source.split_whitespace().next().unwrap_or(""),
                            rec.grid_x.len(),
                            rec.grid_y.len(),
                            rec.bubbles.len()
                        );
                    }
                    pages.push(Page { file: fi, page: pg as u32, rec });
                }
                Err(e) => *errors.entry(format!("import: {}", class(&e))).or_default() += 1,
            }
        }
    }
    let n = pages.len().max(1) as f64;
    let count = |f: &dyn Fn(&RecognisedSheet) -> bool| pages.iter().filter(|p| f(&p.rec)).count();
    println!(
        "pages imported: {} in {:.1} s ({:.0} ms/page, {:.0} entities/page)",
        pages.len(),
        t_all.elapsed().as_secs_f64(),
        stats.get("import_ms").copied().unwrap_or(0.0) / n,
        stats.get("entities").copied().unwrap_or(0.0) / n
    );
    println!("recognise: {:.0} ms/page", stats.get("recognise_ms").copied().unwrap_or(0.0) / n);
    println!("sheet number read: {}/{}", count(&|r| !r.number.is_empty()), pages.len());
    let mut disc: BTreeMap<String, usize> = BTreeMap::new();
    for p in &pages {
        *disc.entry(if p.rec.discipline.is_empty() { "?".into() } else { p.rec.discipline.clone() }).or_default() += 1;
    }
    println!("disciplines: {disc:?}");
    println!("level read: {}/{}", count(&|r| r.level.is_some()), pages.len());
    println!("scale read: {}/{}", count(&|r| r.scale > 0.0), pages.len());
    println!("grid found (x and y): {}/{}", count(&|r| !r.grid_x.is_empty() && !r.grid_y.is_empty()), pages.len());

    // Newer files replace older sheets of the same number (revision folders sort after the set).
    let mut by_number: BTreeMap<String, usize> = BTreeMap::new();
    let mut replaced = 0;
    for (i, p) in pages.iter().enumerate() {
        if p.rec.number.is_empty() {
            continue;
        }
        if let Some(old) = by_number.insert(p.rec.number.clone(), i)
            && pages.get(old).is_some_and(|o| o.file != p.file)
        {
            replaced += 1;
        }
    }
    let dup_in_file = pages.iter().filter(|p| !p.rec.number.is_empty()).count() - by_number.len() - replaced;
    println!("distinct sheet numbers: {} (replaced by a revision: {replaced}, repeated within a file: {dup_in_file})", by_number.len());

    let current: Vec<&Page> = by_number.values().filter_map(|&i| pages.get(i)).collect();
    let t = Instant::now();
    let map = sheet_reader::sheet_map_from(current.iter().map(|p| p.rec.clone()).collect());
    println!(
        "sheet map: {} sheets, grid {}x{}, aligned {} ({:.0} ms)",
        map.sheets.len(),
        map.grid_x.len(),
        map.grid_y.len(),
        map.sheets.iter().filter(|s| s.alignment.is_some()).count(),
        t.elapsed().as_secs_f64() * 1000.0
    );

    // The readers that read DXF sheets: the current plan sheets of the disciplines they read,
    // imported again one at a time and written as DXF.
    let t = Instant::now();
    let mut dxf: Vec<(String, Vec<u8>)> = Vec::new();
    let mut last: Option<(usize, Vec<u8>)> = None;
    for p in current.iter().filter(|p| ["A", "S", "M", "P", "FP", "E"].contains(&p.rec.discipline.as_str())) {
        if last.as_ref().is_none_or(|(f, _)| *f != p.file) {
            last = files.get(p.file).and_then(|f| std::fs::read(f).ok()).map(|b| (p.file, b));
        }
        let Some((_, bytes)) = &last else { continue };
        match import(bytes, p.page).and_then(|imp| cadaddy_io::write(&imp.drawing, "sheet.dxf").map_err(|e| e.to_string())) {
            Ok(b) => dxf.push((p.rec.number.clone(), b)),
            Err(e) => *errors.entry(format!("dxf: {}", class(&e))).or_default() += 1,
        }
    }
    drop(last);
    println!(
        "dxf written: {} ({:.0} ms, {:.0} MB)",
        dxf.len(),
        t.elapsed().as_secs_f64() * 1000.0,
        dxf.iter().map(|(_, b)| b.len()).sum::<usize>() as f64 / 1e6
    );

    let stage = |name: &str, t: Instant| println!("  {name}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let ceil = cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map);
    stage("ceilings", t);
    println!("ceilings: {} rooms, {} ceilings", ceil.rooms.len(), ceil.ceilings.len());
    // Structure, ducts and pipes read parsed drawings: the S, M and P sheets only.
    let parsed: Vec<(String, Drawing)> = dxf
        .iter()
        .filter(|(n, _)| map.sheets.iter().any(|s| &s.number == n && ["S", "M", "P"].contains(&s.discipline.as_str())))
        .filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.clone(), d)))
        .collect();
    let prefs: Vec<(String, &Drawing)> = parsed.iter().map(|(n, d)| (n.clone(), d)).collect();
    let t = Instant::now();
    let st = cadaddy_mepfix::struct_reader::read_structure(&map, &prefs);
    stage("structure", t);
    println!("structure: {} columns", st.columns.len());
    let t = Instant::now();
    let walls = cadaddy_mepfix::wall_reader::read_walls(&dxf, &map, &ceil, &st);
    stage("walls", t);
    println!("walls: {}", walls.walls.len());
    let t = Instant::now();
    let arch = cadaddy_mepfix::arch_reader::read_arch(&dxf, &map, &ceil);
    stage("arch", t);
    println!("doors: {}", arch.doors.len());
    let t = Instant::now();
    let sprinklers = cadaddy_mepfix::sprinkler_reader::read_sprinklers(&dxf, &map, &ceil);
    stage("sprinklers", t);
    println!("sprinkler heads: {}", sprinklers.heads.len());
    let t = Instant::now();
    let mut conduits = cadaddy_mepfix::conduit_reader::read_conduits(&dxf, &map);
    stage("conduits", t);
    println!("conduit runs: {}, homeruns: {}, panels: {}", conduits.conduits.len(), conduits.homeruns.len(), conduits.panels.len());
    // Level elevations aren't read yet: every level at 0 with structure 12' above (heights are
    // above the floor, so only the rule-based ones depend on it).
    cadaddy_mepfix::conduit_reader::assign_heights(&mut conduits, &|lvl| {
        Some(cadaddy_mepfix::conduit_reader::context_from_ceilings(&ceil, lvl, 0.0, 12.0))
    });
    let noted = conduits.conduits.iter().filter(|c| c.height_note.is_some()).count();
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    for c in &conduits.conduits {
        *sources.entry(format!("{:?}", c.height_source)).or_default() += 1;
    }
    println!("conduit heights: {noted} with a note; by source {sources:?}");
    let t = Instant::now();
    let (mut duct_runs, mut terminals, mut pipe_runs, mut risers) = (0, 0, 0, 0);
    for (n, d) in &prefs {
        let disc = map.sheets.iter().find(|s| &s.number == n).map(|s| s.discipline.clone()).unwrap_or_default();
        let sh = cadaddy_mepfix::truth::Sheet { number: n.clone(), discipline: disc.clone(), ..Default::default() };
        if disc == "M" {
            let net = cadaddy_mepfix::duct_reader::read_ducts(d, &sh);
            duct_runs += net.runs.len();
            terminals += net.terminals.len();
        } else if disc == "P" {
            let (r, rs) = cadaddy_mepfix::pipe_reader::read_pipes(d, &sh);
            pipe_runs += r.len();
            risers += rs.len();
        }
    }
    stage("ducts+pipes", t);
    println!("duct runs: {duct_runs}, terminals: {terminals}; pipe runs: {pipe_runs}, risers: {risers}");
    let numbers: Vec<String> = map.sheets.iter().map(|s| s.number.clone()).collect();
    let t = Instant::now();
    let (resolved, broken) = cadaddy_mepfix::callout_reader::resolve_callouts(&dxf, &numbers);
    stage("callouts", t);
    println!("callouts: {} resolved, {} unresolved", resolved.len(), broken.len());
    println!("errors: {errors:?}");
}
