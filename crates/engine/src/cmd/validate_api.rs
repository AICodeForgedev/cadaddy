//! The Validate run's optional direct judge (Q6118): with no agent attached, Validate can send
//! each area's evidence packet to Anthropic's Messages API itself, with the user's own API key.
//!
//! Off by default (Options ▸ Automation ▸ "Judge with Anthropic API"); PRIVACY.md lists it, and
//! this is the only file of the engine that reaches the network (`crates/ui-egui/tests/no_network.rs`).
//!
//! - **What is sent, to whom:** one request per area to `https://api.anthropic.com/v1/messages`:
//!   the packet's `prompt.md`, its `objects.json` and `manifest.json` as text, and its pictures
//!   (print crops, model pictures, submittal pages) as base64 PNG image blocks, with one tool
//!   (`report_findings`) whose input schema is the shape `validate.report` takes. Nothing else.
//! - **How:** through the system's `curl`, like the updater. The request body and a curl config
//!   file holding the headers (`x-api-key`, `anthropic-version`, `content-type`) are written with
//!   0600 permissions into a private temporary folder and deleted afterwards; the key is never on
//!   the command line, in a log or in an error message ([`redact`]).
//! - **The key:** kept in the operating system's credential store ([`keystore`]): the macOS
//!   keychain (`security`), the Secret Service on Linux (`secret-tool`), the Windows Credential
//!   Locker (PowerShell's `PasswordVault`). Never in the drawing or the preferences.
//! - **The answer:** the `report_findings` tool call's input is handed to `validate.report` as if
//!   an agent had sent it. A refusal or an answer cut off at `max_tokens` becomes `unsure`
//!   findings saying why ([`Answer::Unsure`]); 429, 5xx (529 overloaded) and timeouts are retried
//!   with backoff, then reported for that area ([`send`]).
//!
//! The models that this was written for (Claude Opus 5.5 and later) refuse a forced
//! `tool_choice`, so the request asks for the tool with `tool_choice: auto` (one call) and says
//! so in its instructions; an answer without the tool call is `unsure` for every rule.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::validate_rules as vr;
use super::validate_run::{self as run, VERDICTS};
use crate::{Result, Session};

/// Where the requests go.
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// The `anthropic-version` header.
pub const API_VERSION: &str = "2023-06-01";
/// The model asked for unless the user changes it.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// The one tool of each request: its input is `validate.report`'s findings.
pub const TOOL: &str = "report_findings";
/// What Options ▸ Automation says before the user turns the judge on.
pub const CONSENT: &str = "Validate will send the selected areas' print crops, model pictures and object lists to Anthropic";
/// How a finding the judge could not make begins (`validate` judges such areas again on resume).
pub const NOT_JUDGED: &str = "Not judged:";

/// The answer's room, tokens (a non-streaming request).
const MAX_TOKENS: u64 = 16_000;
/// Pictures per request, at most (the API takes 100).
const MAX_IMAGES: usize = 100;
/// One picture file, at most: its base64 stays under the API's 5 MB per image.
const MAX_IMAGE_BYTES: u64 = 3_700_000;
/// The whole request body, at most (the API takes 32 MB).
const MAX_BODY_BYTES: usize = 30_000_000;
/// `objects.json` and `manifest.json`, at most, each.
const MAX_TEXT_BYTES: usize = 400_000;
/// The answer read back, at most.
const MAX_RESPONSE_BYTES: u64 = 8_000_000;
/// One request's time limit, seconds.
const TIMEOUT_S: u64 = 600;
/// Tries after the first, for 429, 5xx and timeouts.
pub const RETRIES: u32 = 4;
/// The longest wait between tries, seconds (a larger `retry-after` is capped).
const MAX_WAIT_S: u64 = 60;
/// The longest model name accepted.
const MAX_MODEL_CHARS: usize = 100;
/// The pixel and token caps the API's image sizing works with (the long edge is scaled to
/// 2576 px; about 750 pixels a token, at most 4784 a picture).
const IMAGE_EDGE: f64 = 2576.0;
const IMAGE_TOKENS_MAX: u64 = 4784;
const IMAGE_TOKENS_UNKNOWN: u64 = 1600;

const SYSTEM: &str = "You check a building model against its construction prints for CADaddy's Validate command. Each request is one area's evidence packet: the instructions (prompt.md), the model's objects there, the list of pictures, and the pictures themselves, each introduced by its file name. Judge every rule the instructions list for every element it applies to, citing the pictures by the file names given. When you are done, call the report_findings tool exactly once with every finding; do not answer in text.";

// ---------------------------------------------------------------- the key

/// An Anthropic API key. Its `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

impl ApiKey {
    /// A key as typed or pasted: letters, digits, `-` and `_`, 20 to 300 of them (surrounding
    /// spaces are dropped). The error never repeats the text.
    pub fn parse(text: &str) -> std::result::Result<ApiKey, String> {
        let t = text.trim();
        if !(20..=300).contains(&t.len()) || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err("that doesn't look like an Anthropic API key (letters, digits, - and _; it starts with sk-ant-)".into());
        }
        Ok(ApiKey(t.to_string()))
    }

    /// The key itself, for the header file and the credential store only.
    fn secret(&self) -> &str {
        &self.0
    }
}

/// `text` with every copy of the key replaced by `[key]`: what may go into an error or a log.
pub fn redact(text: &str, key: &ApiKey) -> String {
    let k = key.secret();
    if k.is_empty() { text.to_string() } else { text.replace(k, "[key]") }
}

/// A model name the request may carry: letters, digits, `.`, `-`, `_`, `:`, `@`, 1 to 100.
pub fn valid_model(m: &str) -> bool {
    let m = m.trim();
    !m.is_empty() && m.len() <= MAX_MODEL_CHARS && m.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'@'))
}

