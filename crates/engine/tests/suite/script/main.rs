//! Scripting, agents and settings: LISP, actions, agent calls, parameters, system variables, guards.
//!
//! Each module is one former top-level test file and keeps its name, so
//! `cargo test -p cadaddy-engine <file>::` still selects that file's tests. Every module of
//! this program shares one process: a test file that changes process-wide state (IoHooks other than
//! this program's, environment variables, the current folder, global hooks) stays a top-level file.

#[allow(dead_code)]
#[path = "../helper.rs"]
mod helper;

mod actions;
mod area_units;
mod batch;
mod clipboard_system;
mod cmdvalues;
mod events;
mod guide_commands;
mod headernan;
mod hyperlinks;
mod json_clean;
mod json_param_validation;
mod lisp_vl;
mod malformed_handles;
mod menu_commands_prompt;
mod no_mojibake;
mod non_finite_edits;
mod options_profile;
mod output_params_guarded;
mod overrides;
mod paramconstraint;
mod parameters_manager;
mod paramguards2;
mod parammsgs;
mod parammsgs4;
mod params_docs;
mod ported_input_checks;
mod post_command_passes;
mod progress;
mod prompt_units;
mod quickcalc;
mod recent_commands;
mod secureload;
mod settings_toggles;
mod setvar_cecolor;
mod setvar_coverage;
mod space_is_enter;
mod stubs2;
mod stuck_prompt_fixes;
mod tolerance_dialog_leader;
mod transparent;
mod undo_outside_calls;
mod units_bearing;
mod usage;
mod writes_guarded;
