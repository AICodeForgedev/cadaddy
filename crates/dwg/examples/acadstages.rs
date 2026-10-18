//! Time acadrust's own stages on a DWG file (reading the DWG, writing DXF, reading that DXF back,
//! writing a DWG): `cargo run --release -p cadaddy-dwg --example acadstages -- file.dwg`.

use std::io::Cursor;
use std::time::Instant;

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        println!("{path}: {:.1} MB", bytes.len() as f64 / 1e6);
        let t = Instant::now();
        let doc = match acadrust::DwgReader::from_stream_with_options(Cursor::new(&bytes[..]), acadrust::DwgReadOptions::failsafe()).read_with_stats()
        {
            Ok(o) => o.document,
            Err(e) => {
                println!("  DWG read failed: {e}");
                continue;
            }
        };
        println!("  acadrust DWG read: {:.2}s", t.elapsed().as_secs_f64());
        let t = Instant::now();
        let mut dxf = Vec::new();
        let r = acadrust::DxfWriter::new(&doc).write_to_writer(&mut dxf);
        println!("  acadrust DXF write: {:.2}s ({:?}), {:.1} MB", t.elapsed().as_secs_f64(), r.err(), dxf.len() as f64 / 1e6);
        let t = Instant::now();
        let back = acadrust::DxfReader::from_reader(Cursor::new(dxf)).and_then(|r| r.read());
        println!("  acadrust DXF read: {:.2}s ({})", t.elapsed().as_secs_f64(), if back.is_ok() { "ok" } else { "failed" });
        if let Ok(doc) = back {
            let t = Instant::now();
            let w = acadrust::DwgWriter::write_to_vec(&doc);
            println!(
                "  acadrust DWG write: {:.2}s ({})",
                t.elapsed().as_secs_f64(),
                w.map(|v| format!("{:.1} MB", v.len() as f64 / 1e6)).unwrap_or_else(|e| e.to_string())
            );
        }
    }
}
