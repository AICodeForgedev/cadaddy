# Interface languages

Choose **Edit → Interface Language** and select **Auto (system language)**, **English**,
**Українська**, **Português (Brasil)**, **Deutsch**, **Español**, **Čeština** (Czech), **Русский**, **Français**, **简体中文**, **Italiano**, **한국어** (Korean), **Svenska** (Swedish), or **العربية** (Arabic).
The interface updates without restarting, including the native macOS menus.
The choice is saved with the other interface preferences (`cadcraft.prefs`, beside the theme):
eframe storage on the desktop, browser local storage on the web. Installations without a saved
choice use Auto.

Auto uses the first supported language in the operating system's preferred locales, or the
browser's `navigator.languages` on the web. BCP 47 and POSIX tags select their primary language:
`uk-UA` → `uk`, `de_AT.UTF-8` → `de`, `es-MX` → `es`, `pt_BR.UTF-8` → `pt-BR`,
`cs-CZ` and `cs_CZ.UTF-8` → `cs`, `it-IT` and `it_CH.UTF-8` → `it`, `ko-KR` and `ko_KR.UTF-8` → `ko`,
`sv-SE`, `sv-FI` and `sv_SE.UTF-8` → `sv`. Unsupported languages fall back to English.
Set `CADCRAFT_LOCALE=cs-CZ` to override the desktop's locale for a launch.
An explicit saved language choice takes precedence over that override.

Automation can select the language through the existing control channel:

```json
{"id":1,"method":"engine.execute","params":{"command":"ui.language","params":{"lang":"cs"}}}
```

The `ui.set` method also accepts `{"interfaceLanguage":"cs"}`. Supported values are `auto`,
`en`, `uk`, `pt-BR`, `de`, `es`, `cs`, `ru`, `fr`, `zh`, `it`, `ko`, `sv`, and `ar`; invalid values are rejected without changing the current
selection. Resetting palettes keeps the language choice.

## Translation coverage

The catalogs cover command and menu labels, tool groups and tooltips, the Start page,
file and model-space tabs, status controls, object snaps, drafting prompts, property labels,
dialogs, and the About window's controls. The command reference can be searched by English
command ID, English label, or translated label. Ukrainian has enforced coverage;
missing entries in the other catalogs are reported by tests and fall back to English.

Czech has 1,214 catalog rows (counted 2026-10-13), covering the reference catalog and every marked UI
literal when it landed, including Options. Four `menu` entries distinguish menu headings from object/command
labels: **Zobrazení / Pohled**, **Kreslení / Kreslit**, **Kótování / Kóta**, and **Modifikace / Upravit**.
The translation is AI-assisted work by @Losminek, reviewed against Czech desktop and CAD UI
conventions and an independent review pass; it has not yet been proofread by a native speaker.

Korean has **1,235 rows**: 1,229 ordinary labels, four contextual menu headings, and
two single-form count templates. It covers the union of the existing catalogs, including
the Ukrainian reference, the current Czech catalog, Options, prompts, dialogs, and tooltips.
The translations are original, AI-assisted Korean CAD terminology (for example, 도면층,
치수, 모깎기, 모따기, 간격띄우기 and 객체 스냅). Independent native proofreading and
platform-specific layout/IME validation remain follow-up work.

Brazilian Portuguese has 1,240 catalog rows and also covered the reference catalog and every marked UI
literal when it landed (#791): commands, menus, tool groups and tooltips, object snaps, drafting prompts, dialogs and
Preferences. Its plural rule is `one|other` (0 and 1 are `one`), as CLDR defines
for `pt`, and the catalog's accented letters (`Á Â Í Ó Ú à á â ã ç é ê í ó ô õ ú`) render in the
bundled fonts. The original catalog was contributed by @WladmirJunior; the remaining rows were completed
by @RicSchonfelder with AI assistance and have not yet been proofread by a native speaker.

The other catalogs, with their rows counted on 2026-10-13: Ukrainian 1,322 (the reference), Spanish 1,239,
Italian 1,231, Swedish 1,235, Arabic 1,229, Simplified Chinese 1,112, German 1,030, Russian 1,004 and French 1,003.
Strings added to the Ukrainian catalog since a language's last update (for example the 3D Mouse dialog and the
Block Editor prompt) show in English there; [localization-parity.md](localization-parity.md) lists the count per language.
Arabic uses the six CLDR plural forms and a font chain for Arabic script (Noto Sans Arabic, Segoe UI/Tahoma, Geeza Pro);
the interface is not mirrored. CJK interface fonts load in every language, so Chinese, Japanese and Korean file, layout
and layer names don't show as boxes (#861).

