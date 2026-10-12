use std::collections::BTreeSet;

use super::*;
use crate::{CadApp, Services};
use cadcraft_engine::Session;
use serde_json::json;

fn placeholders(text: &str) -> BTreeSet<&str> {
    text.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(key, _)| key)).collect()
}

#[test]
fn catalog_is_valid_and_preserves_placeholders_and_ellipses() {
    for language in &LANGUAGES {
        let (catalog, errors) = parse(language.source);
        assert!(errors.is_empty(), "{}: {errors:?}", language.code);
        for ((context, key), value) in catalog {
            assert!(["", "menu", "@plural"].contains(&context), "unknown context: {context}");
            let forms: Vec<_> = if context == "@plural" { value.split('|').collect() } else { vec![value] };
            if context == "@plural" {
                assert_eq!(forms.len(), language.plural_forms, "{}: {key}", language.code);
            }
            for form in forms {
                assert_eq!(placeholders(key), placeholders(form), "{key}");
                for suffix in ["...", "…"] {
                    assert_eq!(key.ends_with(suffix), form.ends_with(suffix), "{key}");
                }
            }
        }
    }
    let (parsed, errors) = parse("broken\n\tOpen\tВідкрити\n\tOpen\tВідкрити\n\tEmpty\t\n");
    assert_eq!(parsed.len(), 1);
    assert_eq!(errors.len(), 3);
    assert_eq!(tr("uk", "a future untranslated label"), "a future untranslated label");
    assert_eq!(tr("unknown", "Open"), "Open");
}

/// Ukrainian is the reference translation and must cover everything. Other languages fall back
/// to English for what they don't cover yet; their gaps are reported, not enforced, so adding a
/// command doesn't require a row in every language.
const REQUIRED: &[&str] = &["uk"];
const REPORTED: &[&str] = &["pt-BR", "de", "es", "cs", "ru", "fr", "zh"];

fn check(language: &str, what: &str, missing: Vec<String>) {
    if REQUIRED.contains(&language) {
        assert!(missing.is_empty(), "{language}: missing {what}: {missing:?}");
    } else if !missing.is_empty() {
        eprintln!("{language}: {} {what} fall back to English: {missing:?}", missing.len());
    }
}

#[test]
fn translations_cover_commands_menus_tools_and_snap_labels() {
    for language in REQUIRED.iter().chain(REPORTED) {
        check(language, "translations", missing_commands_menus_tools_and_snap_labels(language));
    }
}

fn missing_commands_menus_tools_and_snap_labels(language: &str) -> Vec<String> {
    let catalog = catalog(language).unwrap();
    let mut sources = BTreeSet::new();
    sources.extend(crate::menus::MENUS.iter().copied());
    for c in cadcraft_engine::command_specs() {
        sources.insert(c.label);
        sources.extend(c.menu.iter().copied());
    }
    for (_, label, path, _) in crate::menus::UI_COMMANDS {
        sources.insert(*label);
        sources.extend(path.iter().copied());
    }
    for groups in [crate::palettes::DRAFTING, crate::palettes::MODELING] {
        for (name, large, small) in groups {
            sources.insert(*name);
            sources.extend(large.iter().chain(*small).map(|(_, _, label)| *label));
        }
    }
    sources.extend(cadcraft_engine::snap::mode::ALL.iter().map(|(_, name)| *name));
    sources.into_iter().filter(|s| !s.is_empty() && !catalog.contains_key(&("", *s))).map(str::to_string).collect()
}

#[test]
fn every_marked_ui_literal_has_a_translation() {
    for language in REQUIRED.iter().chain(REPORTED) {
        check(language, "UI translations", missing_marked_ui_literals(language).into_iter().collect());
    }
}