// ---------------------------------------------------------------- the request

/// One area's packet as `validate.next` wrote it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packet {
    pub dir: PathBuf,
    /// The rule ids it asks about.
    pub rules: Vec<String>,
    /// Its pictures, as the packet names them (`prints/…`, `model/…`).
    pub images: Vec<String>,
}

/// A request ready to send.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// The JSON body.
    pub body: String,
    /// Pictures in it.
    pub images: usize,
    /// Pictures left out, and why.
    pub left_out: Vec<String>,
    /// About how many input tokens it costs.
    pub est_tokens: u64,
}

/// The tool's input schema: `validate.report`'s findings, with the packet's own rule ids and
/// picture names as the only values allowed.
pub fn findings_schema(rules: &[String], images: &[String]) -> Value {
    let mut rule = json!({"type": "string", "description": "a rule id of this packet"});
    if !rules.is_empty() {
        rule["enum"] = json!(rules);
    }
    let mut image = json!({"type": "string", "description": "a picture of this packet, by its file name (prints/… for what the documents show)"});
    if !images.is_empty() {
        image["enum"] = json!(images);
    }
    json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "description": "one finding per rule and element (a pass is a finding too)",
                "items": {
                    "type": "object",
                    "properties": {
                        "rule": rule,
                        "handle": {"type": "string", "description": "the model element's handle from objects.json; none for missing, required for extra"},
                        "verdict": {"type": "string", "enum": VERDICTS},
                        "issue": {"type": "string", "description": "what is wrong, in one sentence (for a pass: what was checked)"},
                        "expected": {"type": "string", "description": "what the print says"},
                        "found": {"type": "string", "description": "what the model has"},
                        "evidence": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "image": image,
                                    "sheet": {"type": "string"},
                                    "what": {"type": "string", "description": "what that picture shows"}
                                },
                                "required": ["image", "what"]
                            }
                        },
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                        "severity": {"type": "string", "enum": vr::SEVERITIES}
                    },
                    "required": ["rule", "verdict", "issue", "evidence", "confidence"]
                }
            }
        },
        "required": ["findings"]
    })
}

fn read_capped(path: &Path, max: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut at {max} bytes)", text.get(..end).unwrap_or_default())
}

/// A PNG's size from its header, if it is one.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

/// About how many tokens the API counts for a picture of this size.
pub fn image_tokens(size: Option<(u32, u32)>) -> u64 {
    let Some((w, h)) = size.filter(|(w, h)| *w > 0 && *h > 0) else { return IMAGE_TOKENS_UNKNOWN };
    let (w, h) = (f64::from(w), f64::from(h));
    let scale = (IMAGE_EDGE / w.max(h)).min(1.0);
    (((w * scale) * (h * scale) / 750.0).ceil() as u64).clamp(1, IMAGE_TOKENS_MAX)
}

/// The request for one packet: `prompt.md`, `objects.json` and `manifest.json` as text, every
/// picture as a base64 PNG image block after its file name, the findings tool, and a closing
/// line asking for the tool call.
pub fn request(p: &Packet, model: &str) -> std::result::Result<Request, String> {
    if !valid_model(model) {
        return Err(format!("the model name isn't valid: {}", model.chars().take(MAX_MODEL_CHARS).collect::<String>()));
    }
    let prompt = std::fs::read_to_string(p.dir.join("prompt.md")).map_err(|e| format!("the packet has no prompt.md ({e})"))?;
    let objects = read_capped(&p.dir.join("objects.json"), MAX_TEXT_BYTES);
    let manifest = read_capped(&p.dir.join("manifest.json"), MAX_TEXT_BYTES);
    let mut content: Vec<Value> = vec![
        json!({"type": "text", "text": prompt}),
        json!({"type": "text", "text": format!("## objects.json\n\n```json\n{objects}\n```")}),
        json!({"type": "text", "text": format!("## manifest.json\n\n```json\n{manifest}\n```")}),
    ];
    let mut chars = content.iter().map(|c| c["text"].as_str().map_or(0, str::len)).sum::<usize>();
    let mut size = chars + 8_000;
    let mut img_tokens = 0u64;
    let mut sent: Vec<String> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    for f in &p.images {
        if sent.len() >= MAX_IMAGES {
            left_out.push(format!("{f}: more than {MAX_IMAGES} pictures"));
            continue;
        }
        let path = p.dir.join(f);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len == 0 {
            left_out.push(format!("{f}: not found"));
            continue;
        }
        if len > MAX_IMAGE_BYTES {
            left_out.push(format!("{f}: larger than the API takes ({len} bytes)"));
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            left_out.push(format!("{f}: unreadable"));
            continue;
        };
        let Some(dims) = png_size(&bytes) else {
            left_out.push(format!("{f}: not a PNG"));
            continue;
        };
        let data = super::file::base64_encode(&bytes);
        if size + data.len() + f.len() + 200 > MAX_BODY_BYTES {
            left_out.push(format!("{f}: the request would pass the API's size limit"));
            continue;
        }
        size += data.len() + f.len() + 200;
        img_tokens += image_tokens(Some(dims));
        content.push(json!({"type": "text", "text": format!("Picture `{f}`:")}));
        content.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}));
        chars += f.len() + 12;
        sent.push(f.clone());
    }
    let mut close = format!(
        "Judge every rule above for every element it applies to, then call the `{TOOL}` tool once with all the findings (its input is the `findings` list of the JSON shown in prompt.md). Cite pictures only by the file names given."
    );
    if !left_out.is_empty() {
        close.push_str(&format!(" These pictures could not be sent: {}. Say UNSURE where they would decide.", left_out.join("; ")));
    }
    chars += close.len();
    content.push(json!({"type": "text", "text": close}));
    let body = json!({
        "model": model.trim(),
        "max_tokens": MAX_TOKENS,
        "output_config": {"effort": "high"},
        "system": SYSTEM,
        "tools": [{
            "name": TOOL,
            "description": "Report the findings for this area: one per rule and element, each with its verdict, what the print says, what the model has, and the pictures it rests on.",
            "input_schema": findings_schema(&p.rules, &sent),
        }],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        "messages": [{"role": "user", "content": content}],
    });
    let body = body.to_string();
    let est_tokens = (chars as u64).div_ceil(3) + img_tokens + 1_500;
    Ok(Request { body, images: sent.len(), left_out, est_tokens })
}

