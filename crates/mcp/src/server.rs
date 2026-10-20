//! JSON-RPC 2.0 framing and the MCP lifecycle / tools / resources / prompts methods. After a
//! tool call that opened or closed a drawing, `notifications/resources/list_changed` is queued
//! ([`Server::take_notifications`]; [`Server::serve`] writes them after the reply).

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::backend::Backend;
use crate::tools::{call_tool, tool_definitions};
use crate::{prompts, resources};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The longest request line read (`execute_many` with ten thousand commands is a few MB); a longer
/// one is answered with an error and skipped without being held in memory.
pub const MAX_REQUEST_BYTES: usize = 32 << 20;
/// The most requests one batch may hold.
pub const MAX_BATCH: usize = 1000;

const INSTRUCTIONS: &str = "CADaddy is a 2D/3D CAD drafting app (an AutoCAD clone). Coordinates are drawing units, \
y up. Fastest path: command_line with AutoCAD syntax, e.g. `line 0,0 10,0 10,5 c`, `circle 5,2.5 1`, \
`rectang 0,0 4,3`, `offset 1` then pick, `zoom e`. Or execute with JSON (list_commands shows ids and params). \
Verify with inspect_drawing / query_entities and look with render.";

pub struct Server {
    backend: Box<dyn Backend>,
    initialized: bool,
    /// The open drawings when last looked at, to notice opening and closing.
    drawings: Option<Vec<(String, String)>>,
    /// Notifications to send after the current reply.
    pending: Vec<Value>,
    /// The client asked to watch events (`watch_events`): they follow each tool call as
    /// `notifications/cadaddy/event`, unless another reader sends them as they happen
    /// ([`Server::with_event_watcher`]).
    watching: bool,
    event_seq: u64,
    external_events: bool,
}

/// The events after `since` from `backend` as JSON-RPC notification lines, and the next `since`.
pub fn event_notifications(backend: &mut dyn Backend, since: u64) -> (Vec<String>, u64) {
    let Ok(r) = backend.call("events.poll", json!({ "since": since })) else { return (Vec::new(), since) };
    let next = r.get("next").and_then(Value::as_u64).unwrap_or(since);
    let lines = r
        .get("events")
        .and_then(Value::as_array)
        .map(|evs| evs.iter().map(|e| json!({"jsonrpc": "2.0", "method": "notifications/cadaddy/event", "params": e}).to_string()).collect())
        .unwrap_or_default();
    (lines, next)
}

fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

