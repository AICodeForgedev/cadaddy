# Compare page review

Status: reviewed 20 October 2026 against the live pages <https://aicodeforge.dev/free-autocad-alternative>
(the compare page, "Updated 20 October 2026 (CADaddy 0.7.8)") and <https://aicodeforge.dev/llms.txt>.
Claims about other programs were checked only against each project's own current official pages;
claims about CADaddy against `docs/compare-facts.json` (0.7.8), `docs/website-claims.json` and the
repository. No other program's source code was read.

The coordinator updates the live page from this file; builders don't touch the server.

## Changes to make, most important first

1. **nanoCAD Free commercial use is no longer "unclear".** The support article that allowed commercial
   use now returns 404. Every current official page says non-commercial only. Change the table cell to
   "No (non-commercial only)" and fix the Weaker text, the "commercial work" FAQ answer and the "best
   free alternative" FAQ answer (details in the nanoCAD section).
2. **QCAD: "scripting is in the paid edition" is wrong.** The ECMAScript interface is ticked for QCAD
   CE on qcad.org's features table.
3. **QCAD Community has no parts library.** "Over 5000 CAD parts" and the Library Browser are ticked
   only for Pro and QCAD/CAM.
4. **nanoCAD Free: be specific about DWG and Windows.** It handles DWG up to 2013 only (from its FAQ),
   and its system requirements list Vista/7/8, with "We can't guarantee its compatibility with the
   Windows OS 10/11".
5. **LibreCAD saving DWG is changing.** The development line (2.2.2 alpha, master README) has a
   `dxf2dwg` command-line converter that writes DWG R2000–R2018 when built with `DWGSUPPORT`. The
   released app (2.2.1.6) still saves DXF only. The table is right for today, but re-check when
   2.2.2 ships, because "the only option here that saves DWG" would then need a qualifier.
6. **The Homebrew cask is stale.** `aicodeforgedev/tap` still installs 0.7.5. It updates itself
   afterwards, but llms.txt says the current version is 0.7.8. Bump the cask on each release, or
   add "(installs the last cask version, then offers the update)" next to the brew line.
7. **The llms.txt summary leaves out Linux.** It says "for Windows and Mac", while the section heading,
   downloads and compare page all include Linux.
8. Small wording fixes: QCAD's licence has exceptions, LibreCAD "free for any use", FreeCAD "most
   capable", and FreeCAD's DWG converter list (see the tables).

## LibreCAD

Latest: 2.2.1.6 (stable, 17 Oct 2026); "2.2.1-latest" release candidate, 18 Oct 2026; 2.2.2 alpha
continuous builds (Windows x64/arm64, universal macOS, Linux x86_64/aarch64).

| Claim (page) | Source | Still true? | Suggested wording |
|---|---|---|---|
| Open DWG: "Some files"; "it can read some DWG files" | <https://dokuwiki.librecad.org/> ("Partial support of Autodesk DWG format (importing only)"); <https://dokuwiki.librecad.org/doku.php/usage:faq> ("The support of DWG format by LibreCAD is basic") | Yes | keep |
| Save DWG: "No, saves DXF"; "it cannot save DWG" | same; <https://github.com/LibreCAD/LibreCAD> README (master): `dxf2dwg`, "DWG input and output require a build with `DWGSUPPORT` enabled" | Yes for the released app; the development line adds a command-line DWG writer | keep; re-check when 2.2.2 is released |
| Windows, Mac, Linux | <https://librecad.org/> ("for Windows, Apple and Linux"); <https://github.com/LibreCAD/LibreCAD/releases> | Yes (Apple-silicon builds since 2.2.1.3) | keep |
| Commercial use: Yes (GPLv2) | <https://librecad.org/> ("Open Source and GPLv2") | Yes | keep |
| "The best-known open-source 2D CAD program, around for well over a decade, with a large community and many translations" | <https://docs.librecad.org/en/latest/appx/licenses.html> (first commit 2010); librecad.org ("translated in over 30 languages", "large, dedicated community") | Yes, though "best-known" is our opinion | "A widely used open-source 2D CAD program, developed since 2010, with a large community and 30+ languages." |
| Strong: "light, stable, cross-platform, free for any use" | librecad.org | Mostly. "Light" and "stable" are opinion; GPLv2 places conditions on redistribution, not on use | "light, cross-platform, free for any use (GPLv2)" is fine; "free to use for any work" is more exact |
| "DXF is its native format" | <https://dokuwiki.librecad.org/> (DXF "is LibreCAD main format") | Yes | keep |

