//! UI screenshot regression suite: the main window, each ribbon tab and every dialog and floating
//! palette, at 1600×1000 and 1024×640, light and dark, in English, German and French, rendered
//! headless (egui_kittest's wgpu renderer) and compared with golden images under
//! `tests/golden/<platform>/` with a tolerance. Layout regressions (a palette's height, a dialog
//! clipped or grown, a tab's panels moved) show as a changed size or position, or as pixels that
//! differ.
//!
//! A ribbon tab or dialog is stored as the part of the window that differs from the bare window
//! (its own rectangle, cropped), at half size in grey, to keep the images small; its rectangle
//! goes in `shots.tsv`. The report — what differs, with the render, the golden and a diff image —
//! goes to `target/tmp/ui-shots/report.md`.
//!
//! Goldens are per platform (and font set): the text renders differently elsewhere. A platform
//! with no goldens renders everything and passes with a note; without a GPU adapter the suite is
//! skipped with a note. `UI_SHOTS_SAVE=1` saves every render, in full, beside the report. After
//! an intended UI change: `UPDATE_UI_GOLDEN=1 cargo test -p cadaddy-ui-egui --test ui_shots`,
//! then look at the changed images before committing them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, menus, ribbon};
use image::{GrayImage, Luma, Rgb, RgbImage, RgbaImage};
use serde_json::json;

const SIZES: &[(u32, u32)] = &[(1600, 1000), (1024, 640)];
const THEMES: &[&str] = &["light", "dark"];

/// A pixel differs when its grey level moves by more than this (0–255).
const LEVEL: u8 = 24;
/// A shot differs when more of its pixels differ than this share (or 16 pixels, whichever is more):
/// a glyph (a version number) passes, a moved control does not.
const SHARE: f64 = 0.005;
/// A rectangle may move or change size by this much (window pixels).
const SLACK: u32 = 2;

/// The dialogs the app opens by id (`ui.dialog`): dialogs.rs and the dialogs with their own id.
fn dialog_ids() -> Vec<&'static str> {
    let src = include_str!("../src/dialogs.rs");
    let mut ids: Vec<&'static str> = src
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix('"')?;
            let (id, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then_some(id)
        })
        .collect();
    ids.extend(["linetype", "ptype", "textstyle", "tablestyle", "dimstyle", "mleaderstyle", "pagesetup", "csettings", "plot", "battman"]);
    ids.extend(["materials", "laywalk", "exchangeloss", "hyperlink", "rooms"]);
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The goldens' folder for this platform and font set.
fn platform() -> String {
    let fonts = if cadaddy_fonts::CRAFT_FONTS.is_empty() { "" } else { "-craftfonts" };
    format!("{}-{}{fonts}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The system interface font the app loads (`theme::install_fonts` tries these in turn), as a
/// fingerprint: an OS update that changes it changes every render, so goldens made with another
/// font are not compared.
fn system_font() -> String {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc", "/Library/Fonts/Arial.ttf"]
    } else if cfg!(windows) {
        &["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\arial.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        ]
    };
    candidates
        .iter()
        .find_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
            Some(format!("{p} {} {hash:016x}", bytes.len()))
        })
        .unwrap_or_else(|| "egui's own fonts".into())
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(platform())
}

fn report_dir() -> PathBuf {
    let tmp = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    tmp.join("ui-shots")
}

/// The window in `lang`, `theme` and `size`, with the ribbon tab and dialog given, rendered.
fn render(lang: &str, theme: &str, (w, h): (u32, u32), tab: &str, dialog: Option<&str>) -> RgbaImage {
    let mut app = CadApp::new(Session::new(), Services::default());
    let _ = app.run("ui.sample", json!({}));
    // DesignCenter starts in the drawing's folder, else the home folder (which is private and
    // differs from machine to machine): a made-up folder, relative to the crate.
    if let Ok(st) = app.session.state_mut() {
        st.path = Some("tests/designcenter/Sample Floor Plan.dxf".into());
    }
    app.ui.ribbon = true;
    app.ui.show_palettes = true;
    app.ui.ribbon_tab = tab.to_string();
    let _ = menus::run_ui_command(&mut app, &format!("ui.theme.{theme}"), &json!({}));
    let _ = menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": lang }));
    app.ui.dialog = dialog.map(str::to_string);
    #[allow(clippy::cast_precision_loss)]
    let size = egui::vec2(w as f32, h as f32);
    let mut harness = egui_kittest::Harness::builder().with_size(size).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness.render().unwrap()
}

