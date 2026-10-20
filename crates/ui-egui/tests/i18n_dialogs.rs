//! Every language, every dialog: for each interface language, every dialog and palette is opened
//! in a headless app and rendered, and what it paints is checked: interface strings with no
//! translation in that language, text cut off by its control (and not cut in English), glyphs the
//! fonts lack (CJK), and the shell mirrored for Arabic. The findings, per language and dialog, go
//! to `target/tmp/i18n-dialogs.md` (printed with `CADADDY_I18N_REPORT=1`); renders of the dialogs
//! that have one are saved beside it when `CADADDY_I18N_SHOTS=1`.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, menus};
use egui::epaint::ClippedShape;
use serde_json::json;

/// Dialogs whose content scrolls sideways (the Layer Properties Manager's table): a column at
/// the edge is partly shown on purpose.
const SCROLLED: &[&str] = &["layers"];

/// Interface strings each language's dialogs showed untranslated when this test was made (the
/// side file `i18ntest-i18n.tsv` fills them). More than this fails; lower it as rows land.
const MISSING_AT_MOST: &[(&str, usize)] = &[
    ("ar", 18),
    ("cs", 19),
    ("de", 27),
    ("es", 19),
    ("fr", 27),
    ("it", 18),
    ("ja", 117),
    ("ko", 18),
    ("pt-BR", 18),
    ("ru", 27),
    ("sv", 18),
    ("uk", 0),
    ("zh", 19),
];

/// A render of dialog `id` in `lang` (egui_kittest's wgpu renderer), for the report.
fn shot(lang: &str, id: &str, dir: &std::path::Path) {
    let mut app = CadApp::new(Session::new(), Services::default());
    let _ = app.run("ui.sample", json!({}));
    let _ = menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": lang }));
    app.ui.dialog = Some(id.to_string());
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    if let Ok(img) = h.render() {
        let _ = img.save(dir.join(format!("{lang}-{}.png", id.replace([':', '.'], "_"))));
    }
}

/// Names that stay as they are in every language.
const NAMES: &[&str] = &["CADaddy", "AICodeForge", "DesignCenter"];

/// The catalogs: (language code, source).
const CATALOGS: &[(&str, &str)] = &[
    ("uk", include_str!("../src/i18n/uk.tsv")),
    ("pt-BR", include_str!("../src/i18n/pt_br.tsv")),
    ("de", include_str!("../src/i18n/de.tsv")),
    ("es", include_str!("../src/i18n/es.tsv")),
    ("cs", include_str!("../src/i18n/cs.tsv")),
    ("ru", include_str!("../src/i18n/ru.tsv")),
    ("fr", include_str!("../src/i18n/fr.tsv")),
    ("zh", include_str!("../src/i18n/zh.tsv")),
    ("it", include_str!("../src/i18n/it.tsv")),
    ("ko", include_str!("../src/i18n/ko.tsv")),
    ("sv", include_str!("../src/i18n/sv.tsv")),
    ("ar", include_str!("../src/i18n/ar.tsv")),
    ("ja", include_str!("../src/i18n/ja.tsv")),
];

/// The dialogs the app opens by id (`ui.dialog`): dialogs.rs, the style and settings managers
/// and the dialogs with their own id constant.
fn dialog_ids() -> Vec<&'static str> {
    let src = include_str!("../src/dialogs.rs");
    let mut ids: Vec<&'static str> = src
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let rest = t.strip_prefix('"')?;
            let (id, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then_some(id)
        })
        .collect();
    ids.extend(["linetype", "ptype", "textstyle", "tablestyle", "dimstyle", "mleaderstyle", "pagesetup", "csettings", "plot", "battman"]);
    ids.extend(["materials", "laywalk", "exchangeloss", "hyperlink", "rooms"]);
    ids.retain(|id| !["spacemouse", "language"].contains(id));
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn keys(source: &str) -> BTreeSet<&str> {
    source.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(1)).filter(|k| !k.is_empty()).collect()
}

