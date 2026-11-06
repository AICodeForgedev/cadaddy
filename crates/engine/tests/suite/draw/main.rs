//! 2D drawing and editing commands: draw, modify, select, snap, grips, layers, prompts, undo.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod angle_prompts;
mod angle_settings;
mod arc_ctrl_direction;
mod array_commands;
mod array_grips;
mod array_path_assoc;
mod assoc_array;
mod break_closed;
mod break_join_locked_layers;
mod break_keeps_z;
mod constraint_geometry;
mod constraint_infer;
mod corpus_roundtrip;
mod drafting_spacing;
mod draw_prompts;
mod draworder;
mod edit2d3;
mod exact_loops;
mod fillet_chamfer;
mod fix_holds_grips;
mod grip_dim_input;
mod grip_options;
mod grip_selection;
mod group_selection;
mod groups;
mod groups_display_copy;
mod image_clip_adjust;
mod inquiry_keywords;
mod inquiry_z;
mod join_close;
mod lasso_and_cycling;
mod layer_cmdline;
mod layer_filters;
mod layer_merge_delete;
mod layer_name_control_chars;
mod layer_states_typed;
mod layoff_repeat;
mod layout_prompts;
mod live_measure;
mod loops;
mod matchprop;
mod matchprop_draworder_overkill;
mod measure_distance_snapping;
mod measure_inquiry_2;
mod measureinit;
mod mf_grips;
mod mirror_degenerate_line;
mod modify_curves;
mod offset_closed_spline;
mod offset_ellipse_side;
mod offset_move;
mod offset_polyline_pieces;
mod offset_preselection;
mod parallel_display_list;
mod pedit_splinedit;
mod pickdrag;
mod pline_arc_xline_options;
mod plot_styles;
mod point_modifiers;
mod prompt_keywords;
mod prompt_text;
mod props_bad_lineweight;
mod qprops;
mod rectang_ortho;
mod rename_cmdline;
mod rotate_reference;
mod scale_invariance;
mod select_keywords;
mod select_similar;
mod select_underscore_keywords;
mod selection_class_isolation;
mod selection_modes;
mod snap_annotations;
mod snap_blocks;
mod snap_curves;
mod snap_cycling;
mod snap_intersections_aperture;
mod snap_nearest;
mod snap_overrides;
mod snap_tracking;
mod snaps_more;
mod spline_degree;
mod spline_options;
mod stretch;
mod stretch_select;
mod stretch_window_ortho;
mod temp_tracking;
mod trim_closed_polyline;
mod trim_extend_options;
mod trim_keeps_handle;
mod undo_count;
mod undo_history;
mod undo_layout_tab;
mod undo_memory;
mod xdata_follows_entity;
