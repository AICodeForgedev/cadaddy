//! CADaddy talks to the network only where PRIVACY.md says: the update check (and the download it
//! offers), the MCP server reaching the app it was given, the web build fetching from its own site,
//! and the control channel listening on this computer only. Every other network call in the code is
//! a failure here, and so is an HTTP client crate in Cargo.lock: a new one must be added to
//! PRIVACY.md and to [`ALLOWED`] on purpose.

use std::path::{Path, PathBuf};

/// Calls that reach the network, as they are written.
const CALLS: &[&str] = &[
    "TcpStream::connect",
    "UdpSocket::bind",
    "Command::new(\"curl\")",
    "Command::new(\"wget\")",
    "fetch_with_str",
    "fetch_with_request",
    "TcpListener::bind",
    "reqwest::",
    "ureq::",
    "hyper::",
    "isahc::",
    "attohttpc::",
    "surf::",
];

/// (file, call): where each is allowed, and why (PRIVACY.md lists them).
const ALLOWED: &[(&str, &str)] = &[
    // The update check and the download it offers: api.github.com and github.com releases only.
    ("apps/cadaddy/src/updater.rs", "Command::new(\"curl\")"),
    // `cadaddy-cli mcp` reaching the running app at the address it was given (127.0.0.1 by default).
    ("crates/mcp/src/backend.rs", "TcpStream::connect"),
    // The control channel and agent port: listening on 127.0.0.1 only (checked below).
    ("apps/cadaddy/src/control_server.rs", "TcpListener::bind"),
    // The web build fetching a walkthrough or drawing a link names (`?walk=`), and its self-test reporting to
    // the page that served it.
    ("apps/cadaddy-web/src/upload.rs", "fetch_with_str"),
    ("apps/cadaddy-web/src/selftest.rs", "fetch_with_str"),
];

/// HTTP and TLS client crates: none is linked; the updater uses the system's curl.
const CLIENT_CRATES: &[&str] = &["reqwest", "ureq", "hyper", "isahc", "surf", "attohttpc", "curl", "curl-sys", "native-tls", "openssl", "rustls"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if p.is_dir() {
            // Tests and build output aren't shipped.
            if !matches!(name.as_str(), "tests" | "target" | "examples" | "benches") {
                sources(&p, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(p);
        }
    }
}

/// A file's shipped code: up to its first `#[cfg(test)]`.
fn shipped(text: &str) -> &str {
    text.find("#[cfg(test)]").map_or(text, |i| text.get(..i).unwrap_or(text))
}

#[test]
fn the_code_reaches_the_network_only_where_privacy_md_says() {
    let root = root();
    let mut files = Vec::new();
    for top in ["crates", "apps"] {
        sources(&root.join(top), &mut files);
    }
    assert!(files.len() > 100, "the sources weren't found: {}", files.len());
    let mut found = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        for (n, line) in shipped(&text).lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for call in CALLS.iter().filter(|c| code.contains(**c)) {
                if !ALLOWED.contains(&(rel.as_str(), *call)) {
                    found.push(format!("{rel}:{}: {call}", n + 1));
                }
                // Listening only on this computer.
                if *call == "TcpListener::bind" && !(code.contains("127.0.0.1") || code.contains("LOCALHOST")) {
                    found.push(format!("{rel}:{}: listens beyond 127.0.0.1", n + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "network calls not in PRIVACY.md:\n  {}", found.join("\n  "));
    let privacy = std::fs::read_to_string(root.join("PRIVACY.md")).expect("PRIVACY.md");
    for (file, _) in ALLOWED {
        assert!(privacy.contains(file), "PRIVACY.md doesn't name {file}");
    }
}

#[test]
fn no_http_client_crate_is_linked() {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).unwrap();
    let linked: Vec<&str> = CLIENT_CRATES.iter().filter(|c| lock.contains(&format!("name = \"{c}\"\n"))).copied().collect();
    assert!(linked.is_empty(), "HTTP/TLS client crates in Cargo.lock (say why in PRIVACY.md and here): {linked:?}");
}

#[test]
fn the_guard_catches_a_new_call() {
    // The scan itself: a made-up file with a call in its shipped code and one in its tests.
    let text = "fn get() { let s = std::net::TcpStream::connect(\"example.com:80\"); }\n#[cfg(test)]\nmod t { fn f() { std::net::TcpStream::connect(\"x\"); } }\n";
    let hits: Vec<&str> = shipped(text).lines().filter(|l| CALLS.iter().any(|c| l.contains(c))).collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
}
