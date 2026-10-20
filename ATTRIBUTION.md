# Asset attribution

Every non-code asset in this repository (images, icons, fonts, example drawings, presets) is listed
here with its author, source and licence. `cargo xtask assets` (part of `cargo xtask ci`) fails if
an asset file is missing from this table.

**Policy (mandatory):** CADCraft contains **no Autodesk, Adobe or Avid iconography, images,
artwork, fonts, hatch patterns, linetypes, templates or presets.** Every asset is original work by
CADCraft contributors or third-party material under an open licence (OSI open source, public domain
/ CC0, or Creative Commons that allows redistribution). Screenshots of Autodesk software are never
committed. Font files are not added here: shared fonts live in
[storytold/craft-fonts](https://github.com/storytold/craft-fonts), an optional build input.

Generated-in-code assets are original and have no file to list:
- the UI icon set (`crates/ui-egui/src/icons.rs`);
- the "CADCraft Stroke" single-stroke drafting font (`crates/fonts/src/stroke.rs`);
- the standard linetype and hatch-pattern libraries (`crates/doc/src/library.rs`; industry-common
  names, our own dash/spacing values);
- the colour index palette, generated from its structure (`crates/color/src/lib.rs`);
- the sample drawings (`crates/engine/src/sample.rs`);
- the 3D furniture and fixture library (`crates/doc/src/library/models3d.rs`; built in code from
  boxes, prisms and cylinders at common real sizes, no outside models).

| Asset | Author | Source | Licence | Notes |
|---|---|---|---|---|
| `assets/app-icon/cadaddy-icon.svg` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark from a concept sketch made with Grok Imagine, redrawn by hand) | MIT OR Apache-2.0 | Icon source |
| `assets/app-icon/cadaddy-icon-small.svg` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark from a concept sketch made with Grok Imagine, redrawn by hand) | MIT OR Apache-2.0 | Icon source, 32 px and below |
| `assets/app-icon/cadaddy-icon-macos.svg` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark from a concept sketch made with Grok Imagine, redrawn by hand) | MIT OR Apache-2.0 | Icon source, macOS grid |
| `assets/app-icon/hicolor/128x128/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/16x16/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/24x24/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/256x256/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/32x32/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/48x48/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/512x512/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/hicolor/64x64/apps/dev.aicodeforge.cadaddy.png` | CADaddy contributors (hexagon icon, 2026-10-17) | rendered from `assets/app-icon/cadaddy-icon*.svg` (packaging/icons.sh) | MIT OR Apache-2.0 | Linux hicolor icon |
| `assets/app-icon/cadaddy.icns` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (packaging/icons.sh) | MIT OR Apache-2.0 | macOS icon |
| `assets/app-icon/cadaddy.ico` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (packaging/icons.sh) | MIT OR Apache-2.0 | Windows icon |
| `assets/app-icon/cadaddy-64.png` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (packaging/icons.sh) | MIT OR Apache-2.0 | |
| `assets/app-icon/cadaddy-256.png` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (packaging/icons.sh) | MIT OR Apache-2.0 | |
| `assets/app-icon/cadaddy-1024.png` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (packaging/icons.sh) | MIT OR Apache-2.0 | |
| `assets/app-icon/cadaddy-macos-512.png` | CADaddy contributors (hexagon icon, 2026-10-17) | generated (macOS margin variant) | MIT OR Apache-2.0 | |
| `assets/spell/en_US.txt` | Kevin Atkinson (English Speller Database / SCOWL), from public-domain 12dicts and ENABLE2K word lists (Alan Beale and others) | generated at https://app.aspell.net/create (size 60, US spelling, variant level 1, roman numerals, diacritics stripped), 2026-10-18; ESDB revision 1e5b7d3 | ESDB/SCOWL licence (permissive: use, copy, modify, distribute and sell, with the copyright and permission notice kept, which is the file's header) | English word list for SPELL |
| `packaging/macos/dmg/background.svg` | @XusBadia | original work for this repository (2026-10-08; CADaddy title and icon layout 2026-10-17), "CADaddy macOS DMG window background"; derived from the CADCraft app icon (`assets/app-icon/`, referenced via `<image>`, not copied); text is outlined Inter / JetBrains Mono glyph paths (OFL fonts the project already uses); no third-party material modified | MIT OR Apache-2.0 (same as the code: `LICENSE-MIT`, `LICENSE-APACHE`) | Finder window background of the macOS DMG; `packaging/macos/dmg/generate.py` renders it to `background.tiff` (see `packaging/macos/dmg/README.md`) |
| `packaging/macos/dmg/background.tiff` | @XusBadia | Rendered from `packaging/macos/dmg/background.svg` by `packaging/macos/dmg/generate.py` | MIT OR Apache-2.0 (same as the code: `LICENSE-MIT`, `LICENSE-APACHE`) | 1x + 2x HiDPI TIFF for the macOS DMG window; includes the app icon (see its row) and LittleCMS's built-in sRGB profile ("No copyright, use freely") |
| `examples/apartment.dxf` | CADCraft contributors | sample drawing made with cadcraft-cli commands | MIT OR Apache-2.0 | Original |
| `crates/io/src/cdy/v1_sample.cdy` | CADaddy contributors | test drawing (three objects) written by the first .cdy writer from test code (`write_v1_sample`) | FSL-1.1-ALv2 | Original |
| `crates/dwg/tests/regress/ac18-section-size.dwg` | CADaddy contributors | fuzzer mutation of a DWG CADaddy wrote from its own every-object test drawing (`cargo xtask fuzz dwg`); a regression input | FSL-1.1-ALv2 | Original |
| `assets/materials/library.json` | CADaddy contributors | original material recipes (colours, patterns, sizes); CADaddy generates their textures in code (`crates/render/src/materials.rs`), no images | CC0-1.0 | Material library |
| `docs/brand/cadaddy-logo-dark.png` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark from a concept sketch made with Grok Imagine, redrawn by hand) plus the CADaddy wordmark set in Inter Bold (Rasmus Andersson, SIL OFL 1.1) and converted to outlines | MIT OR Apache-2.0 | README logo, dark theme |
| `docs/brand/cadaddy-logo-light.png` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark from a concept sketch made with Grok Imagine, redrawn by hand) plus the CADaddy wordmark set in Inter Bold (Rasmus Andersson, SIL OFL 1.1) and converted to outlines | MIT OR Apache-2.0 | README logo, light theme |
| `docs/brand/cadaddy-mark.svg` | CADaddy contributors (2026-10-17) | original vector drawing (hexagon mark, source for the logo) | MIT OR Apache-2.0 | Vector source |
| `docs/images/ui-apartment.png` | CADCraft contributors | screenshot of CADCraft itself (examples/apartment.dxf) | MIT OR Apache-2.0 | Original; no Autodesk UI |
| `docs/images/ui-layout.png` | CADCraft contributors | screenshot of CADCraft itself (a layout of examples/apartment.dxf) | MIT OR Apache-2.0 | Original; no Autodesk UI |
| `docs/images/ui-bracket.png` | CADCraft contributors | screenshot of CADCraft itself (sample drawing generated in code) | MIT OR Apache-2.0 | Original; no Autodesk UI |