// ---------------------------------------------------------------- the answer

/// What the judge said about an area.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The `report_findings` call's findings, as `validate.report` takes them.
    Findings(Vec<Value>),
    /// No findings to keep, and why (a refusal, an answer cut off, no tool call).
    Unsure(String),
}

fn cut(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.len() < s.len() { format!("{t}…") } else { t }
}

/// Read a Messages API answer (a 200's body).
pub fn parse_response(body: &str) -> std::result::Result<Answer, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| format!("the answer isn't JSON: {}", cut(body, 120)))?;
    if v["type"] == "error" {
        return Err(format!("{}: {}", v["error"]["type"].as_str().unwrap_or("error"), cut(v["error"]["message"].as_str().unwrap_or(""), 200)));
    }
    let stop = v["stop_reason"].as_str().unwrap_or_default();
    match stop {
        "refusal" => {
            let why = v["stop_details"]["category"].as_str().map(|c| format!(" ({c})")).unwrap_or_default();
            return Ok(Answer::Unsure(format!("Anthropic declined to judge this area{why}")));
        }
        "max_tokens" => return Ok(Answer::Unsure(format!("the answer was cut off at {MAX_TOKENS} tokens before the findings were complete"))),
        _ => {}
    }
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    if let Some(input) = blocks.iter().find(|b| b["type"] == "tool_use" && b["name"] == TOOL).map(|b| &b["input"]) {
        return match input["findings"].as_array() {
            Some(list) => Ok(Answer::Findings(list.clone())),
            None => Ok(Answer::Unsure(format!("the {TOOL} call had no findings list"))),
        };
    }
    let text: String = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ");
    Ok(Answer::Unsure(if text.trim().is_empty() {
        format!("the judge answered without calling {TOOL}")
    } else {
        format!("the judge answered without calling {TOOL}: {}", cut(text.trim(), 200))
    }))
}

