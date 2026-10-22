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
//! borrow ends, even on panic.  A script started with [`crate::Session::rhai_start`] (the Script
//! Editor) runs on a worker thread instead: each call into the session is sent to the session's
//! thread and run there by [`crate::Session::rhai_pump`], so the window keeps drawing and its
//! Stop button works.
//!
//! ## Stop and nesting
//!
//! Every run checks a stop flag ([`crate::Session::rhai_stop`]) and, on the caller's thread, a
//! wall-clock budget ([`DEFAULT_BUDGET`], `timeoutMs`) as it goes: `loop {}` ends with a
//! "stopped" error. One script runs at a time: `rhai`/`rhairun` called from a running script
//! are refused with a guiding error before anything is installed.
//!
//! ## Objects and events
//!
//! `entity(handle)` reads an object as PROPERTIES gives it; `set_entity(handle, #{...})` changes
//! it through PROPERTIES.SET (one undo step). `on("command_ended" | "objects_changed" |
//! "drawing_opened", |ev| ...)` registers a handler that runs after each command (the script's
//! own, typed ones and agents' calls), with `ev.handles` (the objects the command added, for
//! `command_ended`; every object it added, changed or erased, for `objects_changed`); `off(event)`
//! and `off()` remove handlers. Commands a handler runs set off no further events.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use cadaddy_rhai_bridge::SlotRef;
use rhai::{AST, Dynamic, Engine, EvalAltResult, FnPtr, Map, NativeCallContext, Scope};
use serde_json::{Value, json};
use web_time::{Duration, Instant};

/// Maximum recursion depth for Rhai scripts.
const MAX_CALL_LEVELS: usize = 64;
/// Maximum operations before a script is stopped (prevents infinite loops).
const MAX_OPERATIONS: u64 = 10_000_000;
/// Maximum length for a Rhai script source (512 KiB).
const MAX_SOURCE_LEN: usize = 512 * 1024;
/// Maximum number of modules a script may import.
const MAX_MODULES: usize = 32;
/// Wall-clock budget of a script run on the caller's thread (an agent's or the command line's
/// `rhai`): past it the script stops with a "stopped" error. A caller may pass its own.
pub const DEFAULT_BUDGET: Duration = Duration::from_secs(30);
/// The longest budget a caller may ask for.
pub const MAX_BUDGET: Duration = Duration::from_secs(600);
/// Event handlers a session keeps.
const MAX_HANDLERS: usize = 64;
/// Events waiting for the handlers (the oldest go first).
const MAX_PENDING: usize = 256;
/// Handler calls per dispatch.
const MAX_CALLS: usize = 1_000;
/// Handles listed per event.
const MAX_EVENT_HANDLES: usize = 10_000;
/// Lines of printed output kept per run.
const MAX_OUTPUT: usize = 10_000;
/// The events a script may handle with `on(name, fn)`.
pub const EVENTS: [&str; 3] = ["command_ended", "objects_changed", "drawing_opened"];

// ---------------------------------------------------------------------------
// Thread-local slots
// ---------------------------------------------------------------------------

/// A call into the session sent from a script running on a worker thread ([`RhaiJob`]): run on
/// the session's thread by [`crate::Session::rhai_pump`].
type Request = Box<dyn FnOnce(&mut crate::Session) + Send>;

/// How the running script reaches the session and when it must stop.
struct RunCtl {
    /// Set by Stop ([`crate::Session::rhai_stop`]).
    stop: Arc<AtomicBool>,
    /// The wall-clock budget's end (none for a script the user runs in the Script Editor: it has Stop).
    deadline: Option<Instant>,
    budget: Duration,
    /// A worker thread's way to the session (none: the session is in [`SESSION`]).
    remote: Option<mpsc::Sender<Request>>,
    /// The script, for the handlers it registers.
    ast: Arc<AST>,
}

