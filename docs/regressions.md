# Regression ledger: earlier test reports' fixed findings and the tests that pin them

Every finding an earlier test report marked as fixed, with the test that fails if the fix breaks.
Extend this table when a report's finding is fixed; don't start a new one. A row without a test
says why (not fixed, by design, or not testable headless) instead.

Reports read: every `REPORT-*.md` from the 0.7.7/0.7.8 test rounds, the debug-next report and the
builder reports. The debug-next and builder reports list no "fixed" findings, so they have no rows.
`bugs-for-0.7.4.md` and the 0.7.4 battle-3 report named in the original task do not exist anywhere,
so nothing from them is listed.

Status: **Fixed** = fixed on `next` and the named test exists there (test names checked
2026-10-23). **Open** = not fixed; the reason is given.

## REPORT-0.7.8-battle (battle test, 20 findings)

| # | Finding | Regression test | Status |
|---|---------|-----------------|--------|
| 1 | BREAK can't break a closed polyline; the prompt then gets stuck | `engine/tests/break_closed.rs` (11 tests) | Fixed |
| 2 | 3D Building from Plan (WALLGEN): openings filled with wall, doors 1'-6" off, openings turned into windows | `engine/tests/wallgen.rs::door_blocks_sit_in_their_openings` | Fixed (wallgen3) |
| 3 | MLEADER and imported arrow markups scaled by DIMSCALE in paper space | `engine/tests/markup_import.rs::markup_arrows_on_a_sheet_ignore_dimscale` | Fixed (pspacescale) |
| 4 | `-DIMSTYLE` Apply does not update existing dimensions | `engine/tests/dimstyle_apply.rs` (2 tests) | Fixed |
| 5 | Count table collapses after SCALE | `engine/tests/count_table_scale.rs` (2 tests) | Fixed |
| 6 | New Sheet Set saves the set as a `.cdy` file | `engine/tests/sheetsets.rs::a_new_set_is_a_sheetset_json_file_named_without_its_extension`; `ui-egui/tests/e2e_sheetsets.rs::the_manager_makes_a_sheetset_json_publishes_next_to_it_and_warns_about_blank_sheets` | Fixed (sheetsets3) |
| 7 | Layouts never opened plot blank, no warning; Layout1's viewport shows a tiny corner; viewport stays at the old paper size | `engine/tests/sheetsets.rs::a_drawings_empty_layouts_are_set_up_for_publishing_and_blank_sheets_warned_about` (blank-sheet publish warning); `ui-egui/src/sheetsets.rs::tests::the_last_publish_and_its_blank_sheets_show_in_the_manager`; `engine/tests/layout_setup.rs::a_new_drawings_layout1_shows_the_plan_drawn_before_it_was_opened`, `::a_paper_size_change_makes_the_sheets_viewport_fit_the_new_sheet` | Fixed (sheetsets3) |
| 8 | `.activity.json` written next to every saved file, DWG exports included | `engine/tests/activity.rs::a_short_script_is_logged_saved_and_read_back` (asserts no history file beside the saved files) | Fixed (activity2) |
| 9 | An agent's unknown parameter is ignored with a warning, then the command acts on a default object | `engine/tests/params_docs.rs::agents_unknown_parameters_are_refused_and_the_apps_warned` | Fixed later (strictparams, Q226/Q5012) |
| 10 | Tools ▸ Count doesn't open the Count palette | `ui-egui/src/countpalette.rs::tests::tools_count_opens_the_palette` | Fixed |
| 11 | Markup import: sticky-note text one word per line | `engine/tests/markup_import.rs::sticky_note_text_is_as_wide_as_its_text` | Fixed (pspacescale) |
| 12 | MARKUPIMPORT via the control channel silently does nothing | `ui-egui/tests/control_cmdline.rs::an_agents_markupimport_says_what_happened` | Fixed |
| 13 | Defaults in prompts not in the drawing's units | `engine/tests/prompt_units.rs` (3 tests) | Fixed |
| 14 | Units dialog angle precision shows "45" | `ui-egui/src/dialogs.rs::tests::angle_precisions_show_zero` | Fixed |
| 15 | PDF text layer has raw MTEXT codes | `io/src/tests.rs::mtext_plots_with_a_hidden_copy_without_its_codes` | Fixed (filefix078) |
| 16 | R2000 → .cdy → DWG round trip gives R2004 | `engine/tests/dwgconvert.rs::each_drawing_keeps_its_own_dwg_version` | Fixed (filefix078) |
| 17 | `cadaddy-cli info FILE.cdy` prints two JSON objects back to back | `apps/cadaddy-cli/tests/cdy.rs::info_shows_a_cdy_files_manifest_and_chunk_sizes` | Fixed (filefix078) |
| 18 | Palettes open over the QAT; Count row click zooms with no margin; status line stays | `ui-egui/src/countpalette.rs::tests::a_row_click_zooms_clear_of_the_palette_with_a_margin`, `::the_status_line_goes_with_the_selection`, `::the_palette_lists_counts_and_highlights_references` | Fixed |
| 19 | Walk "Go to room" lists non-room text; 0 devices with the built-in rules | `walk3d/src/derive.rs::room_names_not_labels`, `::the_built_in_rules_place_blocks_on_electrical_layers` | Fixed |
| 20 | Sheet set Publish suggests the last used folder | `ui-egui/src/sheetsets.rs::tests::publishing_suggests_the_sheet_sets_own_folder` | Fixed (sheetsets3) |