/// `unsure` findings for every rule of a packet, saying why it wasn't judged: what
/// `validate.report` keeps for an area the judge couldn't answer.
pub fn unsure_findings(p: &Packet, reason: &str) -> Vec<Value> {
    let Some(image) = p.images.iter().find(|i| i.starts_with("prints/")).or_else(|| p.images.first()) else { return Vec::new() };
    p.rules
        .iter()
        .map(|r| {
            json!({
                "rule": r,
                "verdict": "unsure",
                "issue": format!("{NOT_JUDGED} {}", cut(reason, 400)),
                "expected": "",
                "found": "",
                "evidence": [{"image": image, "what": "not judged"}],
                "confidence": 0.0,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- sending

/// A reply as the transport got it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// The `retry-after` header, seconds.
    pub retry_after: Option<u64>,
}

/// Why a request didn't get a reply.
#[derive(Clone, Debug, PartialEq)]
pub enum SendError {
    Timeout,
    Failed(String),
}

/// How a request reaches the API: [`Curl`] in the app, a fake in tests.
pub trait Transport: Send + Sync {
    fn post(&self, key: &ApiKey, body: &str) -> std::result::Result<Reply, SendError>;
}

/// A judge that gave up on an area.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    /// The HTTP status, when there was a reply.
    pub status: Option<u16>,
    /// What to tell the user (never the key).
    pub message: String,
    /// Every area would fail the same way (a refused key, an unknown model): stop the run.
    pub fatal: bool,
}

fn status_text(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().map(|m| cut(m, 200));
    let what = match status {
        401 => "the API key was refused",
        402 => "a billing problem with the Anthropic account",
        403 => "the API key isn't allowed to do this",
        404 => "the model wasn't found (Options ▸ Automation names it)",
        413 => "the packet is too large for one request",
        429 => "rate limited",
        529 => "Anthropic's API is overloaded",
        500..=599 => "Anthropic's API had an error",
        _ => "the request was refused",
    };
    match msg {
        Some(m) if !m.is_empty() => format!("{status}: {what} ({m})"),
        _ => format!("{status}: {what}"),
    }
}

/// Send one request: 429, 5xx and timeouts are tried again after 2, 4, 8, 16 s (or the
/// `retry-after` the API gives, up to a minute), [`RETRIES`] times; `cancel` stops the waiting.
/// `sleep` waits (the real one in the app, a recorder in tests). Errors never hold the key.
pub fn send(t: &dyn Transport, key: &ApiKey, body: &str, cancel: &AtomicBool, sleep: &dyn Fn(Duration)) -> std::result::Result<Answer, ApiError> {
    let mut attempt = 0u32;
    loop {
        let (err, retry_after) = match t.post(key, body) {
            Ok(r) if r.status == 200 => {
                return parse_response(&r.body).map_err(|m| ApiError { status: Some(200), message: redact(&m, key), fatal: false });
            }
            Ok(r) => {
                let e =
                    ApiError { status: Some(r.status), message: redact(&status_text(r.status, &r.body), key), fatal: matches!(r.status, 401..=404) };
                if !(r.status == 429 || (500..=599).contains(&r.status)) {
                    return Err(e);
                }
                (e, r.retry_after)
            }
            Err(SendError::Timeout) => (ApiError { status: None, message: format!("no answer within {TIMEOUT_S} s"), fatal: false }, None),
            Err(SendError::Failed(m)) => return Err(ApiError { status: None, message: redact(&m, key), fatal: true }),
        };
        if attempt >= RETRIES || cancel.load(Ordering::Relaxed) {
            let tries = attempt + 1;
            return Err(ApiError { message: format!("{} (after {tries} tries)", err.message), ..err });
        }
        let wait = retry_after.unwrap_or(2u64 << attempt).clamp(1, MAX_WAIT_S);
        // Waits in quarter seconds, so Cancel takes effect at once.
        for _ in 0..wait * 4 {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            sleep(Duration::from_millis(250));
        }
        attempt += 1;
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A private temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let p = std::env::temp_dir().join(format!("cadaddy-api-{}-{}-{nanos}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a file only this user can read (0600 on Unix; the temporary folder is the user's own on
/// Windows).
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(data)
}

/// The curl config file's text: the three headers (the key among them).
fn header_config(key: &ApiKey) -> String {
    format!("header = \"x-api-key: {}\"\nheader = \"anthropic-version: {API_VERSION}\"\nheader = \"content-type: application/json\"\n", key.secret())
}

/// curl's arguments for one request: the headers come from `config`, the body from `body`, the
/// answer goes to `out` and its headers to `headers`; stdout gets the HTTP status. No key.
pub fn curl_args(url: &str, config: &Path, body: &Path, out: &Path, headers: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> =
        ["-sS", "--max-time", &TIMEOUT_S.to_string(), "--max-filesize", &MAX_RESPONSE_BYTES.to_string()].iter().map(OsString::from).collect();
    a.push("-K".into());
    a.push(config.as_os_str().to_owned());
    let mut data = OsString::from("@");
    data.push(body.as_os_str());
    a.push("--data-binary".into());
    a.push(data);
    a.push("-o".into());
    a.push(out.as_os_str().to_owned());
    a.push("-D".into());
    a.push(headers.as_os_str().to_owned());
    a.push("-w".into());
    a.push("%{http_code}".into());
    a.push(url.into());
    a
}

fn retry_after(headers: &str) -> Option<u64> {
    headers.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case("retry-after") {
            v.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).map(|s| s.ceil() as u64)
        } else {
            None
        }
    })
}

/// The real transport: the system's curl.
#[derive(Clone, Debug)]
pub struct Curl {
    pub url: String,
}

impl Default for Curl {
    fn default() -> Self {
        Curl { url: API_URL.into() }
    }
}

impl Transport for Curl {
    fn post(&self, key: &ApiKey, body: &str) -> std::result::Result<Reply, SendError> {
        let failed = |e: String| SendError::Failed(redact(&e, key));
        let dir = TempDir::new().map_err(|e| failed(format!("couldn't make a temporary folder ({e})")))?;
        let (config, body_file, out, headers) =
            (dir.0.join("headers.cfg"), dir.0.join("body.json"), dir.0.join("answer.json"), dir.0.join("answer.headers"));
        write_private(&config, header_config(key).as_bytes()).map_err(|e| failed(format!("couldn't write the request ({e})")))?;
        write_private(&body_file, body.as_bytes()).map_err(|e| failed(format!("couldn't write the request ({e})")))?;
        let mut c = std::process::Command::new("curl");
        c.args(curl_args(&self.url, &config, &body_file, &out, &headers));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up.
            c.creation_flags(0x0800_0000);
        }
        let o = c.output().map_err(|e| failed(format!("curl isn't available ({e})")))?;
        if o.status.code() == Some(28) {
            return Err(SendError::Timeout);
        }
        if !o.status.success() {
            let err = String::from_utf8_lossy(&o.stderr);
            return Err(failed(format!("curl failed ({}): {}", o.status.code().unwrap_or(-1), cut(err.trim(), 200))));
        }
        let status = String::from_utf8_lossy(&o.stdout).trim().parse::<u16>().unwrap_or(0);
        let answer = std::fs::read(&out).unwrap_or_default();
        if answer.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(failed("the answer was too large".into()));
        }
        let hdrs = std::fs::read_to_string(&headers).unwrap_or_default();
        Ok(Reply { status, body: String::from_utf8_lossy(&answer).into_owned(), retry_after: retry_after(&hdrs) })
    }
}

// ---------------------------------------------------------------- the run

/// The next area of a run, packed for the API.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prepared {
    pub run: String,
    pub area: String,
    pub label: String,
    pub packet: Packet,
    pub request: Request,
    /// Areas left after this one.
    pub remaining: usize,
}

