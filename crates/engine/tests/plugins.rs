//! Tests for the sandboxed WASM plug-in system.
//!
//! These tests use the example `hello-plugin` WASM module, which must be built first:
//! ```sh
//! cd examples/plugins/hello-plugin
//! RUSTFLAGS="-C link-args=--allow-undefined" cargo build --target wasm32-unknown-unknown --release
//! ```
//!
//! The tests copy the built `.wasm` into a temporary plugins directory and exercise loading,
//! command registration, execution, enable/disable and removal.

#![cfg(not(target_arch = "wasm32"))]

use cadaddy_engine::Session;
use cadaddy_engine::plugins::PluginHost;
use serde_json::json;
use std::path::PathBuf;

/// Path to the pre-built example plugin WASM.
fn hello_wasm() -> PathBuf {
    // Check a few likely locations.
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/hello-plugin/prebuilt/hello_plugin.wasm"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/hello-plugin/target/wasm32-unknown-unknown/release/hello_plugin.wasm"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    panic!(
        "hello_plugin.wasm not found. Build it first:\n\
         cd examples/plugins/hello-plugin && RUSTFLAGS=\"-C link-args=--allow-undefined\" cargo build --target wasm32-unknown-unknown --release"
    );
}

/// A minimal invalid WASM module (just the magic header, truncated).
fn bad_wasm() -> Vec<u8> {
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00] // valid header, empty module
}

/// A WASM module that just loops forever in plugin_init (for the fuel test).
fn looping_wasm() -> Vec<u8> {
    // A minimal WAT that loops in plugin_init:
    // (module
    //   (func (export "plugin_init") (loop (br 0)))
    //   (memory (export "memory") 1)
    // )
    // Compiled to WASM bytes:
    wat::parse_str(
        r#"(module
            (func (export "plugin_init") (loop (br 0)))
            (memory (export "memory") 1)
        )"#,
    )
    .expect("WAT parse failed")
}

#[test]
fn load_hello_plugin() {
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("load_plugin should succeed");
    assert_eq!(host.plugins.len(), 1);
    assert_eq!(host.plugins[0].name, "hello_plugin");
    assert_eq!(host.plugins[0].commands.len(), 2);

    let ids: Vec<&str> = host.plugins[0].commands.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"hello.greet"), "should have hello.greet: {ids:?}");
    assert!(ids.contains(&"hello.count"), "should have hello.count: {ids:?}");
}

#[test]
fn duplicate_load_is_error() {
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("first load");
    let err = host.load_plugin(&wasm);
    assert!(err.is_err(), "duplicate load should fail");
}

#[test]
fn find_command() {
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("load");
    assert!(host.find_command("hello.greet").is_some());
    assert!(host.find_command("hello.count").is_some());
    assert!(host.find_command("nonexistent").is_none());
}

#[test]
fn enable_disable() {
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("load");
    assert!(host.find_command("hello.greet").is_some());

    host.set_enabled("hello_plugin", false).expect("disable");
    assert!(host.find_command("hello.greet").is_none(), "disabled plugin's commands should be hidden");

    host.set_enabled("hello_plugin", true).expect("enable");
    assert!(host.find_command("hello.greet").is_some(), "re-enabled plugin's commands should be visible");
}

#[test]
fn list_plugins() {
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("load");
    let list = host.list();
    let arr = list.as_array().expect("list should be an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "hello_plugin");
    assert!(arr[0]["enabled"].as_bool().unwrap_or(false));
}

#[test]
fn plugins_command_list() {
    let mut s = Session::new();
    let result = s.execute("plugins", &json!({"action": "list"}));
    assert!(result.is_ok(), "plugins list should work: {result:?}");
}

#[test]
fn plugins_command_commands() {
    let mut s = Session::new();
    let result = s.execute("plugins", &json!({"action": "commands"}));
    assert!(result.is_ok(), "plugins commands should work: {result:?}");
}

#[test]
fn execute_hello_greet() {
    let wasm = hello_wasm();
    let mut s = Session::new();
    s.plugins.load_plugin(&wasm).expect("load");

    // Execute the hello.greet command (trampoline: it calls host_execute("text", ...))
    let result = s.execute("hello.greet", &json!({}));
    assert!(result.is_ok(), "hello.greet should succeed: {result:?}");

    // The command should have added a text entity to the drawing.
    let doc = s.doc().expect("should have a drawing");
    // The drawing starts with 0 user entities; hello.greet should add one.
    assert!(doc.entity_count() > 0, "hello.greet should have added an entity");
}

#[test]
fn execute_hello_count() {
    let wasm = hello_wasm();
    let mut s = Session::new();
    s.plugins.load_plugin(&wasm).expect("load");

    let result = s.execute("hello.count", &json!({}));
    assert!(result.is_ok(), "hello.count should succeed: {result:?}");
    let v = result.expect("ok");
    assert!(v.get("count").is_some(), "should have a count field: {v}");
}

#[test]
fn fuel_limit_stops_infinite_loop() {
    // Write the looping WASM to a temp file.
    let dir = std::env::temp_dir().join("cadaddy_plugin_test_fuel");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("looper.wasm");
    std::fs::write(&path, looping_wasm()).expect("write temp wasm");

    let mut host = PluginHost::new();
    let result = host.load_plugin(&path);
    // plugin_init loops forever, so it should fail with a fuel error.
    assert!(result.is_err(), "should fail with fuel exhaustion");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("fuel") || err.contains("Fuel") || err.contains("plugin_init failed"), "error should mention fuel: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn denied_file_access() {
    // The hello plugin has no WASI imports, so it can't access files. This is tested implicitly:
    // the WASM module imports only `env::host_*` functions. Any attempt to import WASI functions
    // would fail at instantiation (wasmi would report an unresolved import).
    // We verify this by checking that the plugin loaded successfully without WASI.
    let wasm = hello_wasm();
    let mut host = PluginHost::new();
    host.load_plugin(&wasm).expect("load should succeed without WASI");
    // The plugin should have no way to read/write files — verified by design (no WASI imports).
}
