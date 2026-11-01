//! `cargo xtask mcptest [--port 47820] [--cli PATH] [--mcpb PATH]`: the MCP suite against the
//! running app (outside CI; start it with `cargo run --release -p cadaddy -- --control 47820`).
//!
//! `cadaddy-cli mcp --connect 127.0.0.1:PORT` (built here, or `--cli`) is driven over stdio, and
//! the live run covers exactly this:
//! - every tool the app's `tools/list` returns is called once normally (must succeed) and once
//!   with bad parameters (invalid params or a tool error, never accepted), from the one table
//!   shared with `crates/mcp/tests/suite.rs` (`crates/mcp/tests/support/table.rs`); a listed tool
//!   with no table entry prints a FAIL line. `quit_app` runs last, as it ends the app;
//! - every resource `resources/list` returns is read (`resources/read`, valid JSON) and every
//!   prompt `prompts/list` returns is fetched (`prompts/get`, with its required arguments);
//! - the agent guard: while a person is typing a command in the app (keys sent on its control
//!   port) the agent's typing is refused and its `execute` still works.
//!
//! With `--mcpb`, the bundle is unpacked and the server its manifest names is started and listed
//! (only `tools/list`). Prints PASS/FAIL per check.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

#[path = "../../crates/mcp/tests/support/table.rs"]
mod table;

struct Opts {
    port: u16,
    cli: Option<PathBuf>,
    mcpb: Option<PathBuf>,
}

fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut o = Opts { port: 47820, cli: None, mcpb: None };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().map(|s| (*s).to_string()).ok_or(format!("{a} needs a value"));
        match *a {
            "--port" => o.port = val()?.parse().map_err(|_| "--port needs a number")?,
            "--cli" => o.cli = Some(PathBuf::from(val()?)),
            "--mcpb" => o.mcpb = Some(PathBuf::from(val()?)),
            other => return Err(format!("unknown option `{other}` (mcptest [--port 47820] [--cli PATH] [--mcpb PATH])")),
        }
    }
    Ok(o)
}

/// An MCP server on stdio.
struct Mcp {
    child: Child,
    out: BufReader<ChildStdout>,
    id: u64,
}

