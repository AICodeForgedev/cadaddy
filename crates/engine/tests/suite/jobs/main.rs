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
mod asbuilt;
mod autospec;
mod bimux_rw1;
mod bimwork;
mod bimwork_levels;
mod branchgap;
mod breakers;
mod casework_read;
mod ceil_assemblies;
mod ceilings;
mod concrete;
mod conduitjob;
mod conduitroute;
mod dimreader;
mod drywall_views;
mod duct3d_truth;
mod elemprops;
mod exterior;
mod feederwire;
mod followups_626;
mod furnish;
mod gear;
mod grounding;
mod grounding_hotel;
mod guided;
mod imagetrace;
mod imagetrace_accuracy;
mod import_check;
mod jobjunk_doors;
mod levelsread;
mod lighting;
mod mep3d;
mod mepatoms;
mod mepduct_build;
mod mepfix_wholejob;
mod meppipe_build;
mod mepreview;
mod pdfocrjunk;
mod pdfplans;
mod perceive;
mod planview;
mod progress_hotel;
mod propuse;
mod readerwire;
mod rooms3d;
mod roomsmodel;
mod rulemodel;
mod scopesplit;
mod shapes;
mod site_services;
mod sitework;
mod slabarea;
mod soak5speed;
mod spectag;
mod stairs;
mod tagidentity;
mod tradeatoms;
mod typicalfloors;
mod ugplan;
mod unittypes;
mod wall_assemblies;
mod wallgen;
mod walllayout;
mod wholejob;