## Interface translations

| Asset | Author | Source | Licence | Notes |
|---|---|---|---|---|
| `crates/ui-egui/src/i18n/uk.tsv` | @dmatviichuk | Original Ukrainian translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/de.tsv` | the CADCraft contributors | Original German translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/es.tsv` | @PedroGuille | Original Spanish translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/pt_br.tsv` | @WladmirJunior | Original Brazilian Portuguese translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/cs.tsv` | @Losminek (AI-assisted) | Original Czech translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/ru.tsv` | @pechnikov2026-star | Original Russian translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/fr.tsv` | @pechnikov2026-star | Original French translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/zh.tsv` | @mayloglog | Original Simplified Chinese translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/sv.tsv` | Erik Holmbom (@holmbom) with Claude Code (AI-assisted) | Original Swedish translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied; terminology and the running app reviewed by a native speaker |
| `crates/ui-egui/src/i18n/it.tsv` | Emanuele Bottelli (AI-assisted) | Original Italian translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/src/i18n/ko.tsv` | @huk082 (AI-assisted) | Original Korean translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied; no font assets added |
| `crates/ui-egui/src/i18n/ja.tsv` | AICodeForge (AI-assisted) | Original Japanese translations of the English interface strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied; no font assets added |
| `crates/ui-egui/src/i18n/ar.tsv` | @hasan-sdaif | Original Arabic translations of CADCraft English strings | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | No proprietary localization resources copied |
| `crates/ui-egui/tests/golden/` | CADaddy contributors (generated) | renders of CADaddy's own interface by `crates/ui-egui/tests/ui_shots.rs` (half size, grey) | MIT OR Apache-2.0 | UI screenshot regression goldens; our own renders only, never screenshots of other software |