fn missing_marked_ui_literals(language: &str) -> BTreeSet<String> {
    let catalog = catalog(language).unwrap();
    let sources = [
        include_str!("../xrefs.rs"),
        include_str!("../canvas.rs"),
        include_str!("../chrome.rs"),
        include_str!("../cmdline.rs"),
        include_str!("../credits.rs"),
        include_str!("../palettes/quickcalc.rs"),
        include_str!("../palettes/patterns.rs"),
        include_str!("../dialogs.rs"),
        include_str!("../layers.rs"),
        include_str!("../canvas/compare.rs"),
        include_str!("../palettes.rs"),
        include_str!("../parametric.rs"),
        include_str!("../quick.rs"),
        include_str!("../vpscale.rs"),
        include_str!("../options/dialog.rs"),
    ];
    let mut missing = BTreeSet::new();
    for source in sources {
        for marker in ["crate::tl!(", "crate::tf!("] {
            for rest in source.split(marker).skip(1) {
                // These macros require a quoted literal. Use JSON's string decoding for escapes.
                let rest = rest.trim_start();
                let mut escape = false;
                let end = rest
                    .char_indices()
                    .skip(1)
                    .find_map(|(i, c)| {
                        if escape {
                            escape = false;
                            None
                        } else if c == '\\' {
                            escape = true;
                            None
                        } else if c == '"' {
                            Some(i + 1)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let key: String = serde_json::from_str(&rest[..end]).unwrap();
                // Coordinate axes, glyph buttons and product names are not translated.
                if !["X", "Y", "B", "CADCraft"].contains(&key.as_str()) && !catalog.contains_key(&("", key.as_str())) {
                    missing.insert(key);
                }
            }
        }
    }
    missing
}

#[test]
fn locale_and_saved_preference_resolution() {
    for tag in ["uk", "uk-UA", "uk_UA.UTF-8", "UK-ua", " uk-UA ", "uk_UA@euro"] {
        assert_eq!(locale_language(tag), Some("uk"), "{tag}");
    }
    for tag in ["de", "de-DE", "de_DE.UTF-8", "DE-at", "de-CH"] {
        assert_eq!(locale_language(tag), Some("de"), "{tag}");
    }
    for (tag, code) in [("ru", "ru"), ("ru-RU", "ru"), ("ru_RU.UTF-8", "ru"), ("fr", "fr"), ("fr-FR", "fr"), ("fr_CA.UTF-8", "fr")] {
        assert_eq!(locale_language(tag), Some(code), "{tag}");
    }
    for tag in ["ua", "ukulele", "pl-PL", "", "garbage", "💙"] {
        assert_eq!(locale_language(tag), None, "{tag}");
    }
    for tag in ["pt", "pt-BR", "pt_BR.UTF-8", "PT-br", "pt-PT"] {
        assert_eq!(locale_language(tag), Some("pt-BR"), "{tag}");
    }
    assert_eq!(locale_language("ptolemy"), None);
    assert_eq!(Preference::Auto.resolve(&["pl-PL".into(), "pt-BR".into()]), "pt-BR");
    assert_eq!(Preference::Auto.resolve(&["fr-FR".into(), "pt-BR".into()]), "fr");
    for tag in ["es", "es-ES", "es_MX.UTF-8", "ES-ar"] {
        assert_eq!(locale_language(tag), Some("es"), "{tag}");
    }
    assert_eq!(Preference::Auto.resolve(&["es-ES".into(), "pt-BR".into()]), "es");
    assert_eq!(Preference::parse("es"), Some(Preference::Spanish));
    assert_eq!(Preference::BrazilianPortuguese.resolve(&["uk-UA".into()]), "pt-BR");
    assert_eq!(Preference::parse("pt-BR"), Some(Preference::BrazilianPortuguese));
    let locales = vec!["pl-PL".into(), "uk-UA".into(), "en-US".into()];
    assert_eq!(Preference::Auto.resolve(&locales), "uk");
    assert_eq!(Preference::English.resolve(&locales), "en");
    assert_eq!(Preference::German.resolve(&locales), "de");
    assert_eq!(Preference::Auto.resolve(&["de-DE".into(), "en-US".into()]), "de");
    assert_eq!(Preference::Auto.resolve(&["en-US".into(), "uk-UA".into()]), "en");
    assert_eq!(Preference::Auto.resolve(&[]), "en");
    assert_eq!(Preference::restore(None), Preference::Auto);
    assert_eq!(Preference::restore(Some("obsolete")), Preference::Auto);
    for preference in Preference::ALL {
        assert_eq!(Preference::restore(Some(preference.code())), preference);
        let saved = serde_json::to_string(&preference).unwrap();
        assert_eq!(serde_json::from_str::<Preference>(&saved).unwrap(), preference);
    }
    assert!(serde_json::from_str::<Preference>("\"unsupported\"").is_err());
    assert_eq!(serde_json::from_value::<crate::UiState>(json!({})).unwrap().interface_language, Preference::Auto);
    // Saved with the host's other preferences; a bad saved value keeps the current choice.
    let mut a = CadApp::new(Session::new(), Services::default());
    a.ui.interface_language = Preference::Ukrainian;
    let mut b = CadApp::new(Session::new(), Services::default());
    b.load_prefs(&a.prefs_json());
    assert_eq!(b.ui.interface_language, Preference::Ukrainian);
    b.load_prefs(r#"{"interfaceLanguage":"klingon"}"#);
    assert_eq!(b.ui.interface_language, Preference::Ukrainian);
}

#[test]
fn plural_forms_and_templates_preserve_inserted_user_data() {
    with_language("uk", || {
        for (n, form) in [
            (0, 2),
            (1, 0),
            (2, 1),
            (4, 1),
            (5, 2),
            (11, 2),
            (12, 2),
            (14, 2),
            (21, 0),
            (22, 1),
            (25, 2),
            (101, 0),
            (111, 2),
            (114, 2),
            (u64::MAX, 2),
        ] {
            assert_eq!(plural_uk(n), form, "{n}");
        }
        assert_eq!(count(1, "{n} contributor", "{n} contributors"), "1 учасник");
        assert_eq!(count(22, "{n} contributor", "{n} contributors"), "22 учасники");
        assert_eq!(count(11, "{n} contributor", "{n} contributors"), "11 учасників");
        assert_eq!(format("Current: {color}", &[("color", "{n} Open".into())]), "Поточний: {n} Open");
        assert_eq!(format("{b}/{a}", &[("a", "А".into()), ("b", "Б".into())]), "Б/А");
    });
    assert_eq!(t("Layers"), "Layers");
    with_language("de", || {
        assert_eq!(t("Layers"), "Layer");
        assert_eq!(count(1, "{n} contributor", "{n} contributors"), "1 Mitwirkender");
        assert_eq!(count(2, "{n} contributor", "{n} contributors"), "2 Mitwirkende");
        assert_eq!(count(11, "{n} contributor", "{n} contributors"), "11 Mitwirkende");
    });
    with_language("uk", || {
        assert_eq!(t("Layers"), "Шари");
        with_language("en", || assert_eq!(t("Layers"), "Layers"));
        assert_eq!(t("Layers"), "Шари");
    });
    assert_eq!(count(2, "{n} contributor", "{n} contributors"), "2 contributors");
}

#[test]
fn brazilian_portuguese_plurals_and_accents() {
    for (n, form) in [(0, 0), (1, 0), (2, 1), (5, 1), (21, 1), (u64::MAX, 1)] {
        assert_eq!(plural_pt(n), form, "{n}");
    }
    with_language("pt-BR", || {
        assert_eq!(count(1, "{n} contributor", "{n} contributors"), "1 colaborador");
        assert_eq!(count(3, "{n} contributor", "{n} contributors"), "3 colaboradores");
        assert_eq!(t("Layers"), "Camadas");
        assert_eq!(t("Dimension"), "Cota");
        assert_eq!(format("Height: {height}", &[("height", "{n}".into())]), "Altura: {n}");
    });
    assert_eq!(count(2, "{n} contributor", "{n} contributors"), "2 contributors");
}

#[test]
fn live_switch_to_brazilian_portuguese_translates_the_shell() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.in_window_menu = true;
    let ctx = egui::Context::default();
    app.logic(&ctx);
    app.start("line");
    app.run("ui.language", json!({"lang":"pt-BR"})).unwrap();
    let text = render(&mut app, &ctx);
    assert!(text.contains("Propriedades"), "{text}");
    assert!(text.contains("Arquivo"), "{text}");
    assert_eq!(app.language(), "pt-BR");
    assert_eq!(t("Properties"), "Properties", "language must not leak out of the render scope");
    app.run("ui.language", json!({"lang":"en"})).unwrap();
    assert!(render(&mut app, &ctx).contains("Properties"));
}

fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(s) => {
                text.push_str(s.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in shapes {
        collect(&shape.shape, &mut text);
    }
    text
}

fn render(app: &mut CadApp, ctx: &egui::Context) -> String {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    // Windows fade in; a second pass makes their text visible without a time dependency.
    let mut text = String::new();
    for _ in 0..2 {
        let mut output = ctx.run_ui(input.clone(), |ui| app.ui(ui));
        text = painted_text(&output.shapes);
        output.textures_delta.clear();
    }
    text
}

#[test]
fn live_switch_translates_the_shell_without_mutating_drawing_or_automation() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.in_window_menu = true;
    app.session.state_mut().unwrap().title = "Open".into();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    let before = render(&mut app, &ctx);
    assert!(before.contains("Properties"));
    let commands = crate::control::all_commands(&app);
    let (menu_request, _) = crate::ControlRequest::new("ui.menu.list", json!({}));
    let crate::control::Outcome::Done(menu_before) = crate::control::handle(&mut app, &ctx, &menu_request) else { panic!("expected menu reply") };
    app.start("line");
    let prompt = crate::control::cmdline_state(&app);
    app.run("ui.language", json!({"lang":"uk"})).unwrap();
    let after = render(&mut app, &ctx);
    assert!(after.contains("Властивості"), "{after}");
    assert!(after.contains("Файл"), "{after}");
    assert!(after.contains("Укажіть першу точку"), "{after}");
    assert!(after.contains("Open"), "the drawing title must remain unchanged");
    assert_eq!(crate::control::cmdline_state(&app), prompt);
    assert_eq!(crate::control::all_commands(&app), commands);
    let crate::control::Outcome::Done(menu_after) = crate::control::handle(&mut app, &ctx, &menu_request) else { panic!("expected menu reply") };
    assert_eq!(menu_before, menu_after);
    assert_eq!(app.session.state().unwrap().title, "Open");
    assert_eq!(t("Properties"), "Properties", "language must not leak out of the render scope");
    assert!(app.run("ui.language", json!({"lang":"bad"})).is_err());
    assert_eq!(app.language(), "uk");
    app.run("ui.resetpalettes", json!({})).unwrap();
    assert_eq!(app.language(), "uk");
    app.run("ui.language", json!({"lang":"en"})).unwrap();
    assert!(render(&mut app, &ctx).contains("Properties"));
    // The menu tree used by the control channel keeps its original labels and IDs.
    assert_eq!(crate::menus::tree(&app).first().unwrap().0, "File");
}

#[test]
fn dialogs_render_in_ukrainian_and_keep_their_source_ids() {
    for (dialog, heading) in [
        ("dsettings", "Налаштування креслення"),
        ("about", "Про CADCraft"),
        ("commands", "Довідник команд"),
        ("blocks", "Блоки"),
        ("layers", "Диспетчер властивостей шарів"),
        ("qselect", "Швидкий вибір"),
        ("parameters", "Диспетчер параметрів"),
        ("language", "Мова інтерфейсу"),
    ] {
        let mut app = CadApp::new(Session::new(), Services::default());
        app.ui.interface_language = Preference::Ukrainian;
        app.ui.dialog = Some(dialog.into());
        let ctx = egui::Context::default();
        app.logic(&ctx);
        let text = render(&mut app, &ctx);
        assert!(text.contains(heading), "{dialog}: {text}");
        assert_eq!(app.ui.dialog.as_deref(), Some(dialog));
    }
}

#[test]
fn default_fonts_cover_every_german_special_letter_in_the_catalog() {
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(Default::default(), |_| {});
    output.textures_delta.clear();
    let letters: BTreeSet<_> = include_str!("de.tsv").chars().filter(|c| ('\u{00c0}'..='\u{00ff}').contains(c)).collect();
    ctx.fonts_mut(|view| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let mut font = view.fonts.font(&family);
            for c in &letters {
                assert!(font.characters().contains_key(c), "missing {c} in {family:?}");
            }
        }
    });
}

#[test]
fn default_fonts_cover_every_ukrainian_letter_in_the_catalog() {
    let ctx = egui::Context::default();
    // Use only egui's bundled fonts, exactly as the web build does (no system-font loader).
    let mut output = ctx.run_ui(Default::default(), |_| {});
    output.textures_delta.clear();
    let letters: BTreeSet<_> = include_str!("uk.tsv").chars().filter(|c| ('\u{0400}'..='\u{04ff}').contains(c)).collect();
    ctx.fonts_mut(|view| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            // In egui 0.36, has_glyph can report a false negative for glyphs in the same
            // face as the replacement character. Check the family's actual character maps.
            let mut font = view.fonts.font(&family);
            for c in &letters {
                assert!(font.characters().contains_key(c), "missing {c} in {family:?}");
            }
        }
    });
}

