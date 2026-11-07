//! Damaged DWG files (Q183): a file cut short, or with bytes overwritten, opens with what can be
//! read — the objects before the damage — and says so in its read notes, never panicking. Before
//! the salvage a DWG file cut short opened as nothing at all (its page and section maps are at the
//! end).

use cadaddy_doc::{Drawing, EntityKind};

/// A made-up drawing of `n` lines, line `i` starting at x = i, saved as DWG `version`.
fn dwg(n: usize, version: &str) -> Vec<u8> {
    let mut src = String::from("0\nSECTION\n2\nENTITIES\n");
    for i in 0..n {
        src += &format!("0\nLINE\n8\nWALLS\n10\n{i}\n20\n0\n30\n0\n11\n{i}\n21\n7\n31\n0\n");
    }
    src += "0\nENDSEC\n0\nEOF\n";
    let mut d = cadaddy_io::read(src.as_bytes(), "made-up.dxf").unwrap();
    d.header.set_str("ACADVER", version);
    cadaddy_io::write(&d, "made-up.dwg").unwrap()
}

/// The x of each line read (its index in the made-up drawing).
fn lines(d: &Drawing) -> Vec<usize> {
    let mut v: Vec<usize> = d.model.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some(l.a.x as usize) } else { None }).collect();
    v.sort_unstable();
    v
}

/// Like [`lines`], also asserting each line read is the made-up one: starts at (i, 0), ends at (i, 7).
fn checked_lines(d: &Drawing, what: &str) -> Vec<usize> {
    let mut v = Vec::new();
    for e in d.model.iter() {
        if let EntityKind::Line(l) = &e.kind {
            assert!(l.a.y == 0.0 && l.b.x == l.a.x && l.b.y == 7.0, "{what}: a misread line {:?} -> {:?}", l.a, l.b);
            v.push(l.a.x as usize);
        }
    }
    v.sort_unstable();
    v
}

/// Where the objects section's data page starts and how long its payload is: the last big data
/// page, found from the page headers (each XOR-masked with the page's own offset, per the
/// specification's section on 2004-and-later page headers).
fn objects_page(b: &[u8]) -> (usize, usize) {
    let word = |p: usize, mask: u32| b.get(p..p + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) ^ mask);
    let (mut pos, mut best) = (0x100usize, None);
    while pos + 32 <= b.len() {
        let mask = 0x4164_536bu32 ^ (pos as u32);
        if word(pos, mask) == Some(0x4163_043b)
            && let Some(csz) = word(pos + 8, mask).map(|v| v as usize)
            && csz <= 0x2_0000
        {
            if csz > 5000 {
                best = Some((pos, csz));
            }
            pos = (pos + 32 + csz + 0x1f) & !0x1f;
            continue;
        }
        pos += 4;
    }
    best.expect("a big data page")
}

const VERSIONS: [&str; 3] = ["AC1018", "AC1024", "AC1032"];

#[test]
fn a_dwg_cut_short_keeps_the_objects_before_the_cut() {
    for v in VERSIONS {
        let full = dwg(2000, v);
        assert_eq!(lines(&cadaddy_io::read(&full, "a.dwg").unwrap()).len(), 2000, "{v}");
        for frac in [0.97, 0.9, 0.8] {
            let cut = &full[..(full.len() as f64 * frac) as usize];
            let d = cadaddy_io::read(cut, "a.dwg").unwrap_or_else(|e| panic!("{v} cut to {frac}: {e}"));
            let got = checked_lines(&d, &format!("{v} cut to {frac}"));
            // Most of what lies before the cut is kept (the lines are the bulk of the file).
            let floor = (0.8 * frac * 2000.0) as usize;
            assert!(got.len() >= floor, "{v} cut to {frac}: {} lines, expected at least {floor}", got.len());
            // What was read is what came before the cut: the first lines, in order, with none
            // missing in between.
            let last = *got.last().unwrap();
            assert!(got.len() * 100 >= (last + 1) * 95, "{v} cut to {frac}: {} lines of the first {}", got.len(), last + 1);
            assert!(d.read_notes.iter().any(|n| n.contains("damaged")), "{v} cut to {frac}: {:?}", d.read_notes);
        }
    }
}

#[test]
fn overwritten_bytes_and_any_cut_never_panic() {
    for v in VERSIONS {
        let full = dwg(400, v);
        // Cut anywhere.
        for k in 1..60 {
            let at = full.len() * k / 60;
            let _ = cadaddy_io::read(&full[..at], "a.dwg");
        }
        // Sixteen bytes overwritten anywhere.
        let mut x = 0x9e37_79b9u32;
        for k in 1..40 {
            let mut b = full.clone();
            let at = (full.len() * k / 40).min(full.len() - 16);
            for i in 0..16 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                b[at + i] ^= x as u8 | 1;
            }
            if let Ok(d) = cadaddy_io::read(&b, "a.dwg") {
                // Whatever it reads are the made-up lines, none invented.
                assert!(lines(&d).iter().all(|i| *i < 400), "{v} at {k}");
            }
        }
    }
}

/// Sixteen bytes overwritten past the header and classes: the file still opens and keeps the
/// objects before the damage, each one the made-up line (nothing misread).
#[test]
fn byte_flipped_files_keep_the_objects_before_the_damage() {
    for v in VERSIONS {
        let full = dwg(400, v);
        let (page, payload) = objects_page(&full);
        let mut x = 0x9e37_79b9u32;
        let mut in_objects = 0;
        for k in 1..40 {
            let at = (full.len() * k / 40).min(full.len() - 16);
            let mut b = full.clone();
            for i in 0..16 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                b[at + i] ^= x as u8 | 1;
            }
            let what = format!("{v} flipped at {at} of {}", full.len());
            if at < page {
                // The header, classes or small sections: only never panicking and never inventing
                // is promised.
                if let Ok(d) = cadaddy_io::read(&b, "a.dwg") {
                    assert!(checked_lines(&d, &what).iter().all(|i| *i < 400), "{what}");
                }
                continue;
            }
            let d = cadaddy_io::read(&b, "a.dwg").unwrap_or_else(|e| panic!("{what}: {e}"));
            let got = checked_lines(&d, &what);
            if at < page + 32 + payload {
                // In the objects: the lines start about a quarter of the way into the payload and
                // run to about three quarters. The lines before the damage, at the least:
                in_objects += 1;
                let frac = (at - page - 32) as f64 / payload as f64;
                let before = (400.0 * ((frac - 0.30) / 0.45).clamp(0.0, 1.0)) as usize;
                assert!(got.len() * 10 >= before * 9, "{what}: {} lines kept of at least {before} before the damage", got.len());
                // What was kept is the first lines, none missing in between.
                assert!(got.iter().enumerate().all(|(i, l)| i == *l), "{what}: not a run from the first line");
                assert!(d.read_notes.iter().any(|n| n.contains("damaged")), "{what}: {:?}", d.read_notes);
            }
            if at >= page + 32 + payload && at * 4 >= full.len() * 3 {
                // Past the objects, in the last quarter (object map, section and page maps): nearly everything.
                assert!(got.len() >= 300, "{what}: {} of 400 lines", got.len());
            }
        }
        assert!(in_objects >= 8, "{v}: only {in_objects} flips landed in the objects");
    }
}

#[test]
fn salvaging_a_sound_file_keeps_everything() {
    for v in VERSIONS {
        let full = dwg(1000, v);
        let s = cadaddy_dwg::salvage::salvage(&full).unwrap();
        assert_eq!(lines(&cadaddy_io::read(&s.bytes, "a.dwg").unwrap()).len(), 1000, "{v}");
    }
}
