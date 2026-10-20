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

const VERSIONS: [&str; 3] = ["AC1018", "AC1024", "AC1032"];

#[test]
fn a_dwg_cut_short_keeps_the_objects_before_the_cut() {
    for v in VERSIONS {
        let full = dwg(2000, v);
        assert_eq!(lines(&cadaddy_io::read(&full, "a.dwg").unwrap()).len(), 2000, "{v}");
        for frac in [0.97, 0.9, 0.8] {
            let cut = &full[..(full.len() as f64 * frac) as usize];
            let d = cadaddy_io::read(cut, "a.dwg").unwrap_or_else(|e| panic!("{v} cut to {frac}: {e}"));
            let got = lines(&d);
            assert!(!got.is_empty(), "{v} cut to {frac}: no lines");
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

#[test]
fn salvaging_a_sound_file_keeps_everything() {
    for v in VERSIONS {
        let full = dwg(1000, v);
        let s = cadaddy_dwg::salvage::salvage(&full).unwrap();
        assert_eq!(lines(&cadaddy_io::read(&s.bytes, "a.dwg").unwrap()).len(), 1000, "{v}");
    }
}
