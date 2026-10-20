//! Reads damaged and arbitrary bytes through the whole codec and checks nothing panics.
//!
//! `cargo run -p cadaddy-dwgcodec --example fuzz_read -- [ROUNDS] [FILE.dwg ...]`
//!
//! Each round mutates one of the given files (flipped bytes, cut short, a stretch overwritten,
//! or bytes behind a DWG id when no files are given), reads it, and on success decodes the
//! header variables and classes and writes it again. A panic ends the run with the round's
//! seed so it can be replayed.

use std::panic::{AssertUnwindSafe, catch_unwind};

fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn mutate(base: &[u8], seed: &mut u64) -> Vec<u8> {
    let mut b = base.to_vec();
    if b.is_empty() {
        let ids: [&[u8]; 5] = [b"AC1015", b"AC1018", b"AC1024", b"AC1027", b"AC1032"];
        b.extend_from_slice(ids[(next(seed) % 5) as usize]);
        b.extend((0..next(seed) % 70_000).map(|_| next(seed) as u8));
        return b;
    }
    match next(seed) % 4 {
        0 => {
            for _ in 0..1 + next(seed) % 16 {
                let at = (next(seed) as usize) % b.len();
                b[at] ^= (next(seed) as u8) | 1;
            }
        }
        1 => b.truncate((next(seed) as usize) % b.len()),
        2 => {
            let at = (next(seed) as usize) % b.len();
            let n = ((next(seed) % 256) as usize).min(b.len() - at);
            for x in &mut b[at..at + n] {
                *x = next(seed) as u8;
            }
        }
        _ => {
            // Keep the first 0x100 bytes (header CRCs intact) and scramble after them.
            for _ in 0..1 + next(seed) % 8 {
                if b.len() > 0x100 {
                    let at = 0x100 + (next(seed) as usize) % (b.len() - 0x100);
                    b[at] = next(seed) as u8;
                }
            }
        }
    }
    b
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rounds: u64 = args.next().and_then(|r| r.parse().ok()).unwrap_or(10_000);
    let files: Vec<Vec<u8>> = args.filter_map(|p| std::fs::read(p).ok()).collect();
    let (mut read_ok, mut read_err) = (0u64, 0u64);
    for round in 0..rounds {
        let mut seed = round.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let base: &[u8] = if files.is_empty() { &[] } else { &files[(round as usize) % files.len()] };
        let bytes = mutate(base, &mut seed);
        let outcome = catch_unwind(AssertUnwindSafe(|| match cadaddy_dwgcodec::read(&bytes) {
            Ok(d) => {
                let _ = d.header_vars().map(|h| h.encode());
                let _ = d.classes().map(|c| c.encode());
                let _ = cadaddy_dwgcodec::write(&d);
                true
            }
            Err(_) => false,
        }));
        match outcome {
            Ok(true) => read_ok += 1,
            Ok(false) => read_err += 1,
            Err(_) => {
                eprintln!("PANIC in round {round}");
                std::process::exit(1);
            }
        }
    }
    println!("{rounds} rounds: {read_ok} read, {read_err} refused, no panics");
}
