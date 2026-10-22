//! Rhai scripting engine integration.
//!
//! Embeds [Rhai](https://rhai.rs/) with access to every `CommandSpec` in the command registry,
//! entity read/write, selection, system variables and UI prompts (`getpoint`, `getstring`).
//! Scripts are sandboxed: Rhai's built-in file and network packages are excluded
//! (`no_std_packages`), and only CADaddy commands that already ask the user may touch the
//! file system.
//!
//! ## Security
//!
//! - `SECURELOAD` applies to `.rhai` files exactly as it does to `.lsp` files.
//! - The engine is bounded: recursion depth, operation count, string/array/map sizes.
//! - A running Rhai script may not change `SECURELOAD` or `TRUSTEDPATHS`.
//!
//! ## Architecture
//!
//! Rhai function registrations need `'static` closures, but evaluation requires `&mut Session`.
//! We bridge this with [`cadaddy_rhai_bridge::SlotRef`]: a thread-local slot holds the Session
//! reference for the duration of the synchronous evaluation, and registered Rhai functions
//! access it through the slot.  The slot is cleared (by a drop guard) before the Session
//! borrow ends, even on panic.

use std::cell::RefCell;

use cadaddy_rhai_bridge::SlotRef;
use rhai::{Dynamic, Engine, EvalAltResult, Map, Scope};
use serde_json::{Value, json};

/// Maximum recursion depth for Rhai scripts.
const MAX_CALL_LEVELS: usize = 64;
/// Maximum operations before a script is stopped (prevents infinite loops).
const MAX_OPERATIONS: u64 = 10_000_000;
/// Maximum length for a Rhai script source (512 KiB).
const MAX_SOURCE_LEN: usize = 512 * 1024;
/// Maximum number of modules a script may import.
const MAX_MODULES: usize = 32;

// ---------------------------------------------------------------------------
// Thread-local slots
// ---------------------------------------------------------------------------

thread_local! {
    /// The Session slot: installed by `Session::rhai_eval`, used by registered Rhai functions.
    static SESSION: SlotRef<crate::Session> = const { SlotRef::new() };
    /// Pre-answered inputs for `getpoint`/`getreal`/`getstring`/`getint` (consumed in order).
    static INPUTS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// Call a function on Session through the thread-local slot, returning a Rhai error if empty.
fn with_session<R>(f: impl FnOnce(&mut crate::Session) -> R) -> Result<R, Box<EvalAltResult>> {
    SESSION.with(|slot| slot.with(f).ok_or_else(|| -> Box<EvalAltResult> { "Rhai: no active session".into() }))
}

/// Consume the next input from the pre-answered queue.
fn next_input(what: &str) -> Result<Value, Box<EvalAltResult>> {
    INPUTS.with_borrow_mut(|q| if q.is_empty() { Err(format!("no more inputs for {what}").into()) } else { Ok(q.remove(0)) })
}

// ---------------------------------------------------------------------------
// Public state
// ---------------------------------------------------------------------------

/// The per-session Rhai state: engine and persistent scope.
pub struct RhaiState {
    engine: Engine,
    /// Variables survive across `rhai` calls within the same session (like LISP's globals).
    scope: Scope<'static>,
}

impl std::fmt::Debug for RhaiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RhaiState").field("scope_len", &self.scope.len()).finish()
    }
}

impl Default for RhaiState {
    fn default() -> Self {
        Self::new()
    }
}

impl RhaiState {
    pub fn new() -> Self {
        Self { engine: build_engine(), scope: Scope::new() }
    }
}

// ---------------------------------------------------------------------------
// Session integration
// ---------------------------------------------------------------------------

