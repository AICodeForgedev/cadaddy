//! Files with the standard readers and writers plus the plotter installed.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod bidpackage_letter;
mod mesh3d;
mod output_guard;
mod output_guard_sweep;
mod raster_out;
mod rfi;
mod ugsheets;
