//! 3D, views and viewports: solids, surfaces, meshes, render, sections, UCS, zoom, viewports.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod coordviews;
mod edit3d;
mod fourd;
mod geo;
mod helix;
mod isometric;
mod lib3d;
mod lightbake;
mod materials;
mod named_viewports;
mod named_views;
mod new_drawing_viewports;
mod region;
mod region_booleans;
mod render;
mod sections;
mod sequence4d;
mod solids;
mod solids2;
mod solview;
mod surfaces;
mod tours;
mod ucs;
mod undo_view;
mod view_levels;
mod view_twist;
mod viewport_clip;
mod viewport_clip_partners;
mod volume_measure;
mod vplayer_prompts;
mod vports;
mod vptools;
mod zoom_keywords;
mod zoom_viewport;
mod zoom_window_ortho;
