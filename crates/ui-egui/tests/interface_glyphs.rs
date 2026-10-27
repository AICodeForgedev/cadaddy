//! Icon-like characters in button and label text show in the fonts CADaddy ships (UI review
//! 0.7.7b #1, i18n test #1: "✕", "⇤", "⌀" were empty boxes on Windows). The web build has no
//! system fonts and Segoe UI lacks most symbols, so a symbol is fine only if egui's bundled
//! faces have it; anything else is drawn in code (`icons.rs`) or listed in [`KNOWN`] with why.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Calls whose string arguments are shown as a widget's text.
const WIDGET_CALLS: &[&str] = &[
    "button(",
    "Button::new(",
    "label(",
    "Label::new(",
    "RichText::new(",
    "heading(",
    "checkbox(",
    "selectable_value(",
    "radio_value(",
    "radio(",
    "hyperlink_to(",
    "on_hover_text(",
    "tl!(",
    "tf!(",
    "collapsing(",
    "shortcut_text(",
];

/// Symbols the bundled faces lack that are still used, per file (a new file using one fails):
/// the desktop's symbol face draws them (Segoe UI Symbol, which `theme::install_fonts` adds on
/// Windows; DejaVu or Noto on Linux; the system font on macOS), and the web build shows a box.
/// Open: docs/regressions.md, "Web-build symbol boxes".
const KNOWN: &[(char, &str, &str)] = &[
    ('←', "mtext_editor.rs", "paragraph alignment button"),
    ('→', "mtext_editor.rs", "paragraph alignment and Insert tab buttons"),
    ('→', "standardsdlg.rs", "standard → fix label"),
    ('↑', "customui.rs", "move up button"),
    ('↓', "customui.rs", "move down button"),
    ('↓', "mtext_editor.rs", "list button"),
    ('≡', "mtext_editor.rs", "justify and list buttons"),
    ('⌀', "mtext_editor.rs", "the text the Diameter button inserts; the button itself shows Ø"),
    ('⌄', "canvas.rs", "viewport control menu marker"),
    ('⏎', "touch.rs", "touch Enter button"),
    ('▲', "sheetsets.rs", "move up"),
    ('▲', "credits.rs", "sort marker"),
    ('▲', "publishdlg.rs", "move up"),
    ('▲', "managers.rs", "move up"),
    ('▼', "sheetsets.rs", "move down"),
    ('▼', "quick.rs", "drop-down marker"),
    ('▼', "credits.rs", "sort marker"),
    ('▼', "publishdlg.rs", "move down"),
    ('▼', "managers.rs", "move down"),
    ('▸', "ifcpalette.rs", "tree expander"),
    ('▸', "activitypalette.rs", "tree expander"),
    ('●', "canvas/blockedit.rs", "Block Editor badge"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") && p.file_name().is_some_and(|n| n != "tests.rs") {
            out.push(p);
        }
    }
}

/// The string literals on a line.
fn literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (&mut cur, c) {
            (None, '"') => cur = Some(String::new()),
            (Some(s), '\\') => {
                if let Some(n) = chars.next() {
                    s.push(n);
                }
            }
            (Some(_), '"') => out.extend(cur.take()),
            (Some(s), c) => s.push(c),
            (None, '/') if line.contains("//") && line.trim_start().starts_with("//") => break,
            _ => {}
        }
    }
    out
}

/// Symbol characters (not letters, digits or spaces) shown as widget text in the interface's
/// non-test source, with where each is used.
fn interface_symbols() -> BTreeMap<char, Vec<String>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 100, "only {} files under {}", files.len(), src.display());
    let mut out: BTreeMap<char, Vec<String>> = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        // Unit tests sit at the end of a file; they are not interface text.
        let code = text.split("#[cfg(test)]\nmod ").next().unwrap_or("");
        for (i, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") || !WIDGET_CALLS.iter().any(|c| line.contains(c)) {
                continue;
            }
            for lit in literals(line) {
                for c in lit.chars().filter(|c| !c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace()) {
                    let at = format!("{}:{}", f.strip_prefix(&src).unwrap_or(&f).display(), i + 1);
                    out.entry(c).or_default().push(at);
                }
            }
        }
    }
    out
}

#[test]
fn icon_like_characters_in_buttons_and_labels_are_in_the_shipped_fonts() {
    let symbols = interface_symbols();
    // The scan finds the symbols the interface does use (menu paths' ▸, the degree sign).
    assert!(symbols.contains_key(&'▸') && symbols.contains_key(&'°'), "scan found {:?}", symbols.keys().collect::<String>());
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::default());
    let mut output = ctx.run_ui(Default::default(), |_| {});
    output.textures_delta.clear();
    let covered: BTreeSet<char> = ctx.fonts_mut(|view| {
        let mut font = view.fonts.font(&egui::FontFamily::Proportional);
        symbols.keys().copied().filter(|c| font.characters().contains_key(c)).collect()
    });
    let known = |c: char, at: &str| KNOWN.iter().any(|(k, file, _)| *k == c && at.split(':').next() == Some(*file));
    let missing: Vec<String> = symbols
        .iter()
        .filter(|(c, _)| !covered.contains(c))
        .filter_map(|(c, at)| {
            let new: Vec<&String> = at.iter().filter(|a| !known(*c, a)).collect();
            (!new.is_empty()).then(|| format!("{c} (U+{:04X}) at {new:?}", u32::from(*c)))
        })
        .collect();
    assert!(missing.is_empty(), "symbols the shipped fonts lack (draw them with icons::button or use another character):\n{}", missing.join("\n"));
}
