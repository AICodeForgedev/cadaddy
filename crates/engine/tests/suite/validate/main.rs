//! Validate and review: checks, comparison, fidelity, accuracy, clash.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod accuracytrend;
mod clash_rerun;
mod compare_export;
mod covercheck;
mod fidelity;
mod printcheck;
mod reviewimpact;
mod validateagent;
mod validateconduit;
mod validatecoverage;
mod validateframes;
mod validategear;
mod validatekeynotes;
mod validatelighting;
mod validatepacket;
mod validateprescreen;
mod validatepresent;
mod validateregister;
mod validatereport;
mod validatetr;
mod validateunderground;
mod validatevalue;
mod validatewalllist;
mod verify3d;
