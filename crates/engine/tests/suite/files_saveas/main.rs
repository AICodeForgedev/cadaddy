//! Files with the standard readers and writers and the save-as writer (DWG/DXF versions) installed.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod b4polish;
mod blockhatchassoc;
mod classic2d;
mod dwgconvert;
mod exchangeloss;
mod keptnotices2;
mod mleader_table_dwg;
