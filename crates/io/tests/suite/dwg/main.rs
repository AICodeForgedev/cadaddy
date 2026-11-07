//! DWG reading and writing: the native codec, interop, headers, objects, templates.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-io <file>::` still selects that file's tests. Every module of this
//! program shares one process: a test file that changes process-wide state stays a top-level file.

mod acad_interop;
mod acis_data_store;
mod dwg_acds;
mod dwg_active_layout;
mod dwg_anonymous_block_names;
mod dwg_ctrl_chars;
mod dwg_damaged;
mod dwg_dim_value;
mod dwg_hatch_assoc_keep;
mod dwg_header_vars;
mod dwg_interop;
mod dwg_keeps_custom_records;
mod dwg_layout_owner;
mod dwg_native_common_data;
mod dwg_native_entities;
mod dwg_native_groups;
mod dwg_native_objects;
mod dwg_native_raw_keep;
mod dwg_polyline_vertex_order;
mod dwg_required_dictionaries;
mod dwg_roundtrip_losses;
mod dwg_save_keeps_geometry;
mod dwg_save_keeps_ole;
mod dwg_switch;
mod dwg_write_native;
mod dwt_template;
mod low_handseed;
mod q249_native_objects;
mod savefuzz_repros;