impl Server {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self { backend, initialized: false, drawings: None, pending: Vec::new(), watching: false, event_seq: 0, external_events: false }
    }
    /// Events are sent by a watcher of their own as they happen (the app's events, polled on
    /// a second connection), not after each tool call.
    pub fn with_event_watcher(mut self) -> Self {
        self.external_events = true;
        self
    }
    pub fn backend(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Notifications queued since the last call (JSON-RPC lines), to send after the reply.
    pub fn take_notifications(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending).into_iter().map(|n| n.to_string()).collect()
    }

    /// After a tool call: when drawings were opened or closed since the client last listed the
    /// resources (or since the first tool call), tell it the list changed. The backend is asked
    /// only after a call, so nothing reaches it ahead of the client's own requests.
    fn notice_drawings(&mut self) {
        let now = resources::signature(self.backend.as_mut());
        let changed = self.drawings.as_ref().is_some_and(|d| *d != now);
        self.drawings = Some(now);
        if changed && self.initialized {
            self.pending.push(json!({"jsonrpc": "2.0", "method": "notifications/resources/list_changed"}));
        }
    }

    /// Serve newline-delimited JSON-RPC until `input` closes (logs to stderr only).
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        // Lines are read as bytes: one that isn't UTF-8 is a parse error, not the end of the
        // session.
        while let Some(line) = read_line_capped(&mut input, MAX_REQUEST_BYTES)? {
            let reply = match line {
                Line::TooLong(n) => Some(
                    error(Value::Null, INVALID_REQUEST, format!("request too large ({n} bytes or more; the limit is {MAX_REQUEST_BYTES})"))
                        .to_string(),
                ),
                Line::Full(line) => match std::str::from_utf8(&line) {
                    Ok(text) => self.handle_line(text),
                    Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: invalid UTF-8 ({e})")).to_string()),
                },
            };
            if let Some(reply) = reply {
                output.write_all(reply.as_bytes())?;
                output.write_all(b"\n")?;
            }
            for n in self.take_notifications() {
                output.write_all(n.as_bytes())?;
                output.write_all(b"\n")?;
            }
            output.flush()?;
        }
        Ok(())
    }

    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            // JSON-RPC 2.0: an empty batch is one Invalid Request, not silence.
            Ok(Value::Array(batch)) if batch.is_empty() => Some(error(Value::Null, INVALID_REQUEST, "empty batch")),
            Ok(Value::Array(batch)) if batch.len() > MAX_BATCH => {
                Some(error(Value::Null, INVALID_REQUEST, format!("batch too large ({} requests; the limit is {MAX_BATCH})", batch.len())))
            }
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle(m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => self.handle(msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let Value::Object(o) = &msg else { return Some(error(Value::Null, INVALID_REQUEST, "message must be an object")) };
        let id = o.get("id").cloned();
        let Some(method) = o.get("method").and_then(Value::as_str) else {
            if o.contains_key("result") || o.contains_key("error") {
                return None;
            }
            return Some(error(id.unwrap_or(Value::Null), INVALID_REQUEST, "missing `method`"));
        };
        let params = o.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            if method == "notifications/initialized" {
                self.initialized = true;
            }
            return None;
        };
        Some(match self.request(method, &params) {
            Ok(r) => response(id, r),
            Err((code, m)) => error(id, code, m),
        })
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED.contains(&asked) { asked } else { PROTOCOL_VERSION };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": true, "subscribe": false}, "prompts": {"listChanged": false}},
                    "serverInfo": {"name": "cadaddy", "title": "CADaddy", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": format!("{INSTRUCTIONS} Backend: {}.", self.backend.describe()),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tool_definitions(self.backend.has_ui())),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool `name`".to_string()))?;
                let args = params.get("arguments").unwrap_or(&Value::Null);
                let r = call_tool(self.backend.as_mut(), name, args).map_err(|m| (INVALID_PARAMS, m));
                if name == "watch_events" && r.is_ok() {
                    self.watching = args.get("on").and_then(Value::as_bool).unwrap_or(true);
                    self.event_seq = r.as_ref().ok().and_then(|v| v.get("seq")).and_then(Value::as_u64).unwrap_or(0);
                }
                self.notice_drawings();
                if self.watching && !self.external_events && self.initialized {
                    let (lines, next) = event_notifications(self.backend.as_mut(), self.event_seq);
                    self.event_seq = next;
                    self.pending.extend(lines.iter().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
                }
                r
            }
            "resources/list" => {
                // The list the client now knows: later changes to it are notified.
                let (list, sig) = resources::list(self.backend.as_mut());
                self.drawings = Some(sig);
                Ok(list)
            }
            "resources/templates/list" => Ok(resources::templates()),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing resource `uri`".to_string()))?;
                resources::read(self.backend.as_mut(), uri)
            }
            "prompts/list" => Ok(prompts::list()),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing prompt `name`".to_string()))?;
                prompts::get(name, params.get("arguments").unwrap_or(&Value::Null)).map_err(|m| (INVALID_PARAMS, m))
            }
            _ => Err((METHOD_NOT_FOUND, format!("method `{method}` not found"))),
        }
    }
}

/// A request line from [`read_line_capped`].
enum Line {
    Full(Vec<u8>),
    /// Longer than the limit: at least this many bytes, read and dropped up to the newline.
    TooLong(usize),
}

/// The next newline-ended line of `input` (the last may lack its newline), holding at most `max`
/// bytes in memory; `None` at the end of input.
fn read_line_capped(input: &mut impl BufRead, max: usize) -> std::io::Result<Option<Line>> {
    let mut line = Vec::new();
    let mut seen = 0usize;
    let mut any = false;
    loop {
        let buf = match input.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            break;
        }
        any = true;
        let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (buf.get(..i).unwrap_or(buf), Some(i + 1)),
            None => (buf, None),
        };
        seen = seen.saturating_add(chunk.len());
        if seen <= max {
            line.extend_from_slice(chunk);
        } else if !line.is_empty() {
            line = Vec::new();
        }
        let used = done.unwrap_or(buf.len());
        input.consume(used);
        if done.is_some() {
            break;
        }
    }
    Ok(match (any, seen > max) {
        (false, _) => None,
        (true, true) => Some(Line::TooLong(seen)),
        (true, false) => Some(Line::Full(line)),
    })
}

#[cfg(test)]
mod capped_tests {
    use super::*;

    #[test]
    fn long_lines_are_dropped_without_being_kept() {
        let mut input: &[u8] = b"short\n0123456789abcdef\nok\nlast";
        let mut lines = Vec::new();
        while let Some(l) = read_line_capped(&mut input, 8).unwrap() {
            lines.push(match l {
                Line::Full(b) => String::from_utf8(b).unwrap(),
                Line::TooLong(n) => format!("too long: {n}"),
            });
        }
        assert_eq!(lines, ["short", "too long: 16", "ok", "last"]);
        // Through a reader with a small buffer too.
        let data = format!("{}\n{{}}\n", "x".repeat(100));
        let mut r = std::io::BufReader::with_capacity(7, data.as_bytes());
        assert!(matches!(read_line_capped(&mut r, 50).unwrap(), Some(Line::TooLong(100))));
        assert!(matches!(read_line_capped(&mut r, 50).unwrap(), Some(Line::Full(b)) if b == b"{}"));
        assert!(read_line_capped(&mut r, 50).unwrap().is_none());
    }
}
