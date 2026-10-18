//! Time reading and writing a DWG file stage by stage:
//! `cargo run --release -p cadaddy-io --example dwgbench -- file.dwg [more.dwg…]`.

use std::time::Instant;

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("{path}: cannot read");
            continue;
        };
        println!("{path}: {:.1} MB", bytes.len() as f64 / 1e6);
        let t = Instant::now();
        let dxf = match cadaddy_dwg::dwg_to_dxf_noted(&bytes, cadaddy_dwg::Limits::DEFAULT) {
            Ok((dxf, _)) => dxf,
            Err(e) => {
                println!("  DWG→DXF failed after {:.2}s: {e}", t.elapsed().as_secs_f64());
                continue;
            }
        };
        println!("  DWG→DXF (acadrust read + DXF write): {:.2}s, {:.1} MB of DXF", t.elapsed().as_secs_f64(), dxf.len() as f64 / 1e6);
        let t = Instant::now();
        let d = match cadaddy_io::read_dxf(&dxf) {
            Ok(d) => d,
            Err(e) => {
                println!("  DXF read failed: {e}");
                continue;
            }
        };
        println!("  DXF read (ours): {:.2}s, {} entities", t.elapsed().as_secs_f64(), d.entity_count());
        let t = Instant::now();
        let out = cadaddy_io::write(&d, "out.dxf").map(|b| b.len());
        println!("  DXF write (ours): {:.2}s ({out:?} bytes)", t.elapsed().as_secs_f64());
        let t = Instant::now();
        match cadaddy_io::write(&d, "out.dwg") {
            Ok(b) => {
                println!("  DWG write (ours + acadrust DXF read + DWG write): {:.2}s, {:.1} MB", t.elapsed().as_secs_f64(), b.len() as f64 / 1e6)
            }
            Err(e) => println!("  DWG write failed after {:.2}s: {e}", t.elapsed().as_secs_f64()),
        }
    }
}