#[test]
fn czech_locales_preferences_and_plural_forms() {
    for tag in ["cs", "cs-CZ", "cs_CZ.UTF-8", "CS-cz", " cs-CZ ", "cs_CZ@euro"] {
        assert_eq!(locale_language(tag), Some("cs"), "{tag}");
    }
    for tag in ["cz", "csharp", "csomething"] {
        assert_eq!(locale_language(tag), None, "{tag}");
    }
    assert_eq!(Preference::parse("cs"), Some(Preference::Czech));
    assert_eq!(Preference::Czech.name(), "Čeština");
    assert_eq!(Preference::Czech.resolve(&["en-US".into()]), "cs");
    assert_eq!(Preference::Auto.resolve(&["pl-PL".into(), "cs-CZ".into(), "en-US".into()]), "cs");
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.interface_language = Preference::Czech;
    let mut restored = CadApp::new(Session::new(), Services::default());
    restored.load_prefs(&app.prefs_json());
    assert_eq!(restored.ui.interface_language, Preference::Czech);
    with_language("cs", || {
        for (n, form) in [(0, 2), (1, 0), (2, 1), (4, 1), (5, 2), (11, 2), (21, 2), (22, 2), (25, 2), (u64::MAX, 2)] {
            assert_eq!(plural_cs(n), form, "{n}");
            let contributor = ["přispěvatel", "přispěvatelé", "přispěvatelů"][form];
            let commit = ["commit", "commity", "commitů"][form];
            assert_eq!(count(n, "{n} contributor", "{n} contributors"), format!("{n} {contributor}"));
            assert_eq!(count(n, "{n} commit", "{n} commits"), format!("{n} {commit}"));
        }
        assert_eq!(format("Current: {color}", &[("color", "{n} Open".into())]), "Aktuální: {n} Open");
    });
}