impl crate::Session {
    /// Evaluate Rhai source code with access to the full command API.
    ///
    /// Returns `Ok((value_json, output_lines))` or `Err(error_message)`.
    pub fn rhai_eval(&mut self, source: &str, inputs: Vec<Value>) -> Result<(Value, Vec<String>), String> {
        if source.len() > MAX_SOURCE_LEN {
            return Err(format!("script too long ({} bytes, limit {MAX_SOURCE_LEN})", source.len()));
        }

        let log_start = self.log.len();

        // Move the Rhai state out so `self` is fully available to the slot closures.
        let mut rhai = std::mem::take(&mut self.rhai);

        // Install the input queue.
        INPUTS.with_borrow_mut(|q| *q = inputs);

        // Install the Session reference into the thread-local slot for the duration of eval.
        // `scoped` clears the slot on drop (even on panic).
        let result = SESSION.with(|slot| slot.scoped(self, || rhai.engine.eval_with_scope::<Dynamic>(&mut rhai.scope, source)));

        // Clear input queue.
        INPUTS.with_borrow_mut(|q| q.clear());

        // Collect output produced during the run.
        let output: Vec<String> = self.log.get(log_start..).map(|s| s.to_vec()).unwrap_or_default();

        // Move Rhai state back.
        self.rhai = rhai;

        match result {
            Ok(val) => Ok((dynamic_to_json(&val), output)),
            Err(e) => Err(format!("{e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine construction
// ---------------------------------------------------------------------------

/// Build a sandboxed Rhai engine with all registered functions.
fn build_engine() -> Engine {
    let mut engine = Engine::new_raw();

    // Core packages — math, string, array, map — but NOT file/network.
    use rhai::packages::Package;
    rhai::packages::BasicArrayPackage::new().register_into_engine(&mut engine);
    rhai::packages::BasicMapPackage::new().register_into_engine(&mut engine);
    rhai::packages::BasicStringPackage::new().register_into_engine(&mut engine);
    rhai::packages::BasicMathPackage::new().register_into_engine(&mut engine);
    rhai::packages::LogicPackage::new().register_into_engine(&mut engine);
    rhai::packages::MoreStringPackage::new().register_into_engine(&mut engine);
    rhai::packages::BasicIteratorPackage::new().register_into_engine(&mut engine);

    // Safety limits.
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_string_size(1024 * 1024);
    engine.set_max_array_size(100_000);
    engine.set_max_map_size(10_000);
    engine.set_max_modules(MAX_MODULES);

    // ---- print / debug → session log ----
    engine.on_print(|text| {
        SESSION.with(|slot| {
            slot.with(|s| s.echo(text.to_string()));
        });
    });
    engine.on_debug(|text, src, pos| {
        let loc = match (src, pos) {
            (Some(src), pos) if !pos.is_none() => format!("[{src}:{pos}] "),
            (Some(src), _) => format!("[{src}] "),
            (None, pos) if !pos.is_none() => format!("[{pos}] "),
            _ => String::new(),
        };
        SESSION.with(|slot| {
            slot.with(|s| s.echo(format!("{loc}{text}")));
        });
    });

    // ---- command(id, params) → result ----
    engine.register_fn("command", |id: &str, params: Map| -> Result<Dynamic, Box<EvalAltResult>> {
        if id.eq_ignore_ascii_case("secureload.allow") {
            let is_running = with_session(|s| s.running_code())?;
            if is_running {
                return Err("secureload.allow can't be used from a script".into());
            }
        }
        let json_params = map_to_json(&params);
        with_session(|s| match s.execute(id, &json_params) {
            Ok(val) => Ok(json_to_dynamic(&val)),
            Err(e) => Err(e.to_string().into()),
        })?
    });

    // ---- command(id) — no params ----
    engine.register_fn("command", |id: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        if id.eq_ignore_ascii_case("secureload.allow") {
            let is_running = with_session(|s| s.running_code())?;
            if is_running {
                return Err("secureload.allow can't be used from a script".into());
            }
        }
        with_session(|s| match s.execute(id, &json!({})) {
            Ok(val) => Ok(json_to_dynamic(&val)),
            Err(e) => Err(e.to_string().into()),
        })?
    });

    // ---- getvar(name) ----
    engine.register_fn("getvar", |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        with_session(|s| match crate::sysvars::get(s, name) {
            Some(v) => Ok(json_to_dynamic(&v)),
            None => Err(format!("{name} is not a system variable").into()),
        })?
    });

    // ---- setvar(name, value) ----
    engine.register_fn("setvar", |name: &str, val: Dynamic| -> Result<(), Box<EvalAltResult>> {
        let jval = dynamic_to_json(&val);
        with_session(|s| crate::sysvars::set(s, name, &jval).map_err(|e| -> Box<EvalAltResult> { e.to_string().into() }))?
    });

    // ---- echo(text) ----
    engine.register_fn("echo", |text: &str| {
        SESSION.with(|slot| {
            slot.with(|s| s.echo(text.to_string()));
        });
    });

    // ---- selection() → [handle strings] ----
    engine.register_fn("selection", || -> Result<Dynamic, Box<EvalAltResult>> {
        with_session(|s| {
            let handles: Vec<Dynamic> = s.selection().iter().map(|h| Dynamic::from(h.hex())).collect();
            Ok(Dynamic::from_array(handles))
        })?
    });

    // ---- entity(handle) → properties map ----
    engine.register_fn("entity", |handle_str: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        let params = json!({ "handle": handle_str });
        with_session(|s| match s.execute("entget", &params) {
            Ok(val) => Ok(json_to_dynamic(&val)),
            Err(e) => Err(e.to_string().into()),
        })?
    });

    // ---- ssget() → current selection as array ----
    engine.register_fn("ssget", || -> Result<Dynamic, Box<EvalAltResult>> {
        with_session(|s| {
            let handles: Vec<Dynamic> = s.selection().iter().map(|h| Dynamic::from(h.hex())).collect();
            Ok(Dynamic::from_array(handles))
        })?
    });

    // ---- sslength(ss) ----
    engine.register_fn("sslength", |ss: rhai::Array| -> i64 { ss.len() as i64 });

    // ---- ssname(ss, index) ----
    engine.register_fn("ssname", |ss: rhai::Array, idx: i64| -> Result<Dynamic, Box<EvalAltResult>> {
        let i = idx as usize;
        ss.get(i).cloned().ok_or_else(|| format!("ssname: index {i} out of range (length {})", ss.len()).into())
    });

    // ---- getpoint(prompt) ----
    engine.register_fn("getpoint", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        if !prompt.is_empty() {
            SESSION.with(|slot| {
                slot.with(|s| s.echo(prompt.to_string()));
            });
        }
        let val = next_input("getpoint")?;
        if let Some(arr) = val.as_array()
            && arr.len() >= 2
        {
            let nums: Vec<Dynamic> = arr.iter().filter_map(Value::as_f64).map(Dynamic::from).collect();
            return Ok(Dynamic::from_array(nums));
        }
        Err(format!("getpoint: expected [x, y], got {val}").into())
    });

    // ---- getreal(prompt) ----
    engine.register_fn("getreal", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        if !prompt.is_empty() {
            SESSION.with(|slot| {
                slot.with(|s| s.echo(prompt.to_string()));
            });
        }
        let val = next_input("getreal")?;
        val.as_f64().map(Dynamic::from).ok_or_else(|| format!("getreal: expected number, got {val}").into())
    });