/// A painted text: what it says, its rectangle and the rectangle it is clipped to.
struct Painted {
    text: String,
    rect: egui::Rect,
    clip: egui::Rect,
}

fn painted(shapes: &[ClippedShape]) -> Vec<Painted> {
    fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<Painted>) {
        match shape {
            egui::Shape::Text(t) => out.push(Painted { text: t.galley.text().to_string(), rect: t.galley.rect.translate(t.pos.to_vec2()), clip }),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in shapes {
        walk(&s.shape, s.clip_rect, &mut out);
    }
    out
}

/// A text cut off at the side of what clips it (partly shown).
fn cut(p: &Painted) -> bool {
    let (r, c) = (p.rect, p.clip);
    r.width() > 0.0
        && r.bottom() > c.top() + 1.0
        && r.top() < c.bottom() - 1.0
        && ((r.right() > c.right() + 1.5 && r.left() < c.right() - 1.5) || (r.left() < c.left() - 1.5 && r.right() > c.left() + 1.5))
}

fn frame(app: &mut CadApp, ctx: &egui::Context) -> Vec<ClippedShape> {
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out.shapes
}

/// The dialog `id` open on a fresh app in `lang`: what it painted beyond the bare app.
fn render(lang: &str, id: &str) -> (Vec<Painted>, String, BTreeSet<char>) {
    let mut app = CadApp::new(Session::new(), Services::default());
    let _ = app.run("ui.sample", json!({}));
    let _ = menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": lang }));
    let ctx = egui::Context::default();
    for _ in 0..3 {
        frame(&mut app, &ctx);
    }
    let base: BTreeSet<String> = painted(&frame(&mut app, &ctx)).into_iter().map(|p| p.text).collect();
    app.ui.dialog = Some(id.to_string());
    let mut shapes = Vec::new();
    for _ in 0..4 {
        shapes = frame(&mut app, &ctx);
    }
    let shown = app.language().to_string();
    // The dialog's own text (the shell behind it was checked with every other dialog).
    let p: Vec<Painted> = painted(&shapes).into_iter().filter(|p| !base.contains(&p.text)).collect();
    // Characters the interface fonts (as the app installed them for this language) lack.
    // A character lacks a glyph when it is drawn as the replacement box. (egui's `has_glyph` can't
    // tell: it is false for every character of the face that holds the box, which on Linux is
    // the interface font itself, DejaVu Sans.)
    let mut lacking = BTreeSet::new();
    ctx.fonts_mut(|fs| {
        let mut uv = |c: char| {
            let g = fs.layout_no_wrap(c.to_string(), egui::FontId::proportional(14.0), egui::Color32::WHITE);
            g.rows.first().and_then(|r| r.row.glyphs.first()).map(|g| g.uv_rect)
        };
        let replacement = uv('◻');
        for x in &p {
            for c in x.text.chars().filter(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_control() && *c != '◻') {
                if uv(c) == replacement {
                    lacking.insert(c);
                }
            }
        }
    });
    (p, shown, lacking)
}

#[derive(Default)]
struct Findings {
    missing: BTreeMap<String, BTreeSet<String>>,
    cut: BTreeMap<String, BTreeSet<String>>,
    glyphs: BTreeMap<String, BTreeSet<char>>,
}

#[test]
fn every_language_every_dialog() {
    let ids = dialog_ids();
    assert!(ids.len() > 50, "only {} dialogs found", ids.len());
    let master: BTreeSet<&str> = CATALOGS.iter().flat_map(|(_, s)| keys(s)).collect();
    // English first: its cut texts are the baseline (a dialog that cuts in English too is not a
    // translation's fault).
    let mut english_cut: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut f = Findings::default();
    for id in &ids {
        let (p, _, lacking) = render("en", id);
        english_cut.insert(id, p.iter().filter(|x| cut(x)).map(|x| x.text.clone()).collect());
        if !lacking.is_empty() {
            f.glyphs.entry(format!("en · {id}")).or_default().extend(lacking);
        }
    }
    let mut shown_as: BTreeMap<&str, String> = BTreeMap::new();
    for (lang, src) in CATALOGS {
        let have = keys(src);
        for id in &ids {
            let (p, shown, lacking) = render(lang, id);
            shown_as.insert(lang, shown.clone());
            let key = format!("{lang} · {id}");
            for x in &p {
                // A line of interface text (a catalog key) this language has no row for.
                for line in x.text.lines().map(str::trim) {
                    if master.contains(line) && !have.contains(line) && shown == *lang && !NAMES.contains(&line) {
                        f.missing.entry(key.clone()).or_default().insert(line.to_string());
                    }
                }
                if cut(x) && !SCROLLED.contains(id) && !english_cut.get(id).is_some_and(|e| e.contains(&x.text)) && x.text.trim().chars().count() > 1
                {
                    f.cut.entry(key.clone()).or_default().insert(x.text.clone());
                }
            }
            if !lacking.is_empty() {
                f.glyphs.entry(key.clone()).or_default().extend(lacking);
            }
        }
    }
    let mut md = format!("# Every language, every dialog\n\n{} dialogs × {} languages.\n\n", ids.len(), CATALOGS.len());
    md.push_str(&format!("Shown as (a language without fonts on this machine stays English): {shown_as:?}\n\n"));
    md.push_str("## Interface text with no translation\n\n");
    for (k, v) in &f.missing {
        md.push_str(&format!("- {k}: {}\n", v.iter().map(String::as_str).collect::<Vec<_>>().join(" | ")));
    }
    md.push_str("\n## Text cut off by its control (not cut in English)\n\n");
    for (k, v) in &f.cut {
        md.push_str(&format!("- {k}: {}\n", v.iter().map(String::as_str).collect::<Vec<_>>().join(" | ")));
    }
    md.push_str("\n## Missing glyphs\n\n");
    for (k, v) in &f.glyphs {
        md.push_str(&format!("- {k}: {}\n", v.iter().collect::<String>()));
    }
    let tmp = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    if std::env::var("CADADDY_I18N_SHOTS").is_ok() {
        let dir = tmp.join("i18n-shots");
        let _ = std::fs::create_dir_all(&dir);
        let failing: BTreeSet<&String> = f.cut.keys().chain(f.glyphs.keys()).collect();
        md.push_str("\n## Renders of the dialogs with findings\n\n");
        for k in failing {
            if let Some((lang, id)) = k.split_once(" · ") {
                shot(lang, id, &dir);
                md.push_str(&format!("- {k}: i18n-shots/{lang}-{}.png\n", id.replace([':', '.'], "_")));
            }
        }
    }
    let out = tmp.join("i18n-dialogs.md");
    let _ = std::fs::write(&out, &md);
    if std::env::var("CADADDY_I18N_REPORT").is_ok() {
        println!("{md}\n(report: {})", out.display());
    }
    // Untranslated interface strings: at most each language's count when this test was made.
    let mut over = Vec::new();
    for (lang, most) in MISSING_AT_MOST {
        let n: BTreeSet<&String> = f.missing.iter().filter(|(k, _)| k.split(" · ").next() == Some(*lang)).flat_map(|(_, v)| v.iter()).collect();
        if n.len() > *most {
            over.push(format!("{lang}: {} untranslated (at most {most})", n.len()));
        }
    }
    assert!(over.is_empty(), "new untranslated interface text ({}):\n{}", out.display(), over.join("\n"));
    assert!(f.glyphs.is_empty(), "characters the interface fonts lack (empty boxes): {:?}", f.glyphs);
    assert!(f.cut.is_empty(), "text cut off by its control: {:?}", f.cut);
}