/// The rectangle where `img` differs from `base`: (x, y, w, h).
fn changed(base: &RgbaImage, img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if base.get_pixel_checked(x, y) != Some(p) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    (x0 != u32::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// What a golden stores: half size, 16 grey levels (well inside [`LEVEL`]; they compress well).
fn small(img: &RgbaImage) -> GrayImage {
    let (w, h) = ((img.width() / 2).max(1), (img.height() / 2).max(1));
    let mut g = image::DynamicImage::ImageRgba8(img.clone()).resize_exact(w, h, image::imageops::FilterType::Triangle).to_luma8();
    g.pixels_mut().for_each(|p| p.0[0] = p.0[0] / 16 * 16 + 8);
    g
}

/// Saves a golden as small as the PNG encoder makes it.
fn save_small(img: &GrayImage, path: &Path) {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    PngEncoder::new_with_quality(file, CompressionType::Best, FilterType::Adaptive)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::L8)
        .unwrap();
}

/// The languages' tests run at once and share `shots.tsv`.
static MANIFEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Held while a language renders ([`check_language`]).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One shot: where it is in the window and what it looks like.
struct Shot {
    rect: (u32, u32, u32, u32),
    full: RgbaImage,
    small: GrayImage,
}

/// Every shot of one configuration (`en-light-1600x1000`), by scene, and the dialogs that showed nothing.
fn shots(lang: &str, theme: &str, size: (u32, u32)) -> (BTreeMap<String, Shot>, Vec<String>) {
    let mut out = BTreeMap::new();
    let main = render(lang, theme, size, "Home", None);
    let crop = |img: &RgbaImage, (x, y, w, h): (u32, u32, u32, u32)| image::imageops::crop_imm(img, x, y, w, h).to_image();
    for tab in ribbon::TABS.iter().map(|t| t.name).filter(|n| *n != "Home") {
        let img = render(lang, theme, size, tab, None);
        if let Some(rect) = changed(&main, &img) {
            let full = crop(&img, rect);
            out.insert(format!("tab-{}", tab.to_lowercase()), Shot { rect, small: small(&full), full });
        }
    }
    let mut blank = Vec::new();
    for id in dialog_ids() {
        let img = render(lang, theme, size, "Home", Some(id));
        match changed(&main, &img) {
            Some(rect) => {
                let full = crop(&img, rect);
                out.insert(format!("dialog-{}", id.replace([':', '.'], "_")), Shot { rect, small: small(&full), full });
            }
            None => blank.push(id.to_string()),
        }
    }
    out.insert("main".into(), Shot { rect: (0, 0, size.0, size.1), small: small(&main), full: main });
    (out, blank)
}

/// The golden rectangles: configuration → scene → rectangle.
type Manifest = BTreeMap<String, BTreeMap<String, (u32, u32, u32, u32)>>;

fn read_manifest(dir: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(dir.join("shots.tsv")).ok()?;
    let mut m = Manifest::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let [config, scene, x, y, w, h] = f[..] else { continue };
        let n = |s: &str| s.parse::<u32>().unwrap_or(0);
        m.entry(config.to_string()).or_default().insert(scene.to_string(), (n(x), n(y), n(w), n(h)));
    }
    Some(m)
}

fn far(a: u32, b: u32) -> bool {
    a.abs_diff(b) > SLACK
}