## QCAD Community Edition

Latest: QCAD 3.33.1 (CE and Pro share version numbers), 8 Sep 2026 — <https://qcad.org/en/changelog/>.

| Claim (page) | Source | Still true? | Suggested wording |
|---|---|---|---|
| Open DWG / Save DWG: "Paid edition only" | <https://www.qcad.org/en/features/> ("DWG and DXF: read and write support for format versions" is ticked for Pro and CAM, not CE); <https://www.qcad.org/en/faq/> | Yes | keep |
| Windows, Mac, Linux | <https://github.com/qcad/qcad> README ("for Windows, macOS and Linux"); FAQ (macOS 10.10+, Intel and Apple silicon) | Yes | keep |
| Commercial use: Yes (GPLv3) | <https://qcad.org/en/license/> ("distributed under the GPLv3 with exceptions to allow commercial plugins and script extensions") | Yes, but the licence is GPLv3 with exceptions | "Yes (GPLv3, with exceptions for plugins)" |
| "a clean interface and a large parts library" | features page: "Over 5000 CAD parts available as free add-ons" and "Insert parts into the drawing" are ticked only for Pro and CAM | **No** for the Community Edition | "A long-running 2D CAD program with a clean interface. The Community Edition is the free, open-source core; the paid Professional edition adds DWG and a library of over 5,000 parts." |
| Strong: "precise, well-documented 2D drafting in DXF; free for any use" | features page: "DXF version R15 (2000) [QCAD Community Edition]"; other DXF versions are Pro | Mostly; the free edition writes only DXF R2000 | "precise, well-documented 2D drafting in DXF (the 2000 format in the free edition); free for any use" |
| Weaker: "DWG support, scripting and some tools are in the paid QCAD Professional edition" | features page: the "ECMAScript Interface" rows are ticked for CE, Pro and CAM; polyline offset, viewports and layouts are Pro-only | **No**, scripting is in CE | "DWG, newer DXF versions, the parts library and some tools (polyline offset, layouts with viewports) are in the paid QCAD Professional edition (about US$48, one-off)." |
| "Paid tiers ... (QCAD Professional) add more" | <https://qcad.org/prestashop/en/qcad/20-qcad-professional.html> (US$48, perpetual single-user licence) | Yes | keep |

## FreeCAD

Latest: 1.1.4 (28 Sep 2026), <https://blog.freecad.org/2026/09/28/freecad-1-1-4-released/>; 26.3 release
candidate 2 (18 Oct 2026, new calendar numbering, final expected about November 2026),
<https://blog.freecad.org/2026/10/18/freecad-26-3-release-candidate-2/>. Minimums on
<https://www.freecad.org/downloads.php>: Windows 10, macOS 12.

