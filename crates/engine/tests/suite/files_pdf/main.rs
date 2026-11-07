//! Files with the standard readers and writers plus the PDF reader installed.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod bimjobstate;
mod bimpdfpages;
mod devicefit;
mod jobmep;
mod jobunits;
mod levelregions;
mod mepnoise;
mod pdf_underlay;
mod printlinks;
mod propfill;
mod roofreal;
mod roomtagtext;
mod takeoff_edits;
mod validatecrops;
mod validateequip;
mod validatefixtures;
mod validatekinds;
mod validatewalls;
