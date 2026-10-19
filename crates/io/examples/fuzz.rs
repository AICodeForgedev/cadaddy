//! A mutation fuzzer for the file readers that runs on stable Rust (`cargo xtask fuzz`; the
//! coverage-guided cargo-fuzz targets live in `fuzz/` for nightly):
//! `cargo run --release -p cadaddy-io --example fuzz -- <dxf|dwg|pdf|svg|image|cdy|all> [seconds] [out dir]`.
//!
//! Seeds: a drawing of every object kind (the .cdy tests' DXF) written in each format, the
//! frozen .cdy sample, and any files under `$CADADDY_CORPUS` with the target's extension. Each
//! round takes a seed, mutates it (bit flips, interesting bytes, inserted, deleted, duplicated
//! and spliced runs, truncation, and for text formats numbers swapped for extreme ones) and reads
//! it on a worker thread: a panic is a crash and an input still running after
//! [`HANG_SECS`] is a hang; either is saved under the output directory and reported.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// An input still being read after this long is a hang.
const HANG_SECS: u64 = 10;
/// Largest mutated input.
const MAX_INPUT: usize = 4 << 20;

const TARGETS: [&str; 6] = ["dxf", "dwg", "pdf", "svg", "image", "cdy"];

/// Read `bytes` as `target`; the result doesn't matter, only that it returns.
fn run_target(target: &str, bytes: &[u8]) {
    match target {
        "dxf" => {
            let _ = cadaddy_io::read_dxf(bytes);
        }
        "dwg" => {
            if let Ok((dxf, _)) = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT) {
                let _ = cadaddy_io::read_dxf(&dxf);
            }
        }
        "pdf" => {
            let _ = cadaddy_io::pdf_read::read(bytes, &cadaddy_io::pdf_read::PdfOptions::default());
        }
        "svg" => {
            let _ = cadaddy_io::read(bytes, "fuzz.svg");
        }
        "image" => {
            let _ = cadaddy_render::images::decode(bytes);
        }
        "cdy" => {
            let _ = cadaddy_io::cdy::read_cdy(bytes);
        }
        _ => {}
    }
}

/// xorshift64*: deterministic for a seed, good enough to pick mutations.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const INTERESTING_BYTES: [u8; 8] = [0, 1, 0x7f, 0x80, 0xff, b'\n', b'0', b'-'];
const INTERESTING_NUMBERS: [&str; 10] = ["0", "-1", "1e308", "-1e308", "nan", "inf", "4294967295", "2147483648", "99999999999999999999", "1e-320"];

fn mutate(rng: &mut Rng, input: &[u8], seeds: &[Vec<u8>], text: bool) -> Vec<u8> {
    let mut b = input.to_vec();
    for _ in 0..=rng.below(4) {
        let len = b.len().max(1);
        match rng.below(if text { 9 } else { 8 }) {
            0 => {
                if let Some(x) = b.get_mut(rng.below(len)) {
                    *x ^= 1 << rng.below(8);
                }
            }
            1 => {
                let v = INTERESTING_BYTES[rng.below(INTERESTING_BYTES.len())];
                if let Some(x) = b.get_mut(rng.below(len)) {
                    *x = v;
                }
            }
            2 => {
                let at = rng.below(b.len() + 1);
                let n = 1 + rng.below(16);
                let ins: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                b.splice(at..at, ins);
            }
            3 => {
                let at = rng.below(len);
                let n = rng.below(64).min(b.len().saturating_sub(at));
                b.drain(at..at + n);
            }
            4 => {
                // Duplicate a run (repeated records, deep nesting).
                let at = rng.below(len);
                let n = (1 + rng.below(256)).min(b.len().saturating_sub(at));
                let run: Vec<u8> = b.get(at..at + n).map(<[u8]>::to_vec).unwrap_or_default();
                for _ in 0..1 + rng.below(8) {
                    b.splice(at..at, run.iter().copied());
                }
            }
            5 => {
                // Splice a piece of another seed.
                let other = &seeds[rng.below(seeds.len())];
                let at = rng.below(other.len().max(1));
                let n = rng.below(512).min(other.len().saturating_sub(at));
                let dst = rng.below(b.len() + 1);
                b.splice(dst..dst, other.get(at..at + n).unwrap_or_default().iter().copied());
            }
            6 => b.truncate(rng.below(len)),
            7 => {
                // Overwrite a 4-byte run with an extreme little-endian integer (lengths, counts).
                let at = rng.below(len);
                let v = [u32::MAX, 0x7fff_ffff, 0x8000_0000, 0, 1, 0xffff][rng.below(6)];
                for (k, byte) in v.to_le_bytes().into_iter().enumerate() {
                    if let Some(x) = b.get_mut(at + k) {
                        *x = byte;
                    }
                }
            }
            _ => {
                // Text formats: a number replaced by an extreme one.
                let s = String::from_utf8_lossy(&b).into_owned();
                let starts: Vec<usize> = s
                    .char_indices()
                    .filter(|(i, c)| c.is_ascii_digit() && s[..*i].chars().last().is_none_or(|p| !p.is_ascii_digit() && p != '.'))
                    .map(|(i, _)| i)
                    .collect();
                if let Some(&at) = starts.get(rng.below(starts.len())) {
                    let end = s[at..].find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(s.len(), |e| at + e);
                    let mut t = s.clone();
                    t.replace_range(at..end, INTERESTING_NUMBERS[rng.below(INTERESTING_NUMBERS.len())]);
                    b = t.into_bytes();
                }
            }
        }
    }
    b.truncate(MAX_INPUT);
    b
}