    // ---- getstring(prompt) ----
    engine.register_fn("getstring", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        if !prompt.is_empty() {
            SESSION.with(|slot| {
                slot.with(|s| s.echo(prompt.to_string()));
            });
        }
        let val = next_input("getstring")?;
        match &val {
            Value::String(s) => Ok(Dynamic::from(s.clone())),
            other => Ok(Dynamic::from(other.to_string())),
        }
    });

    // ---- getint(prompt) ----
    engine.register_fn("getint", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        if !prompt.is_empty() {
            SESSION.with(|slot| {
                slot.with(|s| s.echo(prompt.to_string()));
            });
        }
        let val = next_input("getint")?;
        val.as_i64().map(Dynamic::from).ok_or_else(|| format!("getint: expected integer, got {val}").into())
    });

    engine
}

// ---------------------------------------------------------------------------
// JSON ↔ Dynamic conversions
// ---------------------------------------------------------------------------

/// Convert a `serde_json::Value` to a Rhai `Dynamic`.
pub(crate) fn json_to_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else {
                Dynamic::from(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Array(a) => Dynamic::from_array(a.iter().map(json_to_dynamic).collect()),
        Value::Object(m) => {
            let map: Map = m.iter().map(|(k, v)| (k.clone().into(), json_to_dynamic(v))).collect();
            Dynamic::from_map(map)
        }
    }
}

/// Convert a Rhai `Dynamic` to a `serde_json::Value`.
pub(crate) fn dynamic_to_json(v: &Dynamic) -> Value {
    if v.is_unit() {
        return Value::Null;
    }
    if let Ok(b) = v.as_bool() {
        return Value::Bool(b);
    }
    if let Ok(i) = v.as_int() {
        return json!(i);
    }
    if let Ok(f) = v.as_float() {
        return json!(f);
    }
    if let Ok(s) = v.clone().into_string() {
        return Value::String(s);
    }
    if let Ok(arr) = v.clone().into_array() {
        return Value::Array(arr.iter().map(dynamic_to_json).collect());
    }
    if v.is_map()
        && let Some(map) = v.clone().try_cast::<Map>()
    {
        let obj: serde_json::Map<String, Value> = map.iter().map(|(k, v)| (k.to_string(), dynamic_to_json(v))).collect();
        return Value::Object(obj);
    }
    Value::String(v.to_string())
}

/// Convert a Rhai `Map` to a `serde_json::Value`.
fn map_to_json(m: &Map) -> Value {
    let obj: serde_json::Map<String, Value> = m.iter().map(|(k, v)| (k.to_string(), dynamic_to_json(v))).collect();
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let v = json!({"a": 1, "b": [2.5, true, null, "hello"]});
        let d = json_to_dynamic(&v);
        let back = dynamic_to_json(&d);
        assert_eq!(v, back);
    }

    #[test]
    fn engine_limits_are_set() {
        let engine = build_engine();
        assert_eq!(engine.max_call_levels(), MAX_CALL_LEVELS);
        assert!(engine.max_operations() > 0);
    }

    #[test]
    fn basic_eval() {
        let mut s = crate::Session::new();
        let (val, _) = s.rhai_eval("40 + 2", vec![]).expect("eval failed");
        assert_eq!(val, json!(42));
    }

    #[test]
    fn scope_persists() {
        let mut s = crate::Session::new();
        s.rhai_eval("let x = 10;", vec![]).expect("first eval");
        let (val, _) = s.rhai_eval("x + 5", vec![]).expect("second eval");
        assert_eq!(val, json!(15));
    }

    #[test]
    fn source_too_long() {
        let mut s = crate::Session::new();
        let big = "x".repeat(MAX_SOURCE_LEN + 1);
        let err = s.rhai_eval(&big, vec![]).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn print_goes_to_log() {
        let mut s = crate::Session::new();
        let (_, output) = s.rhai_eval(r#"print("hello from rhai");"#, vec![]).expect("eval");
        assert!(output.iter().any(|l| l.contains("hello from rhai")), "output: {output:?}");
    }
}
