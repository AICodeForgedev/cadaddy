//! Files with the standard readers and writers installed (cadaddy-io read and write).
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod adcenter;
mod agentguard;
mod agentrails3;
mod agentready;
mod anno_positions;
mod cobie;
mod conduit_reader;
mod dgn;
mod elecequip;
mod etransmit;
mod exchange3d;
mod extract_depth;
mod framing;
mod grids;
mod ifc;
mod ifc_import;
mod ifc_validity;
mod ifc_viewer;
mod insert_units;
mod labels;
mod layer_tools;
mod layout_template_saveas;
mod oversize;
mod parity2_commands;
mod part;
mod path_case;
mod printrev;
mod projects;
mod renderenv;
mod sprinkler_reader;
mod standards;
mod table_breaks;
mod tray_reader;
mod walkpub;
mod wblock_typed;
mod xref;
mod xref_edit;