/// How `now` differs from the golden at `rect`/`old`, if it does.
fn compare(rect: (u32, u32, u32, u32), old: &GrayImage, now: &Shot) -> Option<(String, Option<RgbImage>)> {
    let r = now.rect;
    if far(rect.2, r.2) || far(rect.3, r.3) {
        return Some((format!("size {}×{} → {}×{}", rect.2, rect.3, r.2, r.3), None));
    }
    if far(rect.0, r.0) || far(rect.1, r.1) {
        return Some((format!("moved from {},{} to {},{}", rect.0, rect.1, r.0, r.1), None));
    }
    let (w, h) = (old.width().min(now.small.width()), old.height().min(now.small.height()));
    let mut diff = RgbImage::new(w, h);
    let mut count = 0usize;
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (old.get_pixel(x, y).0[0], now.small.get_pixel(x, y).0[0]);
            let off = a.abs_diff(b) > LEVEL;
            count += usize::from(off);
            diff.put_pixel(x, y, if off { Rgb([255, 0, 0]) } else { Rgb([b / 3, b / 3, b / 3]) });
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let share = count as f64 / (f64::from(w) * f64::from(h)).max(1.0);
    (count > 16 && share > SHARE).then(|| (format!("{count} pixels differ ({:.2}%)", share * 100.0), Some(diff)))
}