impl RunCtl {
    /// Why the script must stop now, if it must.
    fn stop_reason(&self) -> Option<String> {
        if self.stop.load(Ordering::Relaxed) {
            return Some("script stopped by the user".into());
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Some(format!(
                "script stopped: it ran past its time budget of {} ms (pass `timeoutMs` for a longer one)",
                self.budget.as_millis()
            ));
        }
        None
    }
}

/// An event handler a script registered with `on(event, fn)`.
#[derive(Clone)]
struct Handler {
    event: String,
    f: FnPtr,
    /// The script that defined the handler (its closures live in it).
    ast: Arc<AST>,
}

thread_local! {
    /// The Session slot: installed by `Session::rhai_eval`, used by registered Rhai functions.
    static SESSION: SlotRef<crate::Session> = const { SlotRef::new() };
    /// Pre-answered inputs for `getpoint`/`getreal`/`getstring`/`getint` (consumed in order).
    static INPUTS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// The running script's stop flag, budget and way to the session.
    static RUN: RefCell<Option<RunCtl>> = const { RefCell::new(None) };
    /// The running script's event handlers (moved here from the session for the run).
    static HANDLERS: RefCell<Vec<Handler>> = const { RefCell::new(Vec::new()) };
    /// A handler is running: the commands it calls set off no more events.
    static IN_HANDLER: Cell<bool> = const { Cell::new(false) };
    /// What the running script printed.
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn stopped_now() -> Option<String> {
    RUN.with_borrow(|r| r.as_ref().and_then(RunCtl::stop_reason))
}

/// Call a function on Session — through the thread-local slot, or, from a worker thread, on the
/// session's thread — returning a Rhai error if there is none (or the script was stopped while
/// waiting).
fn with_session<R: Send + 'static>(f: impl FnOnce(&mut crate::Session) -> R + Send + 'static) -> Result<R, Box<EvalAltResult>> {
    if SESSION.with(SlotRef::is_installed) {
        return SESSION.with(|slot| slot.with(f).ok_or_else(|| -> Box<EvalAltResult> { "Rhai: no active session".into() }));
    }
    let Some(remote) = RUN.with_borrow(|r| r.as_ref().and_then(|r| r.remote.clone())) else {
        return Err("Rhai: no active session".into());
    };
    let (tx, rx) = mpsc::channel();
    // Given up (the script stopped while it waited): the call doesn't run late.
    let given_up = Arc::new(AtomicBool::new(false));
    let skip = Arc::clone(&given_up);
    let req: Request = Box::new(move |s: &mut crate::Session| {
        if skip.load(Ordering::Relaxed) {
            return;
        }
        // The call runs as the script's: SECURELOAD can't be loosened from it.
        let was = s.in_script;
        s.in_script = true;
        let r = f(s);
        s.in_script = was;
        let _ = tx.send(r);
    });
    remote.send(req).map_err(|_| -> Box<EvalAltResult> { "script stopped: the session closed".into() })?;
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(r) => return Ok(r),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(why) = stopped_now() {
                    given_up.store(true, Ordering::Relaxed);
                    return Err(why.into());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("script stopped: the session closed".into()),
        }
    }
}

/// Echo a line on the command line and keep it as the script's output.
fn print_line(text: String) {
    OUTPUT.with_borrow_mut(|o| {
        if o.len() < MAX_OUTPUT {
            o.push(text.clone());
        }
    });
    let _ = with_session(move |s| s.echo(text));
}

/// Consume the next input from the pre-answered queue.
fn next_input(what: &str) -> Result<Value, Box<EvalAltResult>> {
    INPUTS.with_borrow_mut(|q| if q.is_empty() { Err(format!("no more inputs for {what}").into()) } else { Ok(q.remove(0)) })
}

/// An error as the caller reads it: a stop (Stop, the time budget, the operation limit) as its
/// reason, without Rhai's wrapping.
fn error_text(e: &EvalAltResult) -> String {
    stop_text(e).unwrap_or_else(|| e.to_string())
}