## REPORT-0.7.7b-ui (UI pass, 16 findings)

| # | Finding | Regression test | Status |
|---|---------|-----------------|--------|
| 1 | Missing glyphs (empty boxes): MTEXT toolbar "A▾" "⇤" "⇥" "⇥\|" "⌀", the "✕" in support.rs | `ui-egui/tests/interface_glyphs.rs::icon_like_characters_in_buttons_and_labels_are_in_the_shipped_fonts` (every symbol in button/label text is in egui's bundled faces or on its reasoned list; it failed on the three remaining "✕" buttons in extractdlg/layerstates/sheetsets, now `icons::button(Icon::Close)`) | Fixed; see "Web-build symbol boxes" below |
| 2 | Long prompts overflow the floating command line | `ui-egui/tests/ui_review_077b.rs::a_long_prompt_stays_inside_the_command_bar` | Fixed |
| 3 | Docked palettes share height badly; Properties shrinks to a row | `ui-egui/src/docking.rs::height_tests::properties_keeps_most_of_the_column_and_layers_its_minimum` | Fixed |
| 4 | Layer Properties Manager clips columns, no horizontal scrollbar | `ui-egui/tests/ui_review_077b.rs::the_layer_managers_last_column_scrolls_into_view_on_a_small_window` | Fixed |
| 5 | Most dialogs open pinned top-left over the QAT and application menu | `ui-egui/tests/ui_review_077b.rs::dialogs_open_centred_clear_of_the_quick_access_toolbar_and_menus` (the 11 dialogs the report named) | Fixed |
| 6 | Drafting Settings is one very tall column | `ui-egui/tests/dialog_elements.rs::dialog_controls_are_reported_and_drafting_settings_has_tabs` | Fixed |
| 7 | MTEXT editor's font box says Arial while the style is OpenSans | `ui-egui/src/mtext_editor.rs::tests::style_fonts_name_their_family` (the box shows the style font's family) | Fixed |
| 8 | Typed PURGE, FIND, COLOR give command-line prompts, not dialogs | `ui-egui/src/cmddlgs.rs::tests::purge_dialog_purges_the_checked_names_and_shows_why_others_stay`, `cmddlgs.rs::color_tests::select_color_goes_to_the_objects_it_was_opened_for_and_every_tab_draws`; `ui-egui/tests/e2e_findreplace.rs::the_dialog_opens_and_replacing_changes_the_texts`; `ui-egui/tests/e2e_078_purge.rs::the_purge_tree_purges_what_is_checked_and_keeps_the_rest` | Fixed |
| 9 | Application menu has no Close, Exit or recent drawings | `ui-egui/tests/ui_review_077b.rs::the_application_menu_closes_quits_and_lists_recent_drawings` | Fixed |
| 10 | Canvas menu shows Redo Ctrl+Shift+Z, not Ctrl+Y | `ui-egui/tests/ui_review_077b.rs::the_canvas_menu_says_ctrl_y_for_redo`; `ui-egui/src/menus.rs::shortcut_label_tests::shortcut_label_per_platform` | Fixed |
| 11 | External References can only be closed from its tab's right-click menu | `ui-egui/tests/ui_review_077b.rs::external_references_has_a_close_button` | Fixed |
| 12 | Opening a file leaves the untouched Drawing1 open | `ui-egui/tests/ui_review_077b.rs::opening_a_file_replaces_an_untouched_drawing1` | Fixed |
| 13 | About says Version 0.7.6 | `ui-egui/tests/ui_review_077b.rs::about_shows_the_build_version` (the workspace version); `ui-egui/src/diag.rs::tests::about_shows_the_release_build` (the build id) | Fixed |
| 14 | Modified marker on the tab but not in the OS title | `ui-egui/tests/ui_review_077b.rs::the_os_title_marks_unsaved_changes` | Fixed |
| 15 | ViewCube keeps full size on small windows | `ui-egui/tests/ui_review_077b.rs::the_viewcube_shrinks_on_a_small_window` | Fixed |
| 16 | `ui.elements` doesn't report widgets inside dialogs or the context menu | `ui-egui/tests/dialog_elements.rs::dialog_controls_are_reported_and_drafting_settings_has_tabs` | Fixed |

