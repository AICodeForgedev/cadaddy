//! Jobs and the building model: guided jobs, print readers, BIM, rooms, walls, MEP, site.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod agentready_hotel;
mod arch_read;
mod archbuild;
mod arexport;
mod arexport_set;
mod asbuilt;
mod autospec;
mod bimux_rw1;
mod bimwork;
mod bimwork_levels;
mod branchgap;
mod breakers;
mod callout_walls;
mod casework_read;
mod ceil_assemblies;
mod ceilings;
mod completebuilding;
mod concrete;
mod conduitjob;
mod conduitroute;
mod dimreader;
mod drywall_views;
mod duct3d_truth;
mod elemprops;
mod emtsize;
mod equip_mounting;
mod exterior;
mod feederwire;
mod fixturekinds;
mod floorjoists;
mod followups_626;
mod furnish;
mod gear;
mod gearspace;
mod grounding;
mod grounding_hotel;
mod guided;
mod imagetrace;
mod imagetrace_accuracy;
mod import_check;
mod jobjunk_doors;
mod levelsread;
mod lighting;
mod liverecord;
mod mep3d;
mod mepatoms;
mod mepduct_build;
mod mepedit;
mod mepfix_wholejob;
mod meppipe_build;
mod mepreview;
mod ocrreviewtags;
mod parapetsteps;
mod pdfocrjunk;
mod pdfplans;
mod perceive;
mod planview;
mod progress_hotel;
mod propuse;
mod puttypads;
mod racewaygeom;
mod riser_storeys;
mod readerwire;
mod relevel;
mod roofframing;
mod rooms3d;
mod roomsmodel;
mod routeunbuilt;
mod rulemodel;
mod scopesplit;
mod shapes;
mod site_services;
mod sitework;
mod slabarea;
mod soak5speed;
mod specrooms;
mod spectag;
mod stair_treads_walls;
mod stairs;
mod swing_joined;
mod symbol_walls;
mod tagidentity;
mod takeoff_drywall_pieces;
mod takeoff_windows;
mod tradeatoms;
mod typicalfloors;
mod ugplan;
mod unittypes;
mod wall_assemblies;
mod wallgen;
mod walllayout;
mod wholejob;
mod window_frame_gap;
mod windows_typed;