#[test]
fn czech_menu_headings_keep_object_labels_and_other_catalogs() {
    with_language("cs", || {
        assert_eq!(menu("View"), "Zobrazení");
        assert_eq!(t("View"), "Pohled");
        assert_eq!(menu("Dimension"), "Kótování");
        assert_eq!(t("Dimension"), "Kóta");
        assert_eq!(menu("Draw"), "Kreslení");
        assert_eq!(menu("Modify"), "Modifikace");
        assert_eq!(menu("Edit"), "Úpravy");
        assert_eq!(menu("File"), "Soubor");
    });
    for language in ["en", "uk", "de", "es", "pt-BR", "unknown"] {
        for source in crate::menus::MENUS {
            assert_eq!(tr_menu(language, source), tr(language, source));
        }
    }
}

#[test]
fn live_czech_switch_preserves_drawing_command_and_saved_choice() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.in_window_menu = true;
    app.session.state_mut().unwrap().title = "Open".into();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    assert!(render(&mut app, &ctx).contains("Properties"));
    app.start("line");
    let prompt = crate::control::cmdline_state(&app);
    let commands = crate::control::all_commands(&app);
    app.run("ui.language", json!({"lang":"cs"})).unwrap();
    let text = render(&mut app, &ctx);
    for label in ["Vlastnosti", "Soubor", "Zobrazení", "Kreslení", "Kótování", "Určete první bod"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    assert_eq!(app.language(), "cs");
    assert_eq!(app.session.state().unwrap().title, "Open");
    assert_eq!(crate::control::cmdline_state(&app), prompt);
    assert_eq!(crate::control::all_commands(&app), commands);
    assert_eq!(t("Properties"), "Properties");
    app.run("ui.resetpalettes", json!({})).unwrap();
    assert_eq!(app.language(), "cs");
    app.run("ui.language", json!({"lang":"en"})).unwrap();
    assert!(render(&mut app, &ctx).contains("Properties"));
}