fn stop_text(e: &EvalAltResult) -> Option<String> {
    match e {
        EvalAltResult::ErrorTerminated(token, _) => Some(token.to_string()),
        EvalAltResult::ErrorTooManyOperations(_) => Some(format!("script stopped: it ran past the limit of {MAX_OPERATIONS} operations")),
        EvalAltResult::ErrorRuntime(v, _) => v.clone().into_string().ok().filter(|s| s.starts_with("script stopped")),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => stop_text(inner),
        _ => None,
    }
}

/// Run the handlers of `events` (on this thread, with the running script's handlers). A handler's
/// error is printed, and the rest still run.
fn dispatch(engine: &Engine, events: Vec<Value>) {
    if events.is_empty() || IN_HANDLER.get() {
        return;
    }
    let calls: Vec<(FnPtr, Arc<AST>, Value)> = HANDLERS.with_borrow(|hs| {
        events
            .iter()
            .flat_map(|ev| {
                let name = ev.get("event").and_then(Value::as_str).unwrap_or_default().to_string();
                hs.iter().filter(move |h| h.event == name).map(move |h| (h.f.clone(), Arc::clone(&h.ast), ev.clone()))
            })
            .take(MAX_CALLS)
            .collect()
    });
    IN_HANDLER.set(true);
    for (f, ast, ev) in calls {
        if stopped_now().is_some() {
            break;
        }
        if let Err(e) = f.call::<Dynamic>(engine, &ast, (json_to_dynamic(&ev),)) {
            let name = ev.get("event").and_then(Value::as_str).unwrap_or_default();
            print_line(format!("Rhai {name} handler: {}", error_text(&e)));
        }
    }
    IN_HANDLER.set(false);
}

/// Install the run's thread-local state (inputs, handlers, stop control).
fn begin_run(inputs: Vec<Value>, handlers: Vec<Handler>, ctl: RunCtl) {
    INPUTS.with_borrow_mut(|q| *q = inputs);
    HANDLERS.with_borrow_mut(|h| *h = handlers);
    OUTPUT.with_borrow_mut(Vec::clear);
    RUN.with_borrow_mut(|r| *r = Some(ctl));
    IN_HANDLER.set(false);
}

/// Clear the run's thread-local state; the handlers and output it leaves.
fn end_run() -> (Vec<Handler>, Vec<String>) {
    INPUTS.with_borrow_mut(Vec::clear);
    RUN.with_borrow_mut(|r| *r = None);
    (HANDLERS.take(), OUTPUT.take())
}

// ---------------------------------------------------------------------------
// Public state
// ---------------------------------------------------------------------------

/// A script's result: its value and output, or its error and the output printed before it.
pub type RunResult = Result<(Value, Vec<String>), (String, Vec<String>)>;

/// The per-session Rhai state: engine, persistent scope and event handlers.
pub struct RhaiState {
    engine: Arc<Engine>,
    /// Variables survive across `rhai` calls within the same session (like LISP's globals).
    scope: Scope<'static>,
    handlers: Vec<Handler>,
    /// Events found since the handlers last ran.
    pending: Vec<Value>,
    /// A script is running (its scope and handlers are out of the session).
    busy: bool,
    /// The running script's stop flag.
    stop: Arc<AtomicBool>,
    /// A script running on a worker thread (the Script Editor).
    job: Option<RhaiJob>,
    /// The worker script's result, until the front end takes it.
    finished: Option<RunResult>,
}

impl std::fmt::Debug for RhaiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RhaiState").field("scope_len", &self.scope.len()).field("handlers", &self.handlers.len()).field("busy", &self.busy).finish()
    }
}

impl Default for RhaiState {
    fn default() -> Self {
        Self::new()
    }
}

impl RhaiState {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(build_engine()),
            scope: Scope::new(),
            handlers: Vec::new(),
            pending: Vec::new(),
            busy: false,
            stop: Arc::new(AtomicBool::new(false)),
            job: None,
            finished: None,
        }
    }

    /// Event handlers registered (and not running a script now).
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }
}