impl Mcp {
    fn start(mut cmd: Command) -> Result<Mcp, String> {
        let mut child =
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("start the MCP server: {e}"))?;
        let out = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut m = Mcp { child, out, id: 0 };
        m.rpc(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "xtask-mcptest", "version": "1" } }),
        )?;
        Ok(m)
    }
    /// The whole JSON-RPC reply (with `result` or `error`).
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let stdin = self.child.stdin.as_mut().ok_or("no stdin")?;
        writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": self.id, "method": method, "params": params })).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        loop {
            let mut line = String::new();
            if self.out.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err(format!("{method}: the server ended"));
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["id"] == json!(self.id) {
                return Ok(v);
            }
        }
    }
    /// A tool call: Ok(text) | Err((code, message)), code 0 for a tool error.
    fn tool(&mut self, name: &str, args: Value) -> Result<String, (i64, String)> {
        let r = self.rpc("tools/call", json!({ "name": name, "arguments": args })).map_err(|e| (-1, e))?;
        if let Some(e) = r.get("error") {
            return Err((e["code"].as_i64().unwrap_or(0), e["message"].as_str().unwrap_or_default().to_string()));
        }
        let text = r["result"]["content"][0]["text"].as_str().unwrap_or("<image>").to_string();
        if r["result"]["isError"] == true { Err((0, text)) } else { Ok(text) }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        std::thread::sleep(Duration::from_millis(200));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The app's control port, as the person at the window (keys and text).
fn control(port: u16, method: &str, params: Value) -> Result<Value, String> {
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("the app at {port}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    writeln!(w, "{}", json!({ "id": 1, "method": method, "params": params })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

struct Checks(usize, usize);

impl Checks {
    fn check(&mut self, what: &str, r: Result<String, String>) {
        match r {
            Ok(m) => {
                self.0 += 1;
                let m = m.split_whitespace().collect::<Vec<_>>().join(" ");
                println!("  PASS  {what}: {m}");
            }
            Err(e) => {
                self.1 += 1;
                println!("  FAIL  {what}: {e}");
            }
        }
    }
}

/// Which listed tools have no entry in the shared table.
fn without_entry<'a>(listed: &'a [String], t: &[(&'static str, Value, Value)]) -> Vec<&'a str> {
    listed.iter().filter(|n| !t.iter().any(|(m, ..)| m == n)).map(String::as_str).collect()
}

/// The table's calls in the order to run them live: `quit_app` last (it ends the app).
fn live_order(t: Vec<(&'static str, Value, Value)>, listed: &[String]) -> Vec<(&'static str, Value, Value)> {
    let (last, mut first): (Vec<_>, Vec<_>) = t.into_iter().filter(|(n, ..)| listed.iter().any(|l| l == n)).partition(|(n, ..)| *n == "quit_app");
    first.extend(last);
    first
}

/// Every resource the app lists is read and parses as JSON; every prompt is fetched with its
/// required arguments.
fn live_resources_and_prompts(c: &mut Checks, m: &mut Mcp) {
    let list = |m: &mut Mcp, method: &str, key: &str| -> Result<Vec<Value>, String> {
        let r = m.rpc(method, json!({}))?;
        r["result"][key].as_array().cloned().ok_or(format!("{method}: no `{key}` in {r}"))
    };
    match list(m, "resources/list", "resources") {
        Err(e) => c.check("resources/list", Err(e)),
        Ok(rs) => {
            c.check("resources/list", if rs.is_empty() { Err("no resources".into()) } else { Ok(format!("{} resources", rs.len())) });
            for uri in rs.iter().filter_map(|r| r["uri"].as_str()) {
                let read = m.rpc("resources/read", json!({ "uri": uri })).and_then(|r| {
                    let text = r["result"]["contents"][0]["text"].as_str().unwrap_or_default();
                    serde_json::from_str::<Value>(text).map(|_| format!("{} bytes of JSON", text.len())).map_err(|_| format!("not JSON: {r}"))
                });
                c.check(&format!("resources/read {uri}"), read);
            }
        }
    }
    match list(m, "prompts/list", "prompts") {
        Err(e) => c.check("prompts/list", Err(e)),
        Ok(ps) => {
            c.check("prompts/list", if ps.is_empty() { Err("no prompts".into()) } else { Ok(format!("{} prompts", ps.len())) });
            for p in &ps {
                let name = p["name"].as_str().unwrap_or_default();
                let args: serde_json::Map<String, Value> = p["arguments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|a| a["required"] == true)
                    .filter_map(|a| a["name"].as_str().map(|n| (n.to_string(), json!("Sample"))))
                    .collect();
                let got = m.rpc("prompts/get", json!({ "name": name, "arguments": args })).and_then(|r| {
                    let n = r["result"]["messages"].as_array().map_or(0, Vec::len);
                    if n > 0 { Ok(format!("{n} messages")) } else { Err(format!("no messages: {r}")) }
                });
                c.check(&format!("prompts/get {name}"), got);
            }
        }
    }
}

/// The server a .mcpb's `manifest` starts, unpacked in `dir`: `server.mcp_config.command` (the win32
/// override on Windows) with `${__dirname}` = `dir`, and its args. `cargo xtask release` checks the
/// .mcpb it builds with this too.
pub(crate) fn mcpb_server(manifest: &Value, dir: &Path) -> (PathBuf, Vec<String>) {
    let cfg = &manifest["server"]["mcp_config"];
    let command =
        if cfg!(windows) { cfg["platform_overrides"]["win32"]["command"].as_str().or(cfg["command"].as_str()) } else { cfg["command"].as_str() }
            .unwrap_or_default();
    let exe = PathBuf::from(command.replace("${__dirname}", &dir.to_string_lossy()));
    let args: Vec<String> = cfg["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
    (exe, args)
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let o = parse(rest)?;
    let dir = std::env::temp_dir().join(format!("cadaddy-mcptest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut c = Checks(0, 0);
    control(o.port, "ping", json!({})).map_err(|e| format!("{e}\nstart the app first: cargo run --release -p cadaddy -- --control {}", o.port))?;
    let cli = match &o.cli {
        Some(p) => p.clone(),
        None => {
            let mut b = crate::cargo();
            b.args(["build", "-q", "--release", "-p", "cadaddy-cli"]);
            crate::run(b, "cargo build --release -p cadaddy-cli")?;
            let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
            target.join("release").join(if cfg!(windows) { "cadaddy-cli.exe" } else { "cadaddy-cli" })
        }
    };
    println!("MCP against the app on {} ({})", o.port, cli.display());
    let mut cmd = Command::new(&cli);
    cmd.args(["mcp", "--connect", &format!("127.0.0.1:{}", o.port)]);
    let mut m = Mcp::start(cmd)?;
    let listed: Vec<String> = m.rpc("tools/list", json!({}))?["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    c.check(
        "tools/list",
        if listed.iter().any(|t| t == "screenshot") {
            Ok(format!("{} tools, the app's included", listed.len()))
        } else {
            Err(format!("no app tools in {listed:?}"))
        },
    );
    let t = table::table(&dir.to_string_lossy());
    for n in without_entry(&listed, &t) {
        c.check(&format!("{n} has a table entry"), Err("listed by the app but missing from crates/mcp/tests/support/table.rs".into()));
    }
    let calls = live_order(t, &listed);
    // The agent guard first, on the drawing as it starts: the person types LINE in the app; the
    // agent's typing is refused, its execute goes through, and after Escape its typing works again.
    let typed = control(o.port, "ui.text", json!({ "text": "line" })).and_then(|_| control(o.port, "ui.key", json!({ "key": "enter" })));
    std::thread::sleep(Duration::from_millis(500));
    c.check("user types LINE", typed.map(|_| "keys sent".into()));
    c.check(
        "agent typing refused",
        match m.tool("command_line", json!({ "text": "circle 5,5 1" })) {
            Err((_, msg)) if msg.contains("the user is running") => Ok(msg.chars().take(60).collect()),
            other => Err(format!("{other:?}")),
        },
    );
    c.check(
        "agent execute works",
        m.tool("execute", json!({ "command": "circle", "params": { "center": [9, 9], "radius": 1 } }))
            .map(|_| "ok".into())
            .map_err(|e| format!("{e:?}")),
    );
    let _ = control(o.port, "ui.key", json!({ "key": "escape" }));
    std::thread::sleep(Duration::from_millis(500));
    c.check(
        "agent typing after Escape",
        m.tool("command_line", json!({ "text": "circle 5,5 1" })).map(|_| "ok".into()).map_err(|e| format!("{e:?}")),
    );
    for (name, good, bad) in calls {
        c.check(&format!("{name} normal"), m.tool(name, good).map(|t| t.chars().take(60).collect()).map_err(|e| format!("{e:?}")));
        c.check(
            &format!("{name} bad"),
            match m.tool(name, bad) {
                Err((code, msg)) if code == -32602 || code == 0 => Ok(format!("refused ({code}): {}", msg.chars().take(60).collect::<String>())),
                Ok(t) => Err(format!("accepted: {}", t.chars().take(60).collect::<String>())),
                Err(e) => Err(format!("{e:?}")),
            },
        );
    }
    live_resources_and_prompts(&mut c, &mut m);
    drop(m);
    if let Some(mcpb) = &o.mcpb {
        let un = dir.join("mcpb");
        std::fs::create_dir_all(&un).map_err(|e| e.to_string())?;
        let ok = Command::new("tar").arg("-xf").arg(mcpb).arg("-C").arg(&un).status().is_ok_and(|s| s.success());
        let manifest: Value =
            std::fs::read_to_string(un.join("manifest.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let (exe, args) = mcpb_server(&manifest, &un);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
        }
        c.check(
            "mcpb unpack",
            if ok && exe.is_file() {
                Ok(format!("{} {}", exe.display(), args.join(" ")))
            } else {
                Err(format!("unpacked {ok}, server {}", exe.display()))
            },
        );
        let mut cmd = Command::new(&exe);
        cmd.args(&args);
        c.check(
            "mcpb server",
            Mcp::start(cmd).and_then(|mut m| {
                let r = m.rpc("tools/list", json!({}))?;
                let n = r["result"]["tools"].as_array().map_or(0, Vec::len);
                if n >= 20 { Ok(format!("{n} tools (manifest {})", manifest["version"])) } else { Err(format!("{n} tools")) }
            }),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    println!("{} passed, {} failed", c.0, c.1);
    if c.1 > 0 { Err(format!("{} check(s) failed", c.1)) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_listed_tool_without_an_entry_is_found() {
        let t = table::tests::sample();
        assert!(without_entry(&names(&["screenshot", "execute"]), &t).is_empty());
        assert_eq!(without_entry(&names(&["execute", "brand_new_tool"]), &t), vec!["brand_new_tool"]);
    }

    #[test]
    fn live_order_runs_only_listed_tools_and_quits_last() {
        let order: Vec<&str> =
            live_order(table::tests::sample(), &names(&["quit_app", "screenshot", "execute", "not_in_table"])).iter().map(|(n, ..)| *n).collect();
        assert_eq!(order.last(), Some(&"quit_app"));
        assert_eq!(order.len(), 3);
        assert!(!order.contains(&"not_in_table"));
    }

    #[test]
    fn the_whole_table_is_in_the_live_loop_when_all_are_listed() {
        let t = table::tests::sample();
        let all: Vec<String> = t.iter().map(|(n, ..)| (*n).to_string()).collect();
        let n = t.len();
        assert_eq!(live_order(t, &all).len(), n);
    }
}