#[test]
fn czech_dialogs_render_with_stable_ids() {
    for (dialog, heading) in [
        ("dsettings", "Nastavení kreslení"),
        ("about", "O aplikaci CADCraft"),
        ("commands", "Přehled příkazů"),
        ("blocks", "Bloky"),
        ("layers", "Správce vlastností hladin"),
        ("qselect", "Rychlý výběr"),
        ("parameters", "Správce parametrů"),
        ("language", "Čeština"),
        ("options:preferences", "Uživatelské předvolby"),
    ] {
        let mut app = CadApp::new(Session::new(), Services::default());
        app.ui.interface_language = Preference::Czech;
        app.ui.dialog = Some(dialog.into());
        let ctx = egui::Context::default();
        app.logic(&ctx);
        let text = render(&mut app, &ctx);
        assert!(text.contains(heading), "{dialog}: {text}");
        assert_eq!(app.ui.dialog.as_deref(), Some(dialog));
    }
}

#[test]
fn default_fonts_cover_czech_letters_and_typographic_quotes() {
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(Default::default(), |_| {});
    output.textures_delta.clear();
    let letters: BTreeSet<_> = "ÁČĎÉĚÍŇÓŘŠŤÚŮÝŽáčďéěíňóřšťúůýž„“".chars().collect();
    ctx.fonts_mut(|view| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let mut font = view.fonts.font(&family);
            for c in &letters {
                assert!(font.characters().contains_key(c), "missing {c} in {family:?}");
            }
        }
    });
}

