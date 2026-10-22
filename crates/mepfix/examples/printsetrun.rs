//! Runs the print-set project's import (Q310) on a set that isn't ours to ship: files and
//! folders given in order (the set, then its revision folders), one entry per PDF page, each
//! classified from its print, later files replacing earlier sheets of the same number. It
//! prints only counts, rates and timings, never a file name, sheet number, title or text, so
//! its output can be shared when the prints can't.
//!
//! `cargo run --release -p cadaddy-mepfix --example printsetrun -- [--save FILE] <file or folder>…`
//!
//! `--save` writes the project JSON (it holds the set's sheet numbers and titles: keep it local).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use cadaddy_mepfix::printset::{self, SheetEntry, StageStatus};

/// The file read last (a merged set's pages are loaded one after another).
type Cache = std::cell::RefCell<Option<(String, std::rc::Rc<Vec<u8>>)>>;

fn load(paths: &BTreeMap<String, PathBuf>, cache: &Cache, e: &SheetEntry) -> Result<cadaddy_doc::Drawing, String> {
    let path = paths.get(&e.file).ok_or("not in the project")?;
    let cached = cache.borrow().as_ref().filter(|(f, _)| *f == e.file).map(|(_, b)| b.clone());
    let bytes = match cached {
        Some(b) => b,
        None => {
            let b = std::rc::Rc::new(std::fs::read(path).map_err(|err| err.to_string())?);
            *cache.borrow_mut() = Some((e.file.clone(), b.clone()));
            b
        }
    };
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "pdf" {
        let opts = cadaddy_io::pdf_read::PdfOptions { page: e.page.max(1), images: false, markups: false, layout: false, ..Default::default() };
        return cadaddy_io::pdf_read::read(&bytes, &opts).map(|i| i.drawing).map_err(|err| err.to_string());
    }
    cadaddy_io::read(&bytes, &e.file).map_err(|err| err.to_string())
}

fn main() {
    let mut save: Option<PathBuf> = None;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--save" => save = args.next().map(PathBuf::from),
            _ => inputs.push(PathBuf::from(a)),
        }
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for (i, p) in inputs.iter().enumerate() {
        if p.is_dir() {
            match printset::discover_sheets(p) {
                Ok(found) => files.extend(found.into_iter().map(|(n, path)| (format!("{i}/{n}"), path))),
                Err(e) => eprintln!("input {i}: {e}"),
            }
        } else {
            files.push((format!("{i}/{}", p.file_name().and_then(|n| n.to_str()).unwrap_or("")), p.clone()));
        }
    }
    let paths: BTreeMap<String, PathBuf> = files.iter().cloned().collect();
    let t = Instant::now();
    let mut project = printset::create_project_from_files("Project", std::path::Path::new("."), &files);
    println!("files: {}, entries (pages): {} ({:.1} s)", files.len(), project.sheets.len(), t.elapsed().as_secs_f64());

    let t = Instant::now();
    let cancel = AtomicBool::new(false);
    let cache: Cache = std::cell::RefCell::new(None);
    let rec = printset::run_classify_prints(&mut project, &|e| load(&paths, &cache, e), &cancel, &mut |_| {});
    println!("classify: {:?}, {}/{} pages in {:.1} s", rec.status, rec.sheets_processed, rec.sheets_total, t.elapsed().as_secs_f64());

    let all = &project.sheets;
    let current: Vec<&SheetEntry> = all.iter().filter(|s| s.superseded_by.is_empty()).collect();
    let failed: BTreeMap<String, usize> =
        all.iter().filter_map(|s| s.stage_results.get("classify")).filter(|r| r.status == StageStatus::Failed).fold(BTreeMap::new(), |mut m, r| {
            let class: String = r.error.clone().unwrap_or_default().chars().take(40).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
            *m.entry(class).or_default() += 1;
            m
        });
    let views = all.iter().filter(|s| s.recognised.as_ref().is_some_and(|r| !r.printed_number.is_empty())).count();
    println!(
        "sheets: {} (views of several-view pages: {views}), superseded by a later file: {}, current: {}",
        all.len(),
        all.len() - current.len(),
        current.len()
    );
    let read = |f: &dyn Fn(&SheetEntry) -> bool| current.iter().filter(|s| f(s)).count();
    println!("  number read: {}/{}", read(&|s| s.recognised.as_ref().is_some_and(|r| !r.number.is_empty())), current.len());
    let mut disc: BTreeMap<&str, usize> = BTreeMap::new();
    let mut types: BTreeMap<&str, usize> = BTreeMap::new();
    for s in &current {
        *disc.entry(s.discipline.as_str()).or_default() += 1;
        *types.entry(s.sheet_type.as_str()).or_default() += 1;
    }
    println!("  disciplines: {disc:?}");
    println!("  types: {types:?}");
    let plans: Vec<&&SheetEntry> = current.iter().filter(|s| ["plan", "enlarged_plan", "rcp"].contains(&s.sheet_type.as_str())).collect();
    let n = |f: &dyn Fn(&SheetEntry) -> bool| plans.iter().filter(|s| f(s)).count();
    println!(
        "  plans: {}, with a level: {}, with a scale: {}, with a grid: {}, aligned to the common grid: {}",
        plans.len(),
        n(&|s| !s.level.is_empty()),
        n(&|s| s.scale > 0.0),
        n(&|s| s.recognised.as_ref().is_some_and(|r| !r.grid_x.is_empty() && !r.grid_y.is_empty())),
        n(&|s| s.recognised.as_ref().is_some_and(|r| r.alignment.is_some()))
    );
    let mut per_level: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for s in &plans {
        let e = per_level.entry(if s.level.is_empty() { "?" } else { s.level.as_str() }).or_default();
        e.0 += 1;
        if s.recognised.as_ref().is_some_and(|r| r.alignment.is_some()) {
            e.1 += 1;
        }
    }
    println!("  plans per level (sheets, aligned): {per_level:?}");
    println!("  common grid: {}x{}", project.grid.grid_x.len(), project.grid.grid_y.len());
    let d = &project.dashboard;
    println!(
        "dashboard: classified {}/{}, superseded {}, open gaps {}, mean confidence {:.2}",
        d.classified_sheets, d.total_sheets, d.superseded_sheets, d.open_gaps, d.mean_confidence
    );
    println!("errors: {failed:?}");
    if let Some(p) = save
        && let Err(e) = printset::save_project(&project, &p)
    {
        eprintln!("save: {e}");
    }
}
