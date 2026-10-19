//! A short deterministic burst of the mutation fuzzer (`cargo xtask fuzz` runs it for longer):
//! damaged DXF (ASCII and binary), SVG, PDF and .cdy files read without a panic. DWG is left
//! out: its reader can abort on a hostile section size (an allocation failure, not a panic;
//! see the fuzz task's notes).

use std::panic::{AssertUnwindSafe, catch_unwind};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..=rng.below(3) {
        let len = b.len().max(1);
        match rng.below(5) {
            0 => {
                if let Some(x) = b.get_mut(rng.below(len)) {
                    *x ^= 1 << rng.below(8);
                }
            }
            1 => {
                let at = rng.below(len);
                let n = rng.below(64).min(b.len().saturating_sub(at));
                b.drain(at..at + n);
            }
            2 => {
                let at = rng.below(b.len() + 1);
                let v = [0u8, 0xff, b'9', b'-', b'\n'][rng.below(5)];
                b.insert(at, v);
            }
            3 => b.truncate(rng.below(len)),
            _ => {
                let at = rng.below(len);
                for (k, byte) in u32::MAX.to_le_bytes().into_iter().enumerate() {
                    if let Some(x) = b.get_mut(at + k) {
                        *x = byte;
                    }
                }
            }
        }
    }
    b
}

fn sample() -> cadaddy_doc::Drawing {
    let src: String = include_str!("../src/cdy/every_kind.dxf.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (code, value) = l.split_once(' ').unwrap_or((l, ""));
            format!("{code}\n{value}\n")
        })
        .collect();
    cadaddy_io::read_dxf(src.as_bytes()).unwrap()
}

fn burst(name: &str, seed: &[u8], rounds: usize, read: impl Fn(&[u8])) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..rounds {
        let input = mutate(&mut rng, seed);
        let r = catch_unwind(AssertUnwindSafe(|| read(&input)));
        assert!(r.is_ok(), "{name}: input {i} panicked");
    }
}

#[test]
fn damaged_files_read_without_panicking() {
    let d = sample();
    let ascii = cadaddy_io::write(&d, "s.dxf").unwrap();
    let binary = cadaddy_io::write_with(&d, "s.dxf", &cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2000), binary: true }).unwrap();
    let svg = cadaddy_io::write(&d, "s.svg").unwrap();
    let pdf = cadaddy_io::write(&d, "s.pdf").unwrap();
    let cdy = cadaddy_io::cdy::write_cdy(&d).unwrap();
    burst("dxf", &ascii, 400, |b| drop(cadaddy_io::read_dxf(b)));
    burst("binary dxf", &binary, 400, |b| drop(cadaddy_io::read_dxf(b)));
    burst("svg", &svg, 300, |b| drop(cadaddy_io::read(b, "x.svg")));
    burst("pdf", &pdf, 200, |b| drop(cadaddy_io::pdf_read::read(b, &cadaddy_io::pdf_read::PdfOptions::default())));
    burst("cdy", &cdy, 400, |b| drop(cadaddy_io::cdy::read_cdy(b)));
}