## Other reports

| Source | Finding | Regression test | Status |
|--------|---------|-----------------|--------|
| fidelity #1 | WIPEOUT gains a 5th corner | `io/src/tests.rs::wipeout_survives_dxf_roundtrip` | Fixed |
| promptfuzz #1 | U erases an agent's work | `engine/tests/undo_outside_calls.rs` (3 tests) | Fixed |
| cmdcover #1 / promptfuzz #2 | TAKEOFF not undoable | `engine/src/cmd/takeoff.rs::takeoff_table_is_undoable` | Fixed |
| webtest #1 | Web build crashes: `std::time::Instant` in the GPU frame timer (ui-egui `gpu.rs`, `shaded`) | `engine/tests/no_std_time_in_source.rs::wasm_built_crates_use_web_time_not_std_clocks` (scans all 17 crates the web build compiles, cfg-aware), `::the_guard_fails_if_gpu_rs_goes_back_to_std_instant`, `::the_guard_catches_every_form_and_skips_native_and_test_code` | Fixed |
| webtest #2 | Other std clocks on the web | same guard; it found 9 more in wasm-compiled code (`opening.rs`, `control.rs`, `diag.rs`, `crash_report.rs`, `coordination.rs`, mepfix `printset.rs`), now `web_time` | Fixed |
| i18ntest #1 | Missing glyphs in DesignCenter / Publish | `ui-egui/tests/interface_glyphs.rs::icon_like_characters_in_buttons_and_labels_are_in_the_shipped_fonts` | Fixed |
| installtest #1 | macOS DMG fails strict codesign | No automated test (macOS packaging only): `packaging/macos/package.sh` builds the image with `hdiutil create -srcfolder` | Fixed, untested |
| supplychain #1 | `inout` 0.2.3 yanked | `cargo deny` in CI: `deny.toml` has `yanked = "deny"` | Fixed |

## Web-build symbol boxes (open)

egui's bundled faces, the only fonts the web build has, lack ←, →, ↑, ↓, ≡, ⌄, ⏎, ▲, ▼, ▸ and ●.
On the desktop the system's symbol face draws them (Segoe UI Symbol on Windows, added by
`theme::install_fonts`; DejaVu or Noto on Linux; the system font on macOS), so they show there and
as boxes on the web. Each use is listed by file in `KNOWN` in `ui-egui/tests/interface_glyphs.rs`;
a new use in another file fails the test. Fix: draw them in `icons.rs`.