Drawing titles, layer/layout/block/style names, parameter names and expressions, text in a
drawing, and command-line input are user data and retain their original spelling. Clickable
command keywords retain their English spelling so the text shown matches the input accepted
by the engine. CLI/MCP/control responses, engine diagnostics and command history remain
English. Uncatalogued text also falls back to English. Number formatting and the operating
system's own file-picker controls follow the existing behavior.

The bundled egui fonts cover Ukrainian (`Ґґ Єє Іі Її`), German (`ÄÖÜ äöü ß`), and Czech
(`ÁČĎÉĚÍŇÓŘŠŤÚŮÝŽ áčďéěíňóřšťúůýž`, including typographic quotes `„…“`).
The translations add no font assets and work on the web without system fonts.

Korean needs a font that covers **every Hangul character in its catalog** and the language
picker label. The CJK font loaded in every language (Noto Sans CJK, Microsoft YaHei, PingFang)
usually covers Hangul already, and then nothing more is loaded. Otherwise, only when Korean is
chosen, desktop builds look for a `Hang`/`Kore` face from the optional `CRAFT_FONTS_DIR` input,
then installed fonts: Malgun Gothic/Gulim on Windows, Apple SD Gothic Neo/AppleGothic on macOS,
and Nanum Gothic or Noto CJK on Linux/FreeBSD. Collection faces are checked individually, and
file reads/face scans are bounded. No system fonts or new font assets are redistributed.

Web builds cannot read installed fonts and embed no Korean face (to stay within hosting size
limits), so the web interface stays English when Korean is chosen. Without a font covering the
catalog, the saved `ko` preference is kept but the interface falls back to English; the language
command reports that a font is needed, and the picker uses the readable label **Korean** wherever
Hangul can't be shown. This change does not fix platform IME issues or localize engine output.

## Adding translations

The implementation follows the display-time catalog pattern used by
[VectorCraft #205](https://github.com/storytold/vectorcraft/pull/205),
[PdfCraft #106](https://github.com/storytold/pdfcraft/pull/106), and
[FilmCraft #238](https://github.com/storytold/filmcraft/pull/238). Its code is local to CADCraft.

- `crates/ui-egui/src/i18n/*.tsv` are UTF-8 with three tab-separated columns:
  `context`, `English source`, and `translation`. Ordinary labels have an empty context.
  Optional `menu` entries translate menu headings via `i18n::menu` / `i18n::tr_menu`;
  without one, the ordinary translation is used.
- `@plural` entries hold the language's forms for integer counts: Ukrainian `one|few|many`,
  German, Spanish, Italian, Swedish and Simplified Chinese `one|other`, Brazilian Portuguese and French `one|other` (0 and 1 are `one`),
  Russian `one|few|many` (as Ukrainian), Czech `one|few|other` (1; 2–4; everything else, including 0, 11, 21 and 22).
  Korean has one form (`other`) for all integer counts; Arabic has six (`plural_arabic`).
  (The rules are the `plural` and `plural_forms` fields of `LANGUAGES` in `i18n/mod.rs`.)
  The API takes `u64`, so fractional counts are not accepted.
- Register a new catalog in `i18n::LANGUAGES`, add its preference variant and code/name,
  and set its integer plural rule and form count on the `Language` row.
- Use `crate::tl!("English label")` for a literal, `i18n::t(label)` for a known dynamic UI
  label, and `crate::tf!("Version {version}", version = value)` for named placeholders.
  Translate only at the display boundary; never look up arbitrary user names or identifiers.
- Windows retain explicit IDs based on their English source, and tool groups retain their
  source names as keys, so switching languages preserves their state.
- Add a translation attribution row in `ATTRIBUTION.md`, update the language lists, and add
  locale, plural, font-glyph and live-switch tests. Add the catalog to `REPORTED` in
  `i18n/tests.rs`: only Ukrainian, the reference translation, is in `REQUIRED`, so adding a
  command needs a row in one language, not in all of them.
- Catalogs are embedded and parsed once. Missing or malformed rows cannot crash the app;
  tests reject invalid rows, duplicate keys, changed placeholders, and mismatched ellipses.
- `cargo test -p cadcraft-ui-egui i18n` checks menu/command/tool/snap coverage, every marked
  UI literal, locale resolution, preferences, integer plurals, bundled glyphs and actual
  egui renders with language switching. `cargo xtask assets` checks asset attribution;
  `cargo xtask ci` runs the workspace quality gates.

Translations are original work under `MIT OR Apache-2.0`, written from the meaning of
CADCraft's English strings. Do not copy proprietary localization resources.