/// A script running on a worker thread: its calls into the session come here and run on the
/// session's thread ([`crate::Session::rhai_pump`]), so the front end stays responsive and Stop
/// works.
pub struct RhaiJob {
    requests: mpsc::Receiver<Request>,
    done: mpsc::Receiver<JobDone>,
}

struct JobDone {
    result: Result<Value, String>,
    output: Vec<String>,
    scope: Scope<'static>,
    handlers: Vec<Handler>,
}

// ---------------------------------------------------------------------------
// Session integration
// ---------------------------------------------------------------------------

/// Why a script can't start: one is running.
const NESTED: &str = "a Rhai script is running already: rhai and rhairun can't be called from a script (or while one runs); call the code directly, or define a function and call it";

impl crate::Session {
    /// A Rhai script is running (on this thread or a worker).
    pub fn rhai_running(&self) -> bool {
        self.rhai.busy
    }

    /// Ask the running script to stop: it ends with a "stopped" error at its next step.
    pub fn rhai_stop(&mut self) {
        self.rhai.stop.store(true, Ordering::Relaxed);
    }

    /// The running script's stop flag (another thread may set it).
    pub fn rhai_stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.rhai.stop)
    }

    /// Evaluate Rhai source code with access to the full command API, on this thread, within
    /// `budget` (wall clock; `None`: [`DEFAULT_BUDGET`]).
    ///
    /// Returns the value and the lines printed, or the error and the lines printed before it.
    pub fn rhai_eval(&mut self, source: &str, inputs: Vec<Value>, budget: Option<Duration>) -> RunResult {
        let (engine, ast) = self.rhai_prepare(source)?;
        let budget = budget.unwrap_or(DEFAULT_BUDGET).min(MAX_BUDGET);
        let mut scope = std::mem::take(&mut self.rhai.scope);
        let handlers = std::mem::take(&mut self.rhai.handlers);
        let ctl = RunCtl { stop: Arc::clone(&self.rhai.stop), deadline: Some(Instant::now() + budget), budget, remote: None, ast: Arc::clone(&ast) };
        begin_run(inputs, handlers, ctl);
        // `scoped` clears the slot on drop (even on panic). The slot is empty: `busy` refused a
        // nested run, so a refusal here only means a script on this thread outside the session.
        let result = SESSION
            .with(|slot| slot.scoped(self, || engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast)))
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| error_text(&e)));
        let (handlers, output) = end_run();
        self.rhai.scope = scope;
        self.rhai.handlers = handlers;
        self.rhai.busy = false;
        match result {
            Ok(val) => Ok((dynamic_to_json(&val), output)),
            Err(e) => Err((e, output)),
        }
    }

    /// Check and compile a script about to run, and mark the session busy.
    fn rhai_prepare(&mut self, source: &str) -> Result<(Arc<Engine>, Arc<AST>), (String, Vec<String>)> {
        if self.rhai.busy {
            return Err((NESTED.into(), Vec::new()));
        }
        if source.len() > MAX_SOURCE_LEN {
            return Err((format!("script too long ({} bytes, limit {MAX_SOURCE_LEN})", source.len()), Vec::new()));
        }
        let engine = Arc::clone(&self.rhai.engine);
        let ast = engine.compile_with_scope(&self.rhai.scope, source).map_err(|e| (e.to_string(), Vec::new()))?;
        self.rhai.busy = true;
        self.rhai.stop = Arc::new(AtomicBool::new(false));
        self.rhai.finished = None;
        Ok((engine, Arc::new(ast)))
    }

    /// Start a script on a worker thread (the Script Editor's Run): the front end calls
    /// [`Self::rhai_pump`] every frame to serve its calls, [`Self::rhai_stop`] to stop it, and
    /// [`Self::rhai_take_finished`] for its result. Where threads can't start (the browser) the
    /// error says so and the caller runs it with `rhai` instead.
    pub fn rhai_start(&mut self, source: &str) -> Result<(), String> {
        let (engine, ast) = self.rhai_prepare(source).map_err(|(e, _)| e)?;
        let mut scope = std::mem::take(&mut self.rhai.scope);
        let handlers = std::mem::take(&mut self.rhai.handlers);
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (done_tx, done_rx) = mpsc::channel::<JobDone>();
        let stop = Arc::clone(&self.rhai.stop);
        let handed: Arc<std::sync::Mutex<Option<(Scope<'static>, Vec<Handler>)>>> =
            Arc::new(std::sync::Mutex::new(Some((std::mem::take(&mut scope), handlers))));
        let theirs = Arc::clone(&handed);
        let spawned = std::thread::Builder::new().name("rhai-script".into()).spawn(move || {
            let Some((mut scope, handlers)) = theirs.lock().ok().and_then(|mut g| g.take()) else { return };
            let ctl = RunCtl { stop, deadline: None, budget: Duration::ZERO, remote: Some(req_tx), ast: Arc::clone(&ast) };
            begin_run(Vec::new(), handlers, ctl);
            let result = engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast).map(|v| dynamic_to_json(&v)).map_err(|e| error_text(&e));
            let (handlers, output) = end_run();
            let _ = done_tx.send(JobDone { result, output, scope, handlers });
        });
        match spawned {
            Ok(_) => {
                self.rhai.job = Some(RhaiJob { requests: req_rx, done: done_rx });
                Ok(())
            }
            Err(e) => {
                // Nothing ran: the scope and handlers go back.
                if let Some((scope, handlers)) = handed.lock().ok().and_then(|mut g| g.take()) {
                    self.rhai.scope = scope;
                    self.rhai.handlers = handlers;
                }
                self.rhai.busy = false;
                Err(format!("can't start a script thread here ({e}); run it with the rhai command"))
            }
        }
    }

    /// Serve the worker script's calls for up to `budget`. True while it still runs.
    pub fn rhai_pump(&mut self, budget: Duration) -> bool {
        let Some(job) = self.rhai.job.take() else { return false };
        let start = Instant::now();
        loop {
            match job.done.try_recv() {
                Ok(done) => {
                    self.rhai_job_ended(&job, Some(done));
                    return false;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rhai_job_ended(&job, None);
                    return false;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let left = budget.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            if let Ok(req) = job.requests.recv_timeout(left.min(Duration::from_millis(2))) {
                req(self);
            }
        }
        self.rhai.job = Some(job);
        true
    }

    fn rhai_job_ended(&mut self, job: &RhaiJob, done: Option<JobDone>) {
        // Calls it left waiting don't run.
        self.rhai.stop.store(true, Ordering::Relaxed);
        while job.requests.try_recv().is_ok() {}
        self.rhai.busy = false;
        self.rhai.finished = Some(match done {
            Some(d) => {
                self.rhai.scope = d.scope;
                self.rhai.handlers = d.handlers;
                match d.result {
                    Ok(v) => Ok((v, d.output)),
                    Err(e) => Err((e, d.output)),
                }
            }
            None => Err(("the script thread ended unexpectedly; its variables are lost".into(), Vec::new())),
        });
        self.rhai.stop = Arc::new(AtomicBool::new(false));
    }

    /// The worker script's result, once it ended (taken once).
    pub fn rhai_take_finished(&mut self) -> Option<RunResult> {
        self.rhai.finished.take()
    }

    // ---- events ----

    /// Commands set off events now: handlers are registered and no script is running (a
    /// script's own `command()` calls tell theirs).
    pub(crate) fn rhai_watching(&self) -> bool {
        !self.rhai.busy && !self.rhai.handlers.is_empty()
    }

    /// The drawing before a watched command, for [`Self::rhai_command_ended`].
    pub(crate) fn rhai_before(&self) -> Option<(u64, Arc<cadaddy_doc::Drawing>)> {
        self.state().ok().map(|st| (st.uid, Arc::clone(&st.doc)))
    }

    /// Command `id` ended: queue `command_ended` (with the objects it added) and, when it changed
    /// objects, `objects_changed`.
    pub(crate) fn rhai_command_ended(&mut self, id: &str, before: Option<(u64, Arc<cadaddy_doc::Drawing>)>, ok: bool) {
        let (added, changed, removed) = match (before, self.state().ok()) {
            (Some((uid, doc)), Some(st)) if st.uid == uid => doc.model.changes(&st.doc.model),
            _ => (Vec::new(), Vec::new(), Vec::new()),
        };
        let hex = |v: &[cadaddy_doc::Handle]| -> Vec<String> {
            let mut v = v.to_vec();
            v.sort_unstable_by_key(|h| h.0);
            v.iter().take(MAX_EVENT_HANDLES).map(|h| h.hex()).collect()
        };
        let (a, c, r) = (hex(&added), hex(&changed), hex(&removed));
        self.rhai_queue(json!({ "event": "command_ended", "command": id, "ok": ok, "handles": a }));
        if !(a.is_empty() && c.is_empty() && r.is_empty()) {
            let all: Vec<&String> = a.iter().chain(&c).chain(&r).take(MAX_EVENT_HANDLES).collect();
            self.rhai_queue(json!({ "event": "objects_changed", "command": id, "handles": all, "added": a, "changed": c, "removed": r }));
        }
    }

    /// Drawing `i` was opened: queue `drawing_opened` (when anything may handle it).
    pub(crate) fn rhai_drawing_opened(&mut self, i: usize) {
        if !self.rhai.busy && self.rhai.handlers.is_empty() {
            return;
        }
        let (title, path) = self.docs.get(i).map(|st| (st.title.clone(), st.path.clone())).unwrap_or_default();
        self.rhai_queue(json!({ "event": "drawing_opened", "title": title, "path": path }));
    }

    fn rhai_queue(&mut self, ev: Value) {
        if self.rhai.pending.len() >= MAX_PENDING {
            self.rhai.pending.remove(0);
        }
        self.rhai.pending.push(ev);
    }

    /// Run the handlers of the events queued, outside any script (after a command typed or an
    /// agent's call), within [`DEFAULT_BUDGET`].
    pub(crate) fn rhai_dispatch_pending(&mut self) {
        if self.rhai.pending.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.rhai.pending);
        if !self.rhai_watching() {
            return;
        }
        let engine = Arc::clone(&self.rhai.engine);
        let handlers = std::mem::take(&mut self.rhai.handlers);
        let Some(ast) = handlers.first().map(|h| Arc::clone(&h.ast)) else { return };
        self.rhai.busy = true;
        self.rhai.stop = Arc::new(AtomicBool::new(false));
        let ctl =
            RunCtl { stop: Arc::clone(&self.rhai.stop), deadline: Some(Instant::now() + DEFAULT_BUDGET), budget: DEFAULT_BUDGET, remote: None, ast };
        begin_run(Vec::new(), handlers, ctl);
        let was = self.in_script;
        self.in_script = true;
        let _ = SESSION.with(|slot| slot.scoped(self, || dispatch(&engine, events)));
        self.in_script = was;
        let (handlers, _) = end_run();
        self.rhai.handlers = handlers;
        self.rhai.busy = false;
    }
}