#[test]
fn russian_and_french_plurals() {
    with_language("ru", || {
        for (n, word) in [(1, "участник"), (2, "участника"), (5, "участников"), (21, "участник"), (22, "участника"), (11, "участников")]
        {
            assert_eq!(count(n, "{n} contributor", "{n} contributors"), format!("{n} {word}"));
        }
    });
    with_language("fr", || {
        for (n, word) in [(0, "contributeur"), (1, "contributeur"), (2, "contributeurs"), (10, "contributeurs")] {
            assert_eq!(count(n, "{n} contributor", "{n} contributors"), format!("{n} {word}"));
        }
    });
}

/// Chinese shows only with an installed CJK font (none is bundled); without one the interface
/// stays English instead of drawing boxes, and choosing it says why.
#[test]
fn chinese_needs_an_installed_cjk_font() {
    assert_eq!(locale_language("zh-CN"), Some("zh"));
    let mut app = CadApp::new(Session::new(), Services::default());
    let r = app.run("ui.language", json!({"lang": "zh"})).unwrap();
    assert_eq!(r["interfaceLanguage"], "zh");
    let font = crate::theme::cjk_font();
    assert_eq!(app.language(), if font.is_some() { "zh" } else { "en" });
    assert_eq!(r["shown"], app.language());
    assert!(crate::theme::can_show("uk") && crate::theme::can_show("en"));
    if font.is_none() {
        assert!(app.status.as_ref().is_some_and(|(s, _)| s.contains("No Chinese font")));
    }
}
