//! Example CADaddy WASM plug-in.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`
//! Install: copy `target/wasm32-unknown-unknown/release/hello_plugin.wasm` to `<config>/plugins/`.
//!
//! This plug-in registers two commands:
//! - `hello.greet` — adds a text entity saying "Hello from a plug-in!" at a given point.
//! - `hello.count` — returns the number of objects in the current selection.
//!
//! ## Trampoline protocol
//!
//! `plugin_run` returns 0 (success), 1 (error) or 2 (host call needed). When it returns 2 the
//! host processes the pending request (set via `host_execute` / `host_query` / `host_selection`)
//! and calls `plugin_resume(result_code)` with the host's return code.

// The host functions we can call.
extern "C" {
    fn host_register_command(
        id_ptr: i32, id_len: i32,
        label_ptr: i32, label_len: i32,
        params_ptr: i32, params_len: i32,
    );
    fn host_execute(cmd_ptr: i32, cmd_len: i32, params_ptr: i32, params_len: i32);
    fn host_selection();
    fn host_result_len() -> i32;
    fn host_read_result(dst_ptr: i32, max_len: i32) -> i32;
    fn host_set_result(ptr: i32, len: i32);
    fn host_log(ptr: i32, len: i32);
}

// --- state machine for the trampoline ---

static mut STATE: PluginState = PluginState::Idle;
static mut HOST_RC: i32 = 0;

#[derive(Clone, Copy, PartialEq)]
enum PluginState {
    Idle,
    GreetWaitText,
    CountWaitSelection,
}

/// A simple bump allocator for the host to write into our memory.
static mut HEAP_POS: usize = 0;
static mut HEAP: [u8; 65536] = [0u8; 65536];

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    unsafe {
        let pos = HEAP_POS;
        let end = pos + size as usize;
        if end > HEAP.len() { return 0; }
        HEAP_POS = end;
        HEAP.as_ptr().add(pos) as i32
    }
}

/// Called once at load.
#[no_mangle]
pub extern "C" fn plugin_init() {
    register("hello.greet", "Hello Greet", "{point?: [x, y]}");
    register("hello.count", "Hello Count", "{}");
}

/// Called when one of our commands is invoked. Returns 0/1/2.
#[no_mangle]
pub extern "C" fn plugin_run(
    cmd_ptr: i32, cmd_len: i32,
    _params_ptr: i32, _params_len: i32,
) -> i32 {
    let cmd = read_str(cmd_ptr, cmd_len);
    match cmd.as_str() {
        "hello.greet" => {
            // Ask the host to create a TEXT entity.
            let text_params = r#"{"text":"Hello from a plug-in!","at":[5,5],"height":2.5}"#;
            call_execute("text", text_params);
            unsafe { STATE = PluginState::GreetWaitText; }
            2 // come back via plugin_resume
        }
        "hello.count" => {
            // Ask the host for the current selection.
            call_selection();
            unsafe { STATE = PluginState::CountWaitSelection; }
            2
        }
        _ => {
            set_result(r#"{"error":"unknown command"}"#);
            1
        }
    }
}

/// Called after the host processes a request. Returns 0/1/2.
#[no_mangle]
pub extern "C" fn plugin_resume(result_code: i32) -> i32 {
    unsafe { HOST_RC = result_code; }
    let state = unsafe { STATE };
    unsafe { STATE = PluginState::Idle; }

    match state {
        PluginState::GreetWaitText => {
            if result_code != 0 {
                let err = read_host_result();
                set_result(&err);
                return 1;
            }
            log("hello.greet: placed greeting text");
            let result = read_host_result();
            set_result(&result);
            0
        }
        PluginState::CountWaitSelection => {
            let result = read_host_result();
            // Count the handles in the JSON array.
            let count = result.matches('"').count() / 2; // each handle is a quoted string
            log(&format_simple("hello.count: selection has ", count, " objects"));
            let out = format_simple(r#"{"count":"#, count, "}");
            set_result(&out);
            0
        }
        PluginState::Idle => {
            set_result(r#"{"error":"unexpected resume"}"#);
            1
        }
    }
}

// ---- helpers ----

fn register(id: &str, label: &str, params: &str) {
    unsafe {
        host_register_command(
            id.as_ptr() as i32, id.len() as i32,
            label.as_ptr() as i32, label.len() as i32,
            params.as_ptr() as i32, params.len() as i32,
        );
    }
}

fn call_execute(cmd: &str, params: &str) {
    unsafe {
        host_execute(
            cmd.as_ptr() as i32, cmd.len() as i32,
            params.as_ptr() as i32, params.len() as i32,
        );
    }
}

fn call_selection() {
    unsafe { host_selection(); }
}

fn read_host_result() -> String {
    unsafe {
        let len = host_result_len();
        if len <= 0 { return String::new(); }
        let len = len as usize;
        let mut buf = vec![0u8; len];
        let got = host_read_result(buf.as_ptr() as i32, len as i32) as usize;
        buf.truncate(got);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

fn set_result(s: &str) {
    unsafe { host_set_result(s.as_ptr() as i32, s.len() as i32); }
}

fn log(s: &str) {
    unsafe { host_log(s.as_ptr() as i32, s.len() as i32); }
}

fn read_str(ptr: i32, len: i32) -> String {
    unsafe {
        let slice = core::slice::from_raw_parts(ptr as *const u8, len as usize);
        String::from_utf8_lossy(slice).into_owned()
    }
}

/// Simple string formatting without std::fmt (keeps the WASM small).
fn format_simple(prefix: &str, n: usize, suffix: &str) -> String {
    let mut s = String::from(prefix);
    // Manual number to string.
    if n == 0 {
        s.push('0');
    } else {
        let mut digits = [0u8; 20];
        let mut i = 0;
        let mut v = n;
        while v > 0 {
            digits[i] = (v % 10) as u8 + b'0';
            v /= 10;
            i += 1;
        }
        for d in digits[..i].iter().rev() {
            s.push(*d as char);
        }
    }
    s.push_str(suffix);
    s
}
