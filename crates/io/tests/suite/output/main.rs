//! PDF, SVG, plot and other output and underlay formats.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-io <file>::` still selects that file's tests. Every module of this
//! program shares one process: a test file that changes process-wide state stays a top-level file.

mod dwf_underlay_roundtrip;
mod eps_wmf_export;
mod image_export;
mod layout_plot_window;
mod pdf_followups;
mod pdf_jobspeed;
mod pdf_layers;
mod pdf_markups;
mod pdf_native;
mod pdf_shape_text;
mod pdf_text;
mod pdf_underlay_roundtrip;
mod plot_layout_scale;
mod plot_model_png_scale;
mod plot_styles;
mod pointcloud_las;
mod svg_arc_budget;
mod svg_tiny_extents;
