//! Annotation and blocks: dimensions, text, leaders, tables, hatch, attributes, blocks, linetypes.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod annomon;
mod annotate_options;
mod annotative;
mod attdisp_attsync;
mod attedit_prompts;
mod attrib_modes;
mod attrib_props;
mod attribute_tools;
mod block_actionbars;
mod block_description;
mod block_detect;
mod block_keep;
mod block_redefine;
mod dash_block;
mod dim_baseline_spacing;
mod dim_chain;
mod dim_commands;
mod dim_diameter_sign;
mod dim_explode_text;
mod dim_properties;
mod dim_radial_inside;
mod dim_radial_outside;
mod dim_vars;
mod dim_vars_2;
mod dimassoc_saved;
mod dimbreak_assoc;
mod dims3;
mod dimstyle_apply;
mod dimtedit_prompts;
mod dyn_input;
mod dynamic_block_data;
mod dynamic_grips;
mod dynblock_actions;
mod dynblock_author;
mod dynblock_authoring;
mod dynblock_roundtrip;
mod explode_keeps_block_props;
mod express_burst_txt2mtxt;
mod express_tools_2;
mod express_tools_3;
mod find_replace;
mod hatch_assoc;
mod hatch_dialog_helpers;
mod hatch_edit_boundaries;
mod hatch_numeric_validation;
mod hatch_options;
mod hatchnotices;
mod insert_prompts;
mod lin_pat_files;
mod ltype_define;
mod metric_linetypes;
mod mleader_content;
mod mleader_landings_style;
mod mleader_remember;
mod mleader_scale;
mod mline;
mod mline_tees_purge;
mod mtext_columns;
mod mtext_options;
mod multiline_attributes;
mod pick_text;
mod props_insert_scale_attribs;
mod purge_arrow_blocks;
mod purge_mlinestyles;
mod purge_prompts;
mod purge_reasons;
mod qdim;
mod qdim_modes;
mod refedit;
mod table_formulas;
mod table_malformed_handle;
mod table_styles;
mod table_wrap_formats;
mod text_commands;
mod text_left_justify;
mod textalign;
mod textalign_mtext_attribs;
mod textmask;
mod texttools2;
mod tolerance;