| Claim (page) | Source | Still true? | Suggested wording |
|---|---|---|---|
| Open / Save DWG: "With add-on converter"; "DWG needs a separate converter (the ODA File Converter or LibreDWG) set up in preferences" | wiki "FreeCAD and DWG Import" (<https://wiki.freecad.org/FreeCAD_and_DWG_Import>; the wiki blocks scripted fetches, so it was read through the official mirror <https://github.com/FreeCAD/FreeCAD-documentation/blob/main/wiki/FreeCAD_and_DWG_Import.md>) | Yes, but the list is incomplete: QCAD Pro's `dwg2dwg` (paid) also works, a converter on the system path is found without setup, and files go through DXF | "DWG needs a separate converter (LibreDWG, the ODA File Converter or QCAD Pro's dwg2dwg), which goes through DXF" |
| Windows, Mac, Linux | <https://www.freecad.org/> ("Windows, Mac and Linux") | Yes | keep |
| Commercial use: Yes (LGPL) | wiki "License" ("LGPL2+", "can be used freely for any kind of purpose, being private, commercial or institutional") | Yes | "Yes (LGPL 2+)" (optional) |
| "parametric 3D modeller with workbenches for parts, architecture (BIM), drafting and more" | <https://www.freecad.org/features.php> (BIM workbench built in since 1.0) | Yes | keep |
| Strong: "the most capable free tool for 3D design; huge feature set; scriptable in Python" | features page (Python) | Python yes; "most capable" is our superlative | "one of the most capable free 3D design tools" |
| Weaker: "heavier and different to learn if you think in AutoCAD 2D" | — | Opinion, and fair | keep |

## nanoCAD Free

The current product is "nanoCAD Free nanoCAD v.5", still downloadable (one 32-bit .exe). The paid
platform is nanoCAD 26 (March 2026).

| Claim (page) | Source | Still true? | Suggested wording |
|---|---|---|---|
| "The free, legacy version 5 of the nanoCAD platform ... native DWG" | <https://nanocad.com/products/nanocad-free/> ("a legacy version 5 of the platform"); <https://nanocad.com/products/nanocad-free/download/> | Yes | keep |
| Open / Save DWG: "Yes, older versions"; "doesn't support the newest DWG versions" | <https://nanocad.com/support/faq/nanocad-free/> ("nanoCAD Free supports DWG file versions up to 2013 only") | Yes | "Yes, up to DWG 2013"; Weaker: "opens and saves DWG only up to the 2013 format, so newer (2018-format) files need the paid version" |
| Windows only; "(32-bit)" | download page (one 32-bit installer); <https://nanocad.com/support/system-requirements/nanocad-free/> ("MS Windows 8, MS Windows 7, MS Windows Vista (SP1 or later)"); FAQ ("We can't guarantee its compatibility with the Windows OS 10/11"); <https://support.nanocad.com/KB/View/66665060-nanocad-for-mac-linux> (Windows only) | Yes, and Windows 10/11 is officially unsupported | table: "Windows (7/8; 10/11 not guaranteed)"; Weaker: "Windows only (32-bit), and the maker doesn't guarantee Windows 10 or 11" |
| Commercial use: "Unclear: maker's pages conflict"; "the maker's FAQ says non-commercial use only while its support site says commercial use is allowed after registration" | FAQ ("nanoCAD Free is intended for non-commercial use"); product page ("completely free to non-commercial use", "suitable for both individual and business use on non-profit projects"); <https://support.nanocad.com/KB/View/66718631-using-nanocad-5-for-commercial-purposes> now returns **HTTP 404** | **No.** The conflicting page is gone and the maker's pages now agree | table: "No (non-commercial only)"; Weaker: "free for non-commercial use only (the maker's FAQ and product page)"; FAQ answer: "nanoCAD Free is for non-commercial use only, per the maker's FAQ and product page"; "best alternative" answer: "...but is Windows-only and for non-commercial use only"; short answer: drop "(check its licence before paid work)" and say "for non-commercial work" |
| "needs registration and a yearly (free) licence renewal" | product page ("Registration and yearly license renewal are entirely free"); FAQ (one computer per licence) | Yes | optional: "...renewal, one computer per licence" |
| "Paid tiers of ... the current nanoCAD platform add more" | <https://nanocad.com/company/news/nanocad-26-release-smarter-tools-enhanced-precision-and-a-more-intuitive-experience/> | Yes (nanoCAD 26) | optional: "(QCAD Professional, nanoCAD 26)" |

Commercial use is the main thing that changes the comparison, and the page currently gives nanoCAD
the benefit of the doubt. After the fix, "can I use it for paid work" is answered for all five programs.

## CADaddy

Facts: `docs/compare-facts.json` (version 0.7.8, 441 typed commands, DWG R2000–R2018 written and read
back, 31 MCP tools, FSL-1.1-ALv2, macOS minimum 11.0, installer kinds msi/dmg/AppImage/deb). Live
release v0.7.8 (20 Oct 2026) carries every file the page links: `CADaddy-Windows.msi`, `CADaddy-Mac.dmg`,
`CADaddy-Linux.AppImage`, `CADaddy-Linux.deb`, `CADaddy.mcpb`, `SHA256SUMS`, `SHA256SUMS.sig`.

Every claim already listed in `docs/website-claims.json` still holds, and `apps/cadaddy-cli/tests/compare_facts.rs`
checks the ones it can. Claims that were checked by hand and are not in that file:

| Claim (page) | Source | Still true? | Suggested wording / action |
|---|---|---|---|
| Open/Save DWG: "Yes" | compare-facts `versions.dwg` = R2000…R2018; `crates/dwgcodec/src/lib.rs` refuses R13/R14 ("not read yet") | Yes for DWG 2000–2018 (2018 is AutoCAD's current format); pre-2000 DWG is not read | optional, to match the nanoCAD row: "Yes (2000–2018 formats)" |
| "Since 0.7.7 it also does 3D: solids, materials, rendering and walking through the building, drafting a 3D model from the plans, and IFC export" | 0.7.7 release commit (3D solids, materials, lights, rendering, walk mode, BIM walls); `model.build`, `walk3d` crate | Yes | keep; add as a manual row in website-claims.json |
| "Windows 10, 11" | website-claims.json (manual) | Yes (tested by hand on releases) | keep |
| "Per-user install, no administrator needed" (llms.txt) | `packaging/windows/cadaddy.wxs` `Scope="perUser"` | Yes | add to website-claims.json (manual) |
| "Apple silicon and Intel" (llms.txt, compare page) | `packaging/macos/package.sh` (default `--arch universal`, `lipo`) | Yes | add to website-claims.json (manual) |
| "glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later)"; ".deb for Ubuntu 22.04+, Debian 12+ and Mint" | `packaging/linux/package.sh` (built on Ubuntu 22.04, glibc 2.35) | Yes (Debian 12 has 2.36, Fedora 36 has 2.35) | keep |
| "Mac with Homebrew: `brew install --cask aicodeforgedev/tap/cadaddy`" (llms.txt) | <https://github.com/aicodeforgedev/homebrew-tap> `Casks/cadaddy.rb`: `version "0.7.5"`, last changed 17 Oct 2026 | **Stale**: installs 0.7.5, not 0.7.8 | bump the cask with each release (release process, outside this repo) |
| "Updates itself from 0.7.5 on (Help > Check for Updates)" | README ("CADaddy 0.7.5 and later update themselves"); `apps/cadaddy/src/relsig.rs` (ed25519-signed SHA256SUMS) | Yes | keep |
| "AutoCAD-style command line and ribbon" (llms.txt) | `crates/ui-egui/src/ribbon.rs`, `chrome.rs` | Yes | keep |
| Examples `open {"path"}`, `line {"points"}`, `dimlinear {"p1","p2","at"}`, `saveas {"path"}`; `cadaddy-cli convert IN.dwg OUT.pdf` | `crates/engine/src/cmd/annotate.rs` (p1/p2/at), `cmd/file.rs`; `apps/cadaddy-cli/src/main.rs` (`convert`) | Yes | keep |
| Electrical and plumbing symbol libraries (SYMBOL command, Symbols palette) | `crates/doc/src/library/symbols/electrical.rs`, `plumbing.rs`; `cmd/symbol.rs` | Yes | keep |
| "listed in the official MCP Registry as io.github.AICodeForgedev/cadaddy" | <https://registry.modelcontextprotocol.io/v0/servers?search=cadaddy> (0.7.8 `isLatest`, published 20 Oct 2026) | Yes | keep |
| One-click Claude Desktop extension | `releases/latest/download/CADaddy.mcpb` returns 200 | Yes | keep |
| llms.txt summary: "a free AutoCAD-style CAD and drafting program for Windows and Mac" | installers include Linux | Incomplete | "...for Windows, Mac and Linux..." |
| "It is the only option here that does all of that without extra software" (open and save DWG, free, Windows/Mac/Linux, paid work) | the other four programs' rows above | Yes | keep; re-check when LibreCAD 2.2.2 ships |
| "It is also the youngest (version 0.7)" | compare-facts `version` | Yes | keep |

## Re-check list for the next review

- LibreCAD 2.2.2: does the released app (or its CLI) save DWG, and on which platforms?
- FreeCAD 26.3 final: any change to DWG import/export, and the minimum OS versions.
- nanoCAD: whether a newer free edition replaces v5.
- QCAD: the CE/Pro split on the features table.
- Homebrew cask version = the current release.
