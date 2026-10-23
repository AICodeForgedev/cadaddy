//! `cargo xtask pdfstats FILE.pdf` — print a PDF import statistics table.

use std::path::Path;

pub fn run(_root: &Path, args: &[&str]) -> Result<(), String> {
    let files: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
    if files.is_empty() {
        return Err("usage: cargo xtask pdfstats FILE.pdf [FILE2.pdf ...]".into());
    }

    for file in &files {
        let path = Path::new(file);
        if !path.exists() {
            eprintln!("warning: {file} does not exist, skipping");
            continue;
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{file}: {e}"))?;
        let pages = cadaddy_io::pdf_read::page_count(&bytes).unwrap_or(0);

        println!("=== {file} ({pages} page(s)) ===");
        println!();

        let opts = cadaddy_io::pdf_read::PdfOptions::default();
        let r = cadaddy_io::pdf_read::read(&bytes, &opts).map_err(|e| format!("{file}: {e}"))?;

        // count entity types in model space
        let mut lines = 0usize;
        let mut polylines = 0usize;
        let mut circles = 0usize;
        let mut texts = 0usize;
        let mut wipeouts = 0usize;
        let mut hatches = 0usize;
        let mut images = 0usize;
        let mut inserts = 0usize;
        let mut other = 0usize;
        for e in r.drawing.model.iter() {
            use cadaddy_engine::doc::EntityKind;
            match &e.kind {
                EntityKind::Line(_) => lines += 1,
                EntityKind::LwPolyline(_) => polylines += 1,
                EntityKind::Circle(_) => circles += 1,
                EntityKind::Text(_) | EntityKind::MText(_) => texts += 1,
                EntityKind::Wipeout(_) => wipeouts += 1,
                EntityKind::Hatch(_) => hatches += 1,
                EntityKind::Image(_) => images += 1,
                EntityKind::Insert(_) => inserts += 1,
                _ => other += 1,
            }
        }
        let total = r.drawing.model.len();
        let blocks = r.drawing.blocks.len();
        let layers: Vec<&str> = r.drawing.layers.iter().map(|l| l.name.as_str()).collect();

        println!("  Entity counts:");
        println!("    Total:     {total}");
        println!("    Lines:     {lines}");
        println!("    Polylines: {polylines}");
        println!("    Circles:   {circles}");
        println!("    Text:      {texts}");
        println!("    Wipeout:   {wipeouts}");
        println!("    Raster:    {images}");
        println!("    Hatch:     {hatches}");
        println!("    Blocks:    {blocks}");
        println!("    Inserts:   {inserts}");
        println!("    Other:     {other}");
        println!();
        println!("  Scale: {} ({})", r.scale, r.scale_source);
        println!("  DO NOT SCALE: {}", r.do_not_scale);
        println!("  Needs registration: {}", r.needs_registration);
        println!("  Sheet: {:.1} x {:.1} in", r.sheet.0, r.sheet.1);
        println!("  Layers ({}):", layers.len());
        for l in &layers {
            println!("    {l}");
        }
        println!();
        println!("  Import stats:");
        for (k, v) in &r.stats {
            println!("    {k}: {v}");
        }
        if !r.notes.is_empty() {
            println!();
            println!("  Notes:");
            for n in &r.notes {
                println!("    {n}");
            }
        }
        println!();
    }
    Ok(())
}