/// The every-kind drawing written in each format, the frozen .cdy sample, and `$CADADDY_CORPUS`.
fn seeds(target: &str) -> Vec<Vec<u8>> {
    // The fixture is written one `code value` pair per line; DXF has them on two lines.
    let src: String = include_str!("../src/cdy/every_kind.dxf.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (code, value) = l.split_once(' ').unwrap_or((l, ""));
            format!("{code}\n{value}\n")
        })
        .collect();
    let mut out: Vec<Vec<u8>> = Vec::new();
    let d = cadaddy_io::read_dxf(src.as_bytes()).ok();
    match target {
        "dxf" => {
            out.push(src.as_bytes().to_vec());
            if let Some(d) = &d {
                out.extend(
                    cadaddy_io::write_with(d, "s.dxf", &cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2000), binary: true }).ok(),
                );
                out.extend(
                    cadaddy_io::write_with(d, "s.dxf", &cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2018), binary: false }).ok(),
                );
            }
        }
        "dwg" => out.extend(d.as_ref().and_then(|d| cadaddy_io::write(d, "s.dwg").ok())),
        "pdf" => out.extend(d.as_ref().and_then(|d| cadaddy_io::write(d, "s.pdf").ok())),
        "svg" => out.extend(d.as_ref().and_then(|d| cadaddy_io::write(d, "s.svg").ok())),
        "image" => out.extend(d.as_ref().and_then(|d| cadaddy_io::write(d, "s.png").ok())),
        "cdy" => {
            out.push(include_bytes!("../src/cdy/v1_sample.cdy").to_vec());
            out.extend(d.as_ref().and_then(|d| cadaddy_io::cdy::write_cdy(d).ok()));
        }
        _ => {}
    }
    if let Ok(dir) = std::env::var("CADADDY_CORPUS") {
        let exts: &[&str] = match target {
            "image" => &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff"],
            "dxf" => &["dxf"],
            "dwg" => &["dwg"],
            "pdf" => &["pdf"],
            "svg" => &["svg", "svgz"],
            _ => &["cdy"],
        };
        let mut stack = vec![PathBuf::from(dir)];
        while let Some(p) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&p) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
                    && std::fs::metadata(&path).is_ok_and(|m| m.len() < MAX_INPUT as u64)
                    && let Ok(b) = std::fs::read(&path)
                {
                    out.push(b);
                }
            }
        }
    }
    out
}

fn hash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x100_0000_01b3))
}

/// Fuzz `target` for `secs`; returns the number of crashes and hangs found.
fn fuzz(target: &'static str, secs: u64, out: &Path, seed: u64) -> usize {
    let seeds = seeds(target);
    if seeds.is_empty() {
        eprintln!("{target}: no seeds");
        return 0;
    }
    let text = matches!(target, "dxf" | "svg");
    let mut rng = Rng(seed | 1);
    let (mut runs, mut found) = (0u64, 0usize);
    let start = Instant::now();
    // A worker reads inputs; a hung one is abandoned and replaced.
    let spawn = || {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let (done_tx, done_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            for input in rx {
                let r = catch_unwind(AssertUnwindSafe(|| run_target(target, &input))).map_err(|p| {
                    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
                });
                if done_tx.send(r).is_err() {
                    break;
                }
            }
        });
        (tx, done_rx)
    };
    let (mut tx, mut done) = spawn();
    let keep_last = std::env::var_os("FUZZ_KEEP_LAST").is_some();
    while start.elapsed() < Duration::from_secs(secs) {
        let base = &seeds[rng.below(seeds.len())];
        let input = mutate(&mut rng, base, &seeds, text);
        let save = |kind: &str, msg: &str| {
            let dir = out.join(target);
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join(format!("{kind}-{:016x}", hash(&input)));
            let _ = std::fs::write(&file, &input);
            println!("{target}: {kind} ({msg}) → {}", file.display());
        };
        // An allocation failure aborts the process (it can't be caught): with FUZZ_KEEP_LAST
        // set, each input is written first so the one that aborted is left behind.
        if keep_last {
            let dir = out.join(target);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("last-input"), &input);
        }
        if tx.send(input.clone()).is_err() {
            (tx, done) = spawn();
            continue;
        }
        match done.recv_timeout(Duration::from_secs(HANG_SECS)) {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => {
                found += 1;
                save("crash", &msg);
            }
            Err(_) => {
                found += 1;
                save("hang", &format!("over {HANG_SECS}s"));
                (tx, done) = spawn();
            }
        }
        runs += 1;
    }
    println!("{target}: {runs} inputs in {secs}s, {found} crash(es)/hang(s), {} seed(s)", seeds.len());
    found
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `repro <target> <file>`: read one saved input (the default panic hook stays, for a backtrace).
    if args.first().map(String::as_str) == Some("repro") {
        let (Some(t), Some(f)) = (args.get(1), args.get(2)) else {
            eprintln!("repro <target> <file>");
            std::process::exit(2);
        };
        let bytes = std::fs::read(f).unwrap_or_default();
        run_target(t, &bytes);
        println!("{t}: {f} read without crashing");
        return;
    }
    let target = args.first().map_or("all", String::as_str);
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let out = PathBuf::from(args.get(2).map_or("target/fuzz-findings", String::as_str));
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    // Panics are reported by the fuzzer, not printed by the default hook.
    std::panic::set_hook(Box::new(|_| {}));
    let targets: Vec<&'static str> = if target == "all" { TARGETS.to_vec() } else { TARGETS.iter().copied().filter(|t| *t == target).collect() };
    if targets.is_empty() {
        eprintln!("unknown target {target}: {}", TARGETS.join(", "));
        std::process::exit(2);
    }
    let found: usize = targets.iter().map(|t| fuzz(t, secs, &out, seed)).sum();
    std::process::exit(if found > 0 { 1 } else { 0 });
}
