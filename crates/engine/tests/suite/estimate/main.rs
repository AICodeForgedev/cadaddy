//! Estimate and takeoff: BOM, quantities, prices, bids, pull lists, schedules.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod automation_takeoff;
mod count_table_scale;
mod data_extraction;
mod estimate_hotel;
mod finishes_takeoff;
mod jobbom;
mod loadcalc;
mod mepqty;
mod procure;
mod pubcost;
mod pulllist;
mod schedules3d;
mod valueeng;
mod xlsx_link;