/// Renders and checks (or, with `UPDATE_UI_GOLDEN`, writes) every shot in `lang`.
fn check_language(lang: &str) {
    // One language renders at a time: some GPU drivers (WSL's, over /dev/dxg) crash the process
    // when several threads create wgpu devices at once.
    let _gpu = GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // No GPU adapter (a headless CI machine): nothing to compare.
    let probe = std::panic::catch_unwind(|| render("en", "light", (64, 48), "Home", None));
    if probe.is_err() {
        eprintln!("ui_shots: no GPU adapter for the wgpu renderer; skipped");
        return;
    }
    let update = std::env::var_os("UPDATE_UI_GOLDEN").is_some();
    // Every render in full, for looking at: `UI_SHOTS_SAVE=1`.
    let save_all = std::env::var_os("UI_SHOTS_SAVE").is_some();
    let golden = golden_dir();
    let font = system_font();
    let made_with = std::fs::read_to_string(golden.join("system-font.txt")).unwrap_or_default();
    let other_font = !update && !made_with.is_empty() && made_with.trim() != font;
    let manifest = if other_font { None } else { read_manifest(&golden) };
    if other_font {
        eprintln!("ui_shots: goldens made with another system font; rendered only (see the report)");
    }
    let out = report_dir().join(lang);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let mut notes = Vec::new();
    for &size in SIZES {
        for &theme in THEMES {
            let config = format!("{lang}-{theme}-{}x{}", size.0, size.1);
            let (shots, blank) = shots(lang, theme, size);
            if !blank.is_empty() && lang == "en" && theme == "light" {
                notes.push(format!("{config}: show nothing without a drawing state or an argument: {}", blank.join(", ")));
            }
            for (scene, shot) in &shots {
                let (x, y, w, h) = shot.rect;
                rows.push(format!("{config}\t{scene}\t{x}\t{y}\t{w}\t{h}"));
                if save_all {
                    shot.full.save(out.join(format!("{config}-{scene}.png"))).unwrap();
                }
                let file = Path::new(&config).join(format!("{scene}.png"));
                if update {
                    std::fs::create_dir_all(golden.join(&config)).unwrap();
                    save_small(&shot.small, &golden.join(&file));
                    continue;
                }
                let Some(m) = &manifest else { continue };
                let Some(rect) = m.get(&config).and_then(|c| c.get(scene)) else {
                    failures.push(format!("{config} {scene}: new (no golden)"));
                    continue;
                };
                let Ok(old) = image::open(golden.join(&file)).map(|i| i.to_luma8()) else {
                    failures.push(format!("{config} {scene}: golden image missing"));
                    continue;
                };
                if let Some((why, diff)) = compare(*rect, &old, shot) {
                    let base = out.join(format!("{config}-{scene}"));
                    shot.full.save(base.with_extension("now.png")).unwrap();
                    old.save(base.with_extension("golden.png")).unwrap();
                    if let Some(d) = diff {
                        d.save(base.with_extension("diff.png")).unwrap();
                    }
                    failures.push(format!("{config} {scene}: {why} — see {}.*.png", base.display()));
                }
            }
            if let Some(m) = &manifest {
                for scene in m.get(&config).into_iter().flat_map(|c| c.keys()).filter(|s| !shots.contains_key(*s)) {
                    failures.push(format!("{config} {scene}: no longer shown"));
                }
            }
        }
    }
    if update {
        let _lock = MANIFEST.lock();
        // This language's rows replace its old ones; the other languages' stay.
        let mut all: Vec<String> = std::fs::read_to_string(golden.join("shots.tsv"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty() && !l.starts_with(&format!("{lang}-")))
            .map(str::to_string)
            .collect();
        all.extend(rows);
        all.sort();
        std::fs::write(golden.join("system-font.txt"), format!("{font}\n")).unwrap();
        let head = "# configuration\tscene\tx\ty\twidth\theight (window pixels; images at half size)\n";
        std::fs::write(golden.join("shots.tsv"), format!("{head}{}\n", all.join("\n"))).unwrap();
    }
    let mut report = format!("# UI shots, {lang} ({})\n\n", platform());
    if other_font {
        report.push_str(&format!(
            "The goldens were made with another system font ({}; this machine has {font}): rendered only. Make them again with UPDATE_UI_GOLDEN=1.\n\n",
            made_with.trim()
        ));
    } else if manifest.is_none() && !update {
        report.push_str(&format!("No goldens for {}: rendered only. Make them with UPDATE_UI_GOLDEN=1.\n\n", platform()));
    }
    report.push_str(&format!("## Differences ({})\n\n", failures.len()));
    failures.iter().for_each(|f| report.push_str(&format!("- {f}\n")));
    report.push_str(&format!("\n## Notes ({})\n\n", notes.len()));
    notes.iter().for_each(|n| report.push_str(&format!("- {n}\n")));
    std::fs::write(out.join("report.md"), &report).unwrap();
    assert!(failures.is_empty(), "{report}");
}

#[test]
fn english() {
    check_language("en");
}

#[test]
fn german() {
    check_language("de");
}

#[test]
fn french() {
    check_language("fr");
}

#[test]
fn a_moved_or_changed_shot_is_a_difference() {
    let img = RgbaImage::from_fn(40, 20, |x, _| image::Rgba([if x < 20 { 0 } else { 255 }, 0, 0, 255]));
    let shot = Shot { rect: (10, 10, 40, 20), small: small(&img), full: img.clone() };
    assert!(compare((10, 10, 40, 20), &shot.small, &shot).is_none(), "the same");
    assert!(compare((11, 9, 41, 21), &shot.small, &shot).is_none(), "within the slack");
    assert!(compare((10, 10, 40, 30), &shot.small, &shot).is_some_and(|(w, _)| w.contains("size")));
    assert!(compare((30, 10, 40, 20), &shot.small, &shot).is_some_and(|(w, _)| w.contains("moved")));
    let mut other = shot.small.clone();
    for y in 0..10 {
        for x in 0..20 {
            other.put_pixel(x, y, Luma([255 - other.get_pixel(x, y).0[0]]));
        }
    }
    assert!(compare((10, 10, 40, 20), &other, &shot).is_some_and(|(w, d)| w.contains("pixels differ") && d.is_some()));
    // A few pixels (a changed glyph) pass.
    let mut glyph = shot.small.clone();
    glyph.put_pixel(3, 3, Luma([255 - glyph.get_pixel(3, 3).0[0]]));
    assert!(compare((10, 10, 40, 20), &glyph, &shot).is_none());
}