fn manifest_images(dir: &Path) -> Vec<String> {
    let m: Value = std::fs::read_to_string(dir.join("manifest.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    ["prints", "model", "submittals"]
        .iter()
        .flat_map(|k| m[*k].as_array().cloned().unwrap_or_default())
        .filter_map(|x| x["file"].as_str().map(str::to_string))
        .collect()
}

/// Hand out the run's next area (`validate.next`) and pack it as a request; `None` when every
/// area is judged.
pub fn prepare(s: &mut Session, run_id: Option<&str>, model: &str) -> Result<Option<Prepared>> {
    let mut q = json!({});
    if let Some(r) = run_id {
        q["run"] = json!(r);
    }
    let next = s.execute(run::CMD_NEXT, &q)?;
    if next["done"] == true {
        return Ok(None);
    }
    let dir = PathBuf::from(next["packet"]["dir"].as_str().unwrap_or_default());
    let rules: Vec<String> = next["rules"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect();
    let packet = Packet { images: manifest_images(&dir), dir, rules };
    let request = request(&packet, model).map_err(|e| super::bad(run::CMD_NEXT, e))?;
    Ok(Some(Prepared {
        run: next["run"].as_str().unwrap_or_default().to_string(),
        area: next["area"]["id"].as_str().unwrap_or_default().to_string(),
        label: next["area"]["label"].as_str().unwrap_or_default().to_string(),
        packet,
        request,
        remaining: next["remaining"].as_u64().unwrap_or(0) as usize,
    }))
}

/// Keep what the judge said about a prepared area (`validate.report`). Findings the report
/// refuses (a made-up handle, say) are kept as `unsure` with the refusal; an API error after its
/// retries is kept as `unsure` too, except a fatal one, which leaves the area for a later try.
/// Returns `validate.report`'s answer, or `None` when nothing was kept.
pub fn finish(s: &mut Session, p: &Prepared, answer: std::result::Result<Answer, ApiError>) -> Result<Option<Value>> {
    let report = |s: &mut Session, findings: Vec<Value>| s.execute(run::CMD_REPORT, &json!({"run": p.run, "area": p.area, "findings": findings}));
    let unsure = |reason: &str| unsure_findings(&p.packet, reason);
    match answer {
        Err(e) if e.fatal => Ok(None),
        Err(e) => report(s, unsure(&e.message)).map(Some),
        Ok(Answer::Unsure(why)) => report(s, unsure(&why)).map(Some),
        Ok(Answer::Findings(list)) => match report(s, list) {
            Ok(v) => Ok(Some(v)),
            Err(e) => report(s, unsure(&format!("the judge's findings were refused: {e}"))).map(Some),
        },
    }
}

// ---------------------------------------------------------------- the credential store

/// The API key in the operating system's credential store.
pub mod keystore {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::{ApiKey, redact};

    /// The entry's service and account names.
    pub const SERVICE: &str = "cadaddy-anthropic-api-key";
    pub const ACCOUNT: &str = "validate";

    /// The commands each platform runs: (program, arguments, what goes to its standard input).
    /// The key is only ever in the input, never in the arguments.
    #[derive(Debug, PartialEq)]
    pub struct Call {
        pub program: &'static str,
        pub args: Vec<String>,
        pub input: Option<String>,
    }

    #[cfg(target_os = "windows")]
    fn ps(script: &str, input: Option<String>) -> Call {
        Call { program: "powershell.exe", args: ["-NoProfile", "-NonInteractive", "-Command", script].iter().map(|s| s.to_string()).collect(), input }
    }

    #[cfg(target_os = "windows")]
    const VAULT: &str = "[void][Windows.Security.Credentials.PasswordVault,Windows.Security.Credentials,ContentType=WindowsRuntime]; $v = New-Object Windows.Security.Credentials.PasswordVault;";

    /// Read the key.
    pub fn load_call() -> Call {
        #[cfg(target_os = "macos")]
        return Call {
            program: "security",
            args: vec!["find-generic-password".into(), "-s".into(), SERVICE.into(), "-a".into(), ACCOUNT.into(), "-w".into()],
            input: None,
        };
        #[cfg(target_os = "windows")]
        return ps(
            &format!(
                "{VAULT} try {{ $c = $v.Retrieve('{SERVICE}', '{ACCOUNT}'); $c.RetrievePassword(); [Console]::Out.Write($c.Password) }} catch {{ exit 44 }}"
            ),
            None,
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Call {
            program: "secret-tool",
            args: vec!["lookup".into(), "service".into(), SERVICE.into(), "account".into(), ACCOUNT.into()],
            input: None,
        };
    }

    /// Store the key (replacing one stored before).
    pub fn save_call(key: &ApiKey) -> Call {
        #[cfg(target_os = "macos")]
        return Call {
            program: "security",
            args: vec!["-i".into()],
            input: Some(format!("add-generic-password -U -s {SERVICE} -a {ACCOUNT} -l {SERVICE} -w {}\n", key.secret())),
        };
        #[cfg(target_os = "windows")]
        return ps(
            &format!(
                "{VAULT} $k = [Console]::In.ReadLine(); try {{ $v.Remove($v.Retrieve('{SERVICE}', '{ACCOUNT}')) }} catch {{ }}; $v.Add((New-Object Windows.Security.Credentials.PasswordCredential('{SERVICE}', '{ACCOUNT}', $k)))"
            ),
            Some(format!("{}\n", key.secret())),
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Call {
            program: "secret-tool",
            args: vec![
                "store".into(),
                "--label=CADaddy Anthropic API key".into(),
                "service".into(),
                SERVICE.into(),
                "account".into(),
                ACCOUNT.into(),
            ],
            input: Some(key.secret().to_string()),
        };
    }

    /// Remove the key.
    pub fn clear_call() -> Call {
        #[cfg(target_os = "macos")]
        return Call {
            program: "security",
            args: vec!["delete-generic-password".into(), "-s".into(), SERVICE.into(), "-a".into(), ACCOUNT.into()],
            input: None,
        };
        #[cfg(target_os = "windows")]
        return ps(&format!("{VAULT} try {{ $v.Remove($v.Retrieve('{SERVICE}', '{ACCOUNT}')) }} catch {{ exit 44 }}"), None);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Call {
            program: "secret-tool",
            args: vec!["clear".into(), "service".into(), SERVICE.into(), "account".into(), ACCOUNT.into()],
            input: None,
        };
    }

    /// Run a call: (exit code, standard output).
    fn run(c: &Call) -> Result<(Option<i32>, String), String> {
        let mut cmd = Command::new(c.program);
        cmd.args(&c.args).stdin(if c.input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("the credential store ({}) isn't available: {e}", c.program))?;
        if let (Some(input), Some(mut stdin)) = (&c.input, child.stdin.take()) {
            let _ = stdin.write_all(input.as_bytes());
        }
        let o = child.wait_with_output().map_err(|e| format!("the credential store ({}) failed: {e}", c.program))?;
        Ok((o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned()))
    }

    /// The stored key, if there is one.
    pub fn load() -> Result<Option<ApiKey>, String> {
        let (code, out) = run(&load_call())?;
        match code {
            Some(0) => Ok(ApiKey::parse(out.trim()).ok()),
            // macOS's "not found"; secret-tool says nothing and exits 1; ours on Windows.
            Some(44) | Some(1) => Ok(None),
            c => Err(format!("the credential store couldn't be read (exit {})", c.unwrap_or(-1))),
        }
    }

    /// Store `key` (replacing one stored before). The error never holds the key.
    pub fn save(key: &ApiKey) -> Result<(), String> {
        let (code, out) = run(&save_call(key)).map_err(|e| redact(&e, key))?;
        // `security -i` exits 0 even when a command fails: check that the key reads back.
        if code == Some(0) && load().ok().flatten().as_ref() == Some(key) {
            return Ok(());
        }
        let said = redact(out.trim(), key);
        Err(format!(
            "the credential store didn't keep the key{}",
            if said.is_empty() { String::new() } else { format!(" ({})", said.chars().take(160).collect::<String>()) }
        ))
    }

    /// Remove the stored key (no error when there was none).
    pub fn clear() -> Result<(), String> {
        let (code, _) = run(&clear_call())?;
        match code {
            Some(0) | Some(44) | Some(1) => Ok(()),
            c => Err(format!("the credential store couldn't remove the key (exit {})", c.unwrap_or(-1))),
        }
    }

    /// Whether a key is stored (`false` when the store can't be read).
    pub fn has_key() -> bool {
        load().ok().flatten().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const KEY: &str = "sk-ant-api03-TESTKEY_abcdefghijklmnopqrstuvwxyz0123456789";

    fn key() -> ApiKey {
        ApiKey::parse(KEY).unwrap()
    }

    /// A made-up 2×1 PNG header and a little more (enough for the size and the bytes).
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 1, 2, 3, 4]);
        b
    }

    fn packet(name: &str) -> Packet {
        let dir = std::env::temp_dir().join(format!("cadaddy-vapi-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prints")).unwrap();
        std::fs::create_dir_all(dir.join("model")).unwrap();
        std::fs::write(dir.join("prompt.md"), "# Check the model against the prints: 101 Office\n").unwrap();
        std::fs::write(dir.join("objects.json"), "{\"elements\": [{\"handle\": \"2A\"}]}").unwrap();
        std::fs::write(dir.join("manifest.json"), "{\"prints\": [{\"file\": \"prints/01_E-101_plan.png\"}]}").unwrap();
        std::fs::write(dir.join("prints/01_E-101_plan.png"), png(2, 1)).unwrap();
        std::fs::write(dir.join("model/plan.png"), png(4000, 1000)).unwrap();
        Packet { dir, rules: vec!["E-REC-LOC".into()], images: vec!["prints/01_E-101_plan.png".into(), "model/plan.png".into()] }
    }

    #[test]
    fn the_request_body_is_built_from_the_packet() {
        let p = packet("body");
        let r = request(&p, DEFAULT_MODEL).unwrap();
        let v: Value = serde_json::from_str(&r.body).unwrap();
        let b64 = |f: &str| super::super::file::base64_encode(&std::fs::read(p.dir.join(f)).unwrap());
        let golden = json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "output_config": {"effort": "high"},
            "system": SYSTEM,
            "tools": [{
                "name": "report_findings",
                "description": "Report the findings for this area: one per rule and element, each with its verdict, what the print says, what the model has, and the pictures it rests on.",
                "input_schema": findings_schema(&p.rules, &p.images),
            }],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "# Check the model against the prints: 101 Office\n"},
                {"type": "text", "text": "## objects.json\n\n```json\n{\"elements\": [{\"handle\": \"2A\"}]}\n```"},
                {"type": "text", "text": "## manifest.json\n\n```json\n{\"prints\": [{\"file\": \"prints/01_E-101_plan.png\"}]}\n```"},
                {"type": "text", "text": "Picture `prints/01_E-101_plan.png`:"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64("prints/01_E-101_plan.png")}},
                {"type": "text", "text": "Picture `model/plan.png`:"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64("model/plan.png")}},
                {"type": "text", "text": "Judge every rule above for every element it applies to, then call the `report_findings` tool once with all the findings (its input is the `findings` list of the JSON shown in prompt.md). Cite pictures only by the file names given."},
            ]}],
        });
        assert_eq!(v, golden);
        assert_eq!(r.images, 2);
        assert!(r.left_out.is_empty());
        // The tool's schema: the packet's rules and pictures are the only values allowed.
        let item = &golden["tools"][0]["input_schema"]["properties"]["findings"]["items"];
        assert_eq!(item["properties"]["rule"]["enum"], json!(["E-REC-LOC"]));
        assert_eq!(item["properties"]["evidence"]["items"]["properties"]["image"]["enum"], json!(p.images));
        assert_eq!(item["properties"]["verdict"]["enum"], json!(["pass", "fail", "missing", "extra", "unsure"]));
        assert_eq!(item["required"], json!(["rule", "verdict", "issue", "evidence", "confidence"]));
        // The estimate: the text, a tiny picture (1 token) and a 4000×1000 one scaled to 2576 px.
        assert_eq!(image_tokens(Some((2, 1))), 1);
        assert_eq!(image_tokens(Some((4000, 1000))), 2212);
        assert!(r.est_tokens > 2213 && r.est_tokens < 6000, "{}", r.est_tokens);
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn pictures_the_api_cannot_take_are_left_out_and_said() {
        let mut p = packet("leftout");
        std::fs::write(p.dir.join("model/notpng.png"), b"GIF89a....").unwrap();
        p.images.push("model/notpng.png".into());
        p.images.push("model/missing.png".into());
        let r = request(&p, "claude-opus-5-5").unwrap();
        assert_eq!(r.images, 2);
        assert_eq!(r.left_out, vec!["model/notpng.png: not a PNG".to_string(), "model/missing.png: not found".to_string()]);
        assert!(r.body.contains("These pictures could not be sent: model/notpng.png: not a PNG; model/missing.png: not found"));
        // The schema offers only the pictures sent.
        let v: Value = serde_json::from_str(&r.body).unwrap();
        let images =
            &v["tools"][0]["input_schema"]["properties"]["findings"]["items"]["properties"]["evidence"]["items"]["properties"]["image"]["enum"];
        assert_eq!(images, &json!(["prints/01_E-101_plan.png", "model/plan.png"]));
        assert!(request(&p, "bad model\nname").is_err());
        assert!(request(&p, "").is_err());
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn a_tool_call_becomes_findings_and_a_refusal_or_cut_off_answer_unsure() {
        let finding = json!({"rule": "E-REC-LOC", "handle": "2A", "verdict": "fail", "issue": "2 ft off", "evidence": [{"image": "prints/01_E-101_plan.png", "what": "the symbol"}], "confidence": 0.8});
        let ok = json!({"type": "message", "stop_reason": "tool_use", "content": [
            {"type": "thinking", "thinking": ""},
            {"type": "text", "text": "Checking."},
            {"type": "tool_use", "id": "t1", "name": "report_findings", "input": {"findings": [finding]}}
        ]});
        assert_eq!(parse_response(&ok.to_string()).unwrap(), Answer::Findings(vec![finding]));
        let refusal = json!({"type": "message", "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}, "content": []});
        assert_eq!(parse_response(&refusal.to_string()).unwrap(), Answer::Unsure("Anthropic declined to judge this area (cyber)".into()));
        let cut_off =
            json!({"type": "message", "stop_reason": "max_tokens", "content": [{"type": "tool_use", "name": "report_findings", "input": {}}]});
        assert_eq!(
            parse_response(&cut_off.to_string()).unwrap(),
            Answer::Unsure("the answer was cut off at 16000 tokens before the findings were complete".into())
        );
        let text_only = json!({"type": "message", "stop_reason": "end_turn", "content": [{"type": "text", "text": "Everything passes."}]});
        assert_eq!(
            parse_response(&text_only.to_string()).unwrap(),
            Answer::Unsure("the judge answered without calling report_findings: Everything passes.".into())
        );
        let no_list =
            json!({"type": "message", "stop_reason": "tool_use", "content": [{"type": "tool_use", "name": "report_findings", "input": {"x": 1}}]});
        assert_eq!(parse_response(&no_list.to_string()).unwrap(), Answer::Unsure("the report_findings call had no findings list".into()));
        assert!(parse_response("<html>").is_err());
        let err = json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}});
        assert_eq!(parse_response(&err.to_string()), Err("overloaded_error: Overloaded".into()));
        // Unsure findings: one per rule, citing a print, with the reason.
        let p = packet("unsure");
        let u = unsure_findings(&p, "Anthropic declined to judge this area (cyber)");
        assert_eq!(u.len(), 1);
        assert_eq!(u[0]["verdict"], "unsure");
        assert_eq!(u[0]["issue"], "Not judged: Anthropic declined to judge this area (cyber)");
        assert_eq!(u[0]["evidence"][0]["image"], "prints/01_E-101_plan.png");
        assert_eq!(u[0]["confidence"], 0.0);
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    /// Replies in order, recording what each post saw.
    struct Fake {
        replies: Mutex<Vec<std::result::Result<Reply, SendError>>>,
        posts: Mutex<usize>,
    }

    impl Transport for Fake {
        fn post(&self, _key: &ApiKey, _body: &str) -> std::result::Result<Reply, SendError> {
            *self.posts.lock().unwrap() += 1;
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() { Err(SendError::Failed("no more".into())) } else { r.remove(0) }
        }
    }

    fn fake(replies: Vec<std::result::Result<Reply, SendError>>) -> Fake {
        Fake { replies: Mutex::new(replies), posts: Mutex::new(0) }
    }

    fn reply(status: u16, body: Value, retry_after: Option<u64>) -> std::result::Result<Reply, SendError> {
        Ok(Reply { status, body: body.to_string(), retry_after })
    }

    fn ok_reply() -> std::result::Result<Reply, SendError> {
        reply(200, json!({"stop_reason": "tool_use", "content": [{"type": "tool_use", "name": "report_findings", "input": {"findings": []}}]}), None)
    }

    #[test]
    fn overloaded_rate_limited_and_timeouts_are_retried_with_backoff() {
        let waits = Mutex::new(Vec::<Duration>::new());
        let sleep = |d: Duration| waits.lock().unwrap().push(d);
        let t = fake(vec![
            reply(529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}), None),
            reply(429, json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}), Some(7)),
            Err(SendError::Timeout),
            ok_reply(),
        ]);
        let cancel = AtomicBool::new(false);
        assert_eq!(send(&t, &key(), "{}", &cancel, &sleep), Ok(Answer::Findings(vec![])));
        assert_eq!(*t.posts.lock().unwrap(), 4);
        // 2 s, then the 7 s the API asked for, then 8 s: in quarter seconds.
        let total: Duration = waits.lock().unwrap().iter().sum();
        assert_eq!(total, Duration::from_secs(2 + 7 + 8));
        // Out of tries: reported, with how many.
        let t = fake((0..=RETRIES).map(|_| reply(529, json!({"error": {"message": "Overloaded"}}), None)).collect());
        let e = send(&t, &key(), "{}", &cancel, &|_| {}).unwrap_err();
        assert_eq!(e.status, Some(529));
        assert!(!e.fatal);
        assert_eq!(e.message, "529: Anthropic's API is overloaded (Overloaded) (after 5 tries)");
        assert_eq!(*t.posts.lock().unwrap(), RETRIES as usize + 1);
        // Cancel stops the waiting at once.
        cancel.store(true, Ordering::Relaxed);
        let t = fake(vec![reply(529, json!({}), None), ok_reply()]);
        let e = send(&t, &key(), "{}", &cancel, &|_| panic!("no waiting after Cancel")).unwrap_err();
        assert_eq!(e.message, "529: Anthropic's API is overloaded (after 1 tries)");
    }

    #[test]
    fn a_refused_key_stops_at_once_and_never_shows_the_key() {
        let cancel = AtomicBool::new(false);
        // An API that repeats the key back (it doesn't, but an error must never carry it).
        let body = json!({"type": "error", "error": {"type": "authentication_error", "message": format!("invalid x-api-key {KEY}")}});
        let t = fake(vec![reply(401, body, None), ok_reply()]);
        let e = send(&t, &key(), "{}", &cancel, &|_| panic!("401 isn't retried")).unwrap_err();
        assert_eq!(*t.posts.lock().unwrap(), 1);
        assert!(e.fatal);
        assert_eq!(e.status, Some(401));
        assert!(!e.message.contains(KEY) && !e.message.contains("TESTKEY"), "{}", e.message);
        assert_eq!(e.message, "401: the API key was refused (invalid x-api-key [key])");
        let t = fake(vec![Err(SendError::Failed(format!("curl: header {KEY}")))]);
        let e = send(&t, &key(), "{}", &cancel, &|_| {}).unwrap_err();
        assert!(!e.message.contains(KEY), "{}", e.message);
        let t = fake(vec![reply(404, json!({"error": {"message": "model: nope"}}), None)]);
        let e = send(&t, &key(), "{}", &cancel, &|_| {}).unwrap_err();
        assert!(e.fatal);
        assert_eq!(e.message, "404: the model wasn't found (Options ▸ Automation names it) (model: nope)");
        let t = fake(vec![reply(413, json!({}), None)]);
        assert!(!send(&t, &key(), "{}", &cancel, &|_| {}).unwrap_err().fatal);
    }

    #[test]
    fn the_key_is_never_on_the_command_line_and_its_file_is_private() {
        let k = key();
        assert_eq!(format!("{k:?}"), "ApiKey(…)");
        assert!(ApiKey::parse("  short ").is_err());
        let e = ApiKey::parse("sk-ant-api03 with spaces inside it here").unwrap_err();
        assert!(!e.contains("with spaces"), "{e}");
        let dir = TempDir::new().unwrap();
        let (cfg, body, out, hdrs) = (dir.0.join("headers.cfg"), dir.0.join("body.json"), dir.0.join("a.json"), dir.0.join("a.h"));
        let args = curl_args(API_URL, &cfg, &body, &out, &hdrs);
        let line = args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert!(!line.contains(KEY) && !line.contains("x-api-key"), "{line}");
        assert!(line.contains("-K ") && line.contains("--data-binary @") && line.ends_with(API_URL), "{line}");
        write_private(&cfg, header_config(&k).as_bytes()).unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            text,
            format!("header = \"x-api-key: {KEY}\"\nheader = \"anthropic-version: 2023-06-01\"\nheader = \"content-type: application/json\"\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let path = dir.0.clone();
        drop(dir);
        assert!(!path.exists(), "the temporary folder is removed");
        // The credential store gets the key on its input only.
        for c in [keystore::load_call(), keystore::save_call(&k), keystore::clear_call()] {
            assert!(!c.args.iter().any(|a| a.contains(KEY)), "{:?}", c.args);
        }
        assert!(keystore::save_call(&k).input.is_some_and(|i| i.contains(KEY)));
        assert_eq!(retry_after("HTTP/2 429\r\nretry-after: 12\r\n"), Some(12));
        assert_eq!(retry_after("HTTP/2 529\r\n"), None);
    }

    /// The real transport against a made-up local server (no network): the key arrives as the
    /// x-api-key header from the config file, the body from its file, the status and
    /// retry-after come back.
    #[test]
    fn curl_sends_the_headers_from_its_config_file() {
        use std::io::{Read, Write};
        if std::process::Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/messages", l.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = c.read(&mut buf).unwrap_or(0);
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if n == 0 || (text.contains("\r\n\r\n") && text.ends_with("{\"hello\":1}")) {
                    break;
                }
            }
            let body = "{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";
            let _ = write!(
                c,
                "HTTP/1.1 529 Overloaded\r\nretry-after: 3\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            String::from_utf8_lossy(&got).into_owned()
        });
        let r = Curl { url }.post(&key(), "{\"hello\":1}").unwrap();
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase())), "{request}");
        assert!(request.contains("anthropic-version: 2023-06-01") && request.contains("content-type: application/json"), "{request}");
        assert!(request.ends_with("{\"hello\":1}"), "{request}");
        assert_eq!(r.status, 529);
        assert_eq!(r.retry_after, Some(3));
        assert!(r.body.contains("overloaded_error"));
    }
}
