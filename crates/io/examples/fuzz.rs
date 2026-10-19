//! A mutation fuzzer for the file readers that runs on stable Rust (`cargo xtask fuzz`; the
//! coverage-guided cargo-fuzz targets live in `fuzz/` for nightly):
//! `cargo run --release -p cadaddy-io --example fuzz -- <target|all> [seconds] [out dir]`, targets
//! dxf dwg pdf svg image cdy shx las pts xyz cdywalk markups stl obj gltf (the engine's own readers
//! — xlsx, zip, LISP — are in `cadaddy-engine`'s `fuzz` example).
//!
//! Seeds: a drawing of every object kind (the .cdy tests' DXF) written in each format, the
//! frozen .cdy sample, small made-up fonts, clouds, packages and annotated PDFs, and any files
//! under `$CADADDY_CORPUS` with the target's extension. The mutations and the crash/hang
//! detection are in `common/fuzz_core.rs`.

#[path = "common/fuzz_core.rs"]
mod fuzz_core;

const TARGETS: [&str; 15] = ["dxf", "dwg", "pdf", "svg", "image", "cdy", "shx", "las", "pts", "xyz", "cdywalk", "markups", "stl", "obj", "gltf"];

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
        "shx" => {
            // Every glyph a drawing could ask for, both orientations.
            if let Some(f) = cadaddy_fonts::shx::ShxFont::parse(bytes) {
                for code in (0..600u32).chain([0x2126, 0x8140, 0xFFFF]) {
                    let _ = f.glyph(code, false);
                    let _ = f.glyph(code, true);
                }
            }
        }
        "las" => {
            let _ = cadaddy_io::pointcloud_io::read(bytes, "fuzz.las");
        }
        "pts" => {
            let _ = cadaddy_io::pointcloud_io::read(bytes, "fuzz.pts");
        }
        "xyz" => {
            let _ = cadaddy_io::pointcloud_io::read(bytes, "fuzz.xyz");
        }
        "cdywalk" => {
            let _ = cadaddy_io::walkpkg::read(bytes);
        }
        "markups" => {
            let _ = cadaddy_io::pdf_markups::read(bytes, 1);
        }
        "stl" => {
            let _ = cadaddy_io::mesh3d_io::read_stl(bytes);
        }
        "obj" => {
            let _ = cadaddy_io::mesh3d_io::read_obj(bytes, "fuzz.obj");
        }
        "gltf" => {
            let _ = cadaddy_io::mesh3d_io::read_gltf(bytes, "fuzz.glb");
            let _ = cadaddy_io::mesh3d_io::read_gltf(bytes, "fuzz.gltf");
        }
        _ => {}
    }
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
        "shx" => {
            let shapes: [(u16, &str, &[u8]); 4] = [
                (65, "A", &[2, 0x14, 1, 0x44, 0x4c, 2, 0x18, 1, 0x40, 0]),
                (66, "B", &[1, 0x10, 0x12, 8, 3, 0xfe, 0x0c, 0]),
                (10, "LF", &[2, 8, 0, 0xf6, 14, 0]),
                (200, "ARC", &[1, 10, 2, 0x12, 11, 0, 0x20, 0, 2, 0x10, 0]),
            ];
            out.push(cadaddy_fonts::shx::compile_shapes(6, 2, &shapes));
            out.push(cadaddy_fonts::shx::compile_bigfont(6, &[(0x81, 0x9f)], &shapes));
        }
        "pts" => out.push(b"3\n1.5 2.5 3.5 -204 255 0 0\n2 3 4 10 0 255 0\n-1e3 0 7 0 0 0 255\n".to_vec()),
        "xyz" | "las" => {
            let text = b"1.5 2.5 3.5 255 0 0\n2 3 4 0 255 0\n-1000 0 7 0 0 255\n0 0 0\n".to_vec();
            if target == "las" {
                out.extend(cadaddy_io::pointcloud_io::read(&text, "s.xyz").ok().map(|c| cadaddy_io::pointcloud_io::write_las(&c)));
            } else {
                out.push(text);
            }
        }
        "cdywalk" => out.extend(d.as_ref().and_then(|d| cadaddy_io::walkpkg::write(d).ok())),
        "markups" => {
            use cadaddy_doc::markup::{Markup, MarkupKind};
            let m = |kind, points: Vec<[f64; 2]>, text: &str| Markup {
                kind,
                points,
                strokes: vec![vec![[10.0, 10.0], [40.0, 30.0]]],
                rect: [72.0, 72.0, 200.0, 140.0],
                text: text.into(),
                author: "Reviewer".into(),
                subject: "Note".into(),
                color: Some([255, 0, 0]),
                date: "D:20260101000000".into(),
            };
            let list = vec![
                m(MarkupKind::Cloud, vec![[72.0, 72.0], [144.0, 72.0], [144.0, 144.0]], "Move"),
                m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0]], "Add"),
                m(MarkupKind::Note, Vec::new(), "General"),
                m(MarkupKind::Ink, Vec::new(), ""),
            ];
            out.extend(cadaddy_io::pdf_markups::annotated_pdf(612.0, 792.0, &list).ok());
        }
        "stl" | "obj" | "gltf" => {
            if let Some(d) = &d {
                match target {
                    "stl" => out.push(cadaddy_io::mesh3d_io::write_stl(d)),
                    "obj" => out.push(cadaddy_io::mesh3d_io::write_obj(d, "s.mtl").obj.into_bytes()),
                    _ => {
                        out.push(cadaddy_io::mesh3d_io::write_glb(d));
                        out.push(cadaddy_io::mesh3d_io::write_gltf(d));
                    }
                }
            }
        }
        _ => {}
    }
    out.extend(fuzz_core::corpus(match target {
        "image" => &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff"],
        "dxf" => &["dxf"],
        "dwg" => &["dwg"],
        "pdf" | "markups" => &["pdf"],
        "svg" => &["svg", "svgz"],
        "shx" => &["shx"],
        "las" => &["las"],
        "pts" => &["pts"],
        "xyz" => &["xyz", "txt"],
        "cdywalk" => &["cdywalk"],
        "stl" => &["stl"],
        "obj" => &["obj"],
        "gltf" => &["gltf", "glb"],
        _ => &["cdy"],
    }));
    out
}

fn main() {
    fuzz_core::main_with(&fuzz_core::Harness {
        targets: &TARGETS,
        run: run_target,
        seeds,
        text: |t| matches!(t, "dxf" | "svg" | "pts" | "xyz" | "obj" | "gltf"),
    });
}