/// `command(id, params)` from a script: run it, tell the handlers what it did.
fn script_command(ctx: &NativeCallContext, id: String, params: Value) -> Result<Dynamic, Box<EvalAltResult>> {
    if let Some(why) = stopped_now() {
        return Err(why.into());
    }
    if id.eq_ignore_ascii_case("secureload.allow") {
        return Err("secureload.allow can't be used from a script".into());
    }
    let watching = !IN_HANDLER.get() && HANDLERS.with_borrow(|h| !h.is_empty());
    let (result, events) = with_session(move |s| {
        let before = if watching { s.rhai_before() } else { None };
        let r = s.execute(&id, &params);
        if watching {
            s.rhai_command_ended(&id, before, r.is_ok());
        }
        (r.map_err(|e| e.to_string()), std::mem::take(&mut s.rhai.pending))
    })?;
    if watching {
        dispatch(ctx.engine(), events);
    }
    result.map(|v| json_to_dynamic(&v)).map_err(Into::into)
}

/// Echo a prompt (getpoint and the like).
fn prompt_line(prompt: &str) {
    if !prompt.is_empty() {
        let p = prompt.to_string();
        let _ = with_session(move |s| s.echo(p));
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
    rhai::packages::BasicFnPackage::new().register_into_engine(&mut engine);

    // Safety limits.
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_string_size(1024 * 1024);
    engine.set_max_array_size(100_000);
    engine.set_max_map_size(10_000);
    engine.set_max_modules(MAX_MODULES);

    // ---- Stop and the time budget: checked as the script runs ----
    engine.on_progress(|ops| if ops % 256 == 0 { stopped_now().map(Dynamic::from) } else { None });

    // ---- print / debug → session log ----
    engine.on_print(|text| print_line(text.to_string()));
    engine.on_debug(|text, src, pos| {
        let loc = match (src, pos) {
            (Some(src), pos) if !pos.is_none() => format!("[{src}:{pos}] "),
            (Some(src), _) => format!("[{src}] "),
            (None, pos) if !pos.is_none() => format!("[{pos}] "),
            _ => String::new(),
        };
        print_line(format!("{loc}{text}"));
    });

    // ---- command(id, params) → result ----
    engine.register_fn("command", |ctx: NativeCallContext, id: &str, params: Map| script_command(&ctx, id.to_string(), map_to_json(&params)));

    // ---- command(id) — no params ----
    engine.register_fn("command", |ctx: NativeCallContext, id: &str| script_command(&ctx, id.to_string(), json!({})));

    // ---- getvar(name) ----
    engine.register_fn("getvar", |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        let name = name.to_string();
        with_session(move |s| crate::sysvars::get(s, &name).ok_or_else(|| format!("{name} is not a system variable")))?
            .map(|v| json_to_dynamic(&v))
            .map_err(Into::into)
    });

    // ---- setvar(name, value) ----
    engine.register_fn("setvar", |name: &str, val: Dynamic| -> Result<(), Box<EvalAltResult>> {
        let jval = dynamic_to_json(&val);
        let name = name.to_string();
        with_session(move |s| crate::sysvars::set(s, &name, &jval).map_err(|e| e.to_string()))?.map_err(Into::into)
    });

    // ---- echo(text) ----
    engine.register_fn("echo", |text: &str| print_line(text.to_string()));

    // ---- selection() → [handle strings] ----
    engine.register_fn("selection", || -> Result<Dynamic, Box<EvalAltResult>> {
        let handles = with_session(|s| s.selection().iter().map(|h| h.hex()).collect::<Vec<_>>())?;
        Ok(Dynamic::from_array(handles.into_iter().map(Dynamic::from).collect()))
    });

    // ---- ssget() → current selection as array ----
    engine.register_fn("ssget", || -> Result<Dynamic, Box<EvalAltResult>> {
        let handles = with_session(|s| s.selection().iter().map(|h| h.hex()).collect::<Vec<_>>())?;
        Ok(Dynamic::from_array(handles.into_iter().map(Dynamic::from).collect()))
    });

    // ---- entity(handle) → the object's properties (as PROPERTIES gives them) ----
    engine.register_fn("entity", |handle: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        let h = handle.to_string();
        let r = with_session(move |s| s.execute("properties", &json!({ "handles": [h] })).map_err(|e| e.to_string()))??;
        r.get("objects")
            .and_then(Value::as_array)
            .and_then(|o| o.first())
            .map(json_to_dynamic)
            .ok_or_else(|| format!("entity: no object with handle {handle}").into())
    });

    // ---- set_entity(handle, map) → change the object's properties (as PROPERTIES.SET) ----
    engine.register_fn("set_entity", |ctx: NativeCallContext, handle: &str, changes: Map| -> Result<Dynamic, Box<EvalAltResult>> {
        let mut params = map_to_json(&changes);
        if let Some(o) = params.as_object_mut() {
            o.remove("handle");
            o.insert("handles".into(), json!([handle]));
        }
        script_command(&ctx, "properties.set".into(), params)
    });

    // ---- on(event, fn) → handlers registered for the event; off(event) / off() ----
    engine.register_fn("on", |event: &str, f: FnPtr| -> Result<i64, Box<EvalAltResult>> {
        if !EVENTS.contains(&event) {
            return Err(format!("on: unknown event {event:?}; events are {}", EVENTS.join(", ")).into());
        }
        let ast = RUN.with_borrow(|r| r.as_ref().map(|r| Arc::clone(&r.ast))).ok_or("on: no running script")?;
        HANDLERS.with_borrow_mut(|hs| {
            if hs.len() >= MAX_HANDLERS {
                return Err(format!("on: at most {MAX_HANDLERS} handlers; remove some with off(event)").into());
            }
            hs.push(Handler { event: event.to_string(), f, ast });
            Ok(hs.iter().filter(|h| h.event == event).count() as i64)
        })
    });
    engine.register_fn("off", |event: &str| -> i64 {
        HANDLERS.with_borrow_mut(|hs| {
            let n = hs.len();
            hs.retain(|h| h.event != event);
            (n - hs.len()) as i64
        })
    });
    engine.register_fn("off", || -> i64 {
        HANDLERS.with_borrow_mut(|hs| {
            let n = hs.len();
            hs.clear();
            n as i64
        })
    });

    // ---- sslength(ss) ----
    engine.register_fn("sslength", |ss: rhai::Array| -> i64 { ss.len() as i64 });

    // ---- ssname(ss, index) ----
    engine.register_fn("ssname", |ss: rhai::Array, idx: i64| -> Result<Dynamic, Box<EvalAltResult>> {
        usize::try_from(idx)
            .ok()
            .and_then(|i| ss.get(i))
            .cloned()
            .ok_or_else(|| format!("ssname: index {idx} out of range (length {})", ss.len()).into())
    });

    // ---- getpoint(prompt) ----
    engine.register_fn("getpoint", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        prompt_line(prompt);
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
        prompt_line(prompt);
        let val = next_input("getreal")?;
        val.as_f64().map(Dynamic::from).ok_or_else(|| format!("getreal: expected number, got {val}").into())
    });

    // ---- getstring(prompt) ----
    engine.register_fn("getstring", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        prompt_line(prompt);
        let val = next_input("getstring")?;
        match &val {
            Value::String(s) => Ok(Dynamic::from(s.clone())),
            other => Ok(Dynamic::from(other.to_string())),
        }
    });

    // ---- getint(prompt) ----
    engine.register_fn("getint", |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
        prompt_line(prompt);
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
        let (val, _) = s.rhai_eval("40 + 2", vec![], None).expect("eval failed");
        assert_eq!(val, json!(42));
    }

    #[test]
    fn scope_persists() {
        let mut s = crate::Session::new();
        s.rhai_eval("let x = 10;", vec![], None).expect("first eval");
        let (val, _) = s.rhai_eval("x + 5", vec![], None).expect("second eval");
        assert_eq!(val, json!(15));
    }

    #[test]
    fn source_too_long() {
        let mut s = crate::Session::new();
        let big = "x".repeat(MAX_SOURCE_LEN + 1);
        let (err, _) = s.rhai_eval(&big, vec![], None).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn print_goes_to_log() {
        let mut s = crate::Session::new();
        let (_, output) = s.rhai_eval(r#"print("hello from rhai");"#, vec![], None).expect("eval");
        assert!(output.iter().any(|l| l.contains("hello from rhai")), "output: {output:?}");
    }
}
