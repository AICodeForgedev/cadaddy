//! End-to-end: a whole CadApp driven through the control channel (tests/e2e_common).
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-ui-egui <file>::` still selects that file's tests. Every module of this
//! program shares one process: a test file that changes process-wide state stays a top-level file.

#[path = "../../e2e_common/mod.rs"]
mod e2e_common;

mod automation_tab;
mod e2e_078_color;
mod e2e_078_count;
mod e2e_078_purge;
mod e2e_078_qprops;
mod e2e_078_rtl;
mod e2e_078_tearoff;
mod e2e_078_touch;
mod e2e_aliases;
mod e2e_designcenter;
mod e2e_diag;
mod e2e_dwgprops;
mod e2e_edit3d;
mod e2e_elemprops;
mod e2e_findreplace;
mod e2e_help;
mod e2e_lightbake;
mod e2e_markups;
mod e2e_palettes;
mod e2e_plans2model;
mod e2e_plotpublish;
mod e2e_plotstamp;
mod e2e_qprops;
mod e2e_rtl;
mod e2e_sheetsets;
mod e2e_standards;
mod e2e_texttools;
mod e2e_touch;
mod e2e_vptools;
mod e2e_walktours;
mod e2e_workspaces;
mod e2e_xlsx;
mod firstrun_round2;
mod memory_budget_ui;
