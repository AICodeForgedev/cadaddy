//! PNGOUT, JPGOUT, BMPOUT, TIFOUT and the JPEG / TIFF / BMP plot devices (Q149): the image has the
//! size asked for, shows the selected objects where they are in the current view and nothing else,
//! in colour, greys or black and white; an empty selection is refused.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-rasterout-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A red square of thick lines on the left and a blue one on the right, the view fitted to both.
fn scene() -> (Session, Vec<String>, Vec<String>) {
    hooks();
    let mut s = Session::new();
    let square = |s: &mut Session, x: f64, color: &str| -> Vec<String> {
        let r = s.execute("rectang", &json!({ "p1": [x, 0], "p2": [x + 40.0, 40.0], "width": 4 })).unwrap();
        let h = r["handle"].as_str().map(str::to_string).unwrap_or_default();
        s.execute("chprop", &json!({ "handles": [h], "color": color })).unwrap();
        vec![h]
    };
    let red = square(&mut s, 0.0, "red");
    let blue = square(&mut s, 100.0, "blue");
    s.execute("zoom", &json!({ "mode": "extents" })).unwrap();
    (s, red, blue)
}

fn decode(bytes: &[u8]) -> image::RgbImage {
    image::load_from_memory(bytes).unwrap().to_rgb8()
}

/// Pixels that are clearly red / blue.
fn colors(img: &image::RgbImage) -> (usize, usize) {
    let red = img.pixels().filter(|p| p.0[0] > 180 && p.0[1] < 80 && p.0[2] < 80).count();
    let blue = img.pixels().filter(|p| p.0[2] > 180 && p.0[0] < 80 && p.0[1] < 80).count();
    (red, blue)
}

#[test]
fn each_command_writes_the_selection_only_at_the_size_asked() {
    let (mut s, red, _) = scene();
    let d = dir("cmds");
    for (cmd, ext) in [("pngout", "png"), ("jpgout", "jpg"), ("bmpout", "bmp"), ("tifout", "tif")] {
        let path = d.join(format!("red.{ext}"));
        let r = s.execute(cmd, &json!({ "path": path, "handles": red, "size": [400, 200] })).unwrap();
        assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["objects"].as_u64()), (Some(400), Some(200), Some(1)), "{cmd}: {r}");
        let img = decode(&std::fs::read(&path).unwrap());
        assert_eq!(img.dimensions(), (400, 200), "{cmd}");
        let (reds, blues) = colors(&img);
        assert!(reds > 200, "{cmd}: the red square is drawn ({reds} red pixels)");
        assert_eq!(blues, 0, "{cmd}: the blue one isn't selected");
        // On the left half, where it is in the view.
        let left = img.enumerate_pixels().filter(|(x, _, p)| *x < 200 && p.0[0] > 180 && p.0[1] < 80).count();
        assert!(left * 10 >= reds * 9, "{cmd}: {left} of {reds} on the left");
        // White paper by default.
        assert!(img.get_pixel(399, 0).0.iter().all(|c| *c > 240), "{cmd}");
    }
    // Background colour, and returned as data without a path.
    let r = s.execute("pngout", &json!({ "handles": red, "size": [100, 50], "background": "#000000" })).unwrap();
    let img = decode(&cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap()).unwrap());
    assert!(img.get_pixel(99, 0).0.iter().all(|c| *c < 10), "black background");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn greys_and_black_and_white() {
    let (mut s, red, _) = scene();
    for (bits, max_levels) in [(8, 256), (1, 2)] {
        let r = s.execute("tifout", &json!({ "handles": red, "size": [300, 150], "colorDepth": bits })).unwrap();
        let bytes = cadaddy_engine::cmd::file::base64_decode(r["data"].as_str().unwrap()).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        let rgb = img.to_rgb8();
        assert!(rgb.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]), "{bits}: grey only");
        let levels: std::collections::HashSet<u8> = rgb.pixels().map(|p| p.0[0]).collect();
        assert!(levels.len() <= max_levels && levels.len() >= 2, "{bits}: {} levels", levels.len());
    }
}

#[test]
fn nothing_selected_and_bad_values_are_refused() {
    let (mut s, red, _) = scene();
    s.execute("select", &json!({ "handles": [] })).ok();
    assert!(s.execute("pngout", &json!({ "size": [10, 10] })).is_err(), "no selection");
    assert!(s.execute("pngout", &json!({ "handles": ["FFFFFF"], "size": [10, 10] })).is_err(), "no such object");
    for p in [
        json!({ "handles": red, "size": [0, 10] }),
        json!({ "handles": red, "size": [100000, 10] }),
        json!({ "handles": red, "size": "big" }),
        json!({ "handles": red, "colorDepth": 7 }),
        json!({ "handles": red, "background": "red" }),
    ] {
        assert!(s.execute("bmpout", &p).is_err(), "{p}");
    }
}

#[test]
fn typed_asks_for_the_file_then_the_objects() {
    let (mut s, _, _) = scene();
    let d = dir("typed");
    let path = d.join("typed-out");
    s.cmdline("jpgout").unwrap();
    s.cmdline(&path.display().to_string()).unwrap();
    // A crossing window around the red square only.
    for l in ["c", "-10,-10", "50,50", ""] {
        s.cmdline(l).unwrap();
    }
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let file = d.join("typed-out.jpg");
    let img = decode(&std::fs::read(&file).unwrap_or_else(|_| panic!("{:?}", s.log)));
    let (reds, blues) = colors(&img);
    assert!(reds > 50 && blues == 0, "{reds} {blues}");
    // Nothing selected: nothing written.
    s.cmdline("bmpout").unwrap();
    s.cmdline(&d.join("none.bmp").display().to_string()).unwrap();
    s.cmdline("").unwrap();
    assert!(!d.join("none.bmp").exists());
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_raster_plot_devices_write_their_formats() {
    let (mut s, _, _) = scene();
    let d = dir("plot");
    let names: Vec<String> = s.execute("plottermanager", &json!({})).unwrap()["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().or(v["name"].as_str()).map(str::to_string))
        .collect();
    for dev in ["CADaddy JPEG", "CADaddy TIFF", "CADaddy BMP"] {
        assert!(names.iter().any(|n| n == dev), "{dev} in {names:?}");
    }
    for (dev, ext, depth) in [("CADaddy JPEG", "jpg", 24), ("CADaddy TIFF", "tif", 1), ("CADaddy BMP", "bmp", 8)] {
        let path = d.join(format!("plot.{ext}"));
        s.execute("plot", &json!({ "device": dev, "path": path, "paper": "A4", "resolution": 100, "colorDepth": depth }))
            .unwrap_or_else(|e| panic!("{dev}: {e}"));
        let img = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
        // A4 landscape at 100 dpi: about 1169 × 827.
        assert!((img.width() as i64 - 1169).abs() <= 2 && (img.height() as i64 - 827).abs() <= 2, "{dev}: {}×{}", img.width(), img.height());
        let rgb = img.to_rgb8();
        if depth < 24 {
            assert!(rgb.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]), "{dev}: greys");
        }
        assert!(rgb.pixels().any(|p| p.0[0] < 128), "{dev}: something is drawn");
    }
    std::fs::remove_dir_all(&d).ok();
}
