//! Supply chain: what CADaddy ships is built only from what we chose.
//!
//! - `cargo xtask supplychain` (offline, a CI step): every crate in a shipped binary (the normal
//!   dependencies of the workspace's apps and libraries) has a licence `deny.toml` allows: no GPL,
//!   AGPL, LGPL-only or unknown licence; every package comes from crates.io, the workspace or an
//!   allowed git repository at a pinned revision; no banned crate is in the graph; and no secret is
//!   in the tree ([`secrets`]).
//! - `cargo xtask advisories` (a CI step, needs the network): `cargo deny check` with the same
//!   `deny.toml`, and `cargo audit`, against the RustSec advisory database.
//! - `cargo xtask sbom [--out FILE]`: a CycloneDX 1.5 SBOM of the shipped crates, to attach to a
//!   release (default `target/sbom/cadaddy-<version>.cdx.json`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// The policy in `deny.toml`: allowed licences, allowed git repositories and banned crates.
#[derive(Debug, Default, PartialEq)]
pub struct Policy {
    pub licences: Vec<String>,
    pub git: Vec<String>,
    pub banned: Vec<String>,
}

/// The body of `[section]` in a TOML file (up to the next table header).
fn section<'a>(toml: &'a str, name: &str) -> &'a str {
    let header = format!("[{name}]");
    let Some(start) = toml.lines().position(|l| l.trim() == header) else { return "" };
    let offset: usize = toml.lines().take(start + 1).map(|l| l.len() + 1).sum();
    let rest = toml.get(offset.min(toml.len())..).unwrap_or_default();
    let end = rest
        .lines()
        .position(|l| l.trim_start().starts_with('[') && l.trim_end().ends_with(']'))
        .map_or(rest.len(), |i| rest.lines().take(i).map(|l| l.len() + 1).sum());
    rest.get(..end.min(rest.len())).unwrap_or(rest)
}

/// The array of `key = [ … ]` in a section's body, without comments.
fn array(body: &str, key: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in body.lines() {
        let line = line.split('#').next().unwrap_or_default();
        let t = line.trim();
        if !inside {
            let Some(rest) = t.strip_prefix(key).map(str::trim_start).and_then(|r| r.strip_prefix('=')) else { continue };
            inside = true;
            out.push_str(rest);
        } else {
            out.push_str(t);
        }
        let depth = out.matches('[').count() as i32 - out.matches(']').count() as i32;
        if inside && depth <= 0 {
            break;
        }
    }
    out
}

/// The quoted strings in `text` (`"…"`), or those after `name =` when `field` is given.
fn strings(text: &str, field: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let at = match field {
            Some(f) => rest.find(f).and_then(|i| rest.get(i + f.len()..).and_then(|r| r.find('"').map(|j| i + f.len() + j))),
            None => rest.find('"'),
        };
        let Some(open) = at else { break };
        let Some(after) = rest.get(open + 1..) else { break };
        let Some(close) = after.find('"') else { break };
        out.push(after.get(..close).unwrap_or_default().to_string());
        rest = after.get(close + 1..).unwrap_or_default();
    }
    out
}

pub fn policy(toml: &str) -> Policy {
    Policy {
        licences: strings(&array(section(toml, "licenses"), "allow"), None),
        git: strings(&array(section(toml, "sources"), "allow-git"), None),
        banned: strings(&array(section(toml, "bans"), "deny"), Some("name")),
    }
}

/// Whether an SPDX licence expression is satisfied by the allowed licences: `A OR B` (also the
/// old `A/B`) needs one side, `A AND B` both, `A WITH exception` both the licence and the
/// exception. `None` when the expression can't be read.
pub fn licence_ok(expr: &str, allowed: &[String]) -> Option<bool> {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut pos = 0;
    let ok = or(&tokens, &mut pos, allowed)?;
    (pos == tokens.len()).then_some(ok)
}

fn or(t: &[&str], pos: &mut usize, allowed: &[String]) -> Option<bool> {
    let mut v = and(t, pos, allowed)?;
    while t.get(*pos) == Some(&"OR") {
        *pos += 1;
        let r = and(t, pos, allowed)?;
        v = v || r;
    }
    Some(v)
}

fn and(t: &[&str], pos: &mut usize, allowed: &[String]) -> Option<bool> {
    let mut v = with(t, pos, allowed)?;
    while t.get(*pos) == Some(&"AND") {
        *pos += 1;
        let r = with(t, pos, allowed)?;
        v = v && r;
    }
    Some(v)
}

fn with(t: &[&str], pos: &mut usize, allowed: &[String]) -> Option<bool> {
    let base = t.get(*pos).copied();
    let v = atom(t, pos, allowed)?;
    if t.get(*pos) == Some(&"WITH") {
        // Allowed as a whole (`Apache-2.0 WITH LLVM-exception`, as cargo-deny lists them).
        let exception = t.get(*pos + 1)?;
        *pos += 2;
        let whole = format!("{} WITH {exception}", base.unwrap_or_default());
        return Some(allowed.contains(&whole));
    }
    Some(v)
}

fn atom(t: &[&str], pos: &mut usize, allowed: &[String]) -> Option<bool> {
    let tok = *t.get(*pos)?;
    *pos += 1;
    if tok == "(" {
        let v = or(t, pos, allowed)?;
        if t.get(*pos) != Some(&")") {
            return None;
        }
        *pos += 1;
        return Some(v);
    }
    if matches!(tok, ")" | "AND" | "OR" | "WITH") {
        return None;
    }
    // `GPL-2.0+` is "or later"; the base licence decides.
    let id = tok.trim_end_matches('+');
    Some(allowed.iter().any(|a| a == id))
}

/// `cargo metadata` of the workspace (offline: the lock file's versions, nothing fetched).
fn metadata(root: &Path) -> Result<Value, String> {
    let out = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

/// The packages in a shipped binary: the workspace's members but xtask, and every normal (not
/// dev or build) dependency they reach, on any platform.
pub fn shipped(meta: &Value) -> BTreeSet<String> {
    let nodes: BTreeMap<&str, &Value> =
        meta["resolve"]["nodes"].as_array().into_iter().flatten().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    let names: BTreeMap<&str, &str> =
        meta["packages"].as_array().into_iter().flatten().filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?))).collect();
    let mut stack: Vec<String> = meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| names.get(id) != Some(&"xtask"))
        .map(str::to_string)
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = nodes.get(id.as_str()) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].is_null());
            if normal && let Some(p) = dep["pkg"].as_str() {
                stack.push(p.to_string());
            }
        }
    }
    seen
}

/// Every way the graph breaks the policy: (package, why).
pub fn check(meta: &Value, policy: &Policy) -> Vec<String> {
    let shipped = shipped(meta);
    let members: BTreeSet<&str> = meta["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut out = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let (id, name, version) =
            (p["id"].as_str().unwrap_or_default(), p["name"].as_str().unwrap_or_default(), p["version"].as_str().unwrap_or_default());
        let who = format!("{name} {version}");
        // Sources: crates.io, the workspace, or an allowed git repository at a pinned revision.
        match p["source"].as_str() {
            None => {
                if !members.contains(id) {
                    out.push(format!("{who}: a path dependency outside the workspace"));
                }
            }
            Some("registry+https://github.com/rust-lang/crates.io-index") => {}
            Some(s) if s.starts_with("git+") => {
                let url = s.trim_start_matches("git+").split(['?', '#']).next().unwrap_or_default();
                if !policy.git.iter().any(|g| g.trim_end_matches('/') == url.trim_end_matches('/')) {
                    out.push(format!("{who}: git source {url} is not allowed"));
                } else if !s.contains("rev=") {
                    out.push(format!("{who}: git source {url} is not pinned to a revision"));
                }
            }
            Some(s) => out.push(format!("{who}: source {s} is not allowed")),
        }
        if policy.banned.iter().any(|b| b == name) {
            out.push(format!("{who}: banned crate"));
        }
        if shipped.contains(id) {
            match p["license"].as_str() {
                Some(l) => match licence_ok(l, &policy.licences) {
                    Some(true) => {}
                    Some(false) => out.push(format!("{who}: licence \"{l}\" is not allowed in a shipped binary")),
                    None => out.push(format!("{who}: licence \"{l}\" can't be read")),
                },
                None => out.push(format!("{who}: no licence (unknown) in a shipped binary")),
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `cargo xtask supplychain`: licences, sources, bans and secrets, offline.
pub fn run(root: &Path) -> Result<(), String> {
    let toml = std::fs::read_to_string(root.join("deny.toml")).map_err(|e| format!("deny.toml: {e}"))?;
    let policy = policy(&toml);
    if policy.licences.is_empty() || policy.git.is_empty() {
        return Err("deny.toml: no [licenses] allow or [sources] allow-git list read".into());
    }
    let meta = metadata(root)?;
    let shipped = shipped(&meta).len();
    let mut problems = check(&meta, &policy);
    let found = crate::secrets::scan(root)?;
    problems.extend(found.iter().map(|f| format!("secret: {f}")));
    if problems.is_empty() {
        let all = meta["packages"].as_array().map_or(0, Vec::len);
        println!("supplychain: {all} packages from allowed sources, {shipped} shipped with allowed licences, none banned, no secrets");
        Ok(())
    } else {
        Err(format!("supplychain: {} problem(s):\n  {}", problems.len(), problems.join("\n  ")))
    }
}

/// `cargo xtask advisories`: RustSec advisories through cargo-deny (with `deny.toml`) and cargo-audit.
pub fn advisories(root: &Path) -> Result<(), String> {
    for (tool, args, install) in [
        ("cargo-deny", vec!["deny", "--locked", "check"], "cargo install --locked cargo-deny"),
        ("cargo-audit", vec!["audit", "--deny", "warnings"], "cargo install --locked cargo-audit"),
    ] {
        let status = Command::new(env!("CARGO")).current_dir(root).args(&args).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) if s.code() == Some(101) && which(tool).is_none() => return Err(format!("{tool} isn't installed: {install}")),
            Ok(s) => return Err(format!("cargo {}: {s}", args.join(" "))),
            Err(e) => return Err(format!("{tool}: {e} ({install})")),
        }
    }
    Ok(())
}

fn which(tool: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(format!("{tool}{}", std::env::consts::EXE_SUFFIX))).find(|p| p.is_file())
}

/// The checksums in `Cargo.lock`, by (name, version).
fn checksums(lock: &str) -> BTreeMap<(String, String), String> {
    let mut out = BTreeMap::new();
    for block in lock.split("[[package]]") {
        let field = |k: &str| block.lines().find_map(|l| l.strip_prefix(&format!("{k} = \"")).and_then(|r| r.strip_suffix('"')).map(str::to_string));
        if let (Some(n), Some(v), Some(c)) = (field("name"), field("version"), field("checksum")) {
            out.insert((n, v), c);
        }
    }
    out
}

fn purl(p: &Value) -> String {
    let (name, version) = (p["name"].as_str().unwrap_or_default(), p["version"].as_str().unwrap_or_default());
    match p["source"].as_str() {
        Some(s) if s.starts_with("git+") => {
            let url = s.trim_start_matches("git+").split(['?', '#']).next().unwrap_or_default();
            let rev = s.rsplit('#').next().unwrap_or_default();
            format!("pkg:cargo/{name}@{version}?vcs_url=git%2B{}%40{rev}", url.replace(':', "%3A"))
        }
        _ => format!("pkg:cargo/{name}@{version}"),
    }
}

/// The SBOM of the shipped crates, CycloneDX 1.5 JSON.
pub fn sbom_of(meta: &Value, lock: &str, timestamp: &str) -> Value {
    let shipped = shipped(meta);
    let sums = checksums(lock);
    let packages: Vec<&Value> =
        meta["packages"].as_array().into_iter().flatten().filter(|p| p["id"].as_str().is_some_and(|id| shipped.contains(id))).collect();
    let refs: BTreeMap<&str, String> = packages.iter().filter_map(|p| Some((p["id"].as_str()?, purl(p)))).collect();
    let version = packages.iter().find(|p| p["name"] == "cadaddy").and_then(|p| p["version"].as_str()).unwrap_or("0.0.0");
    let components: Vec<Value> = packages
        .iter()
        .filter(|p| p["name"] != "cadaddy")
        .map(|p| {
            let key = (p["name"].as_str().unwrap_or_default().to_string(), p["version"].as_str().unwrap_or_default().to_string());
            let mut c = json!({
                "type": "library",
                "bom-ref": purl(p),
                "name": p["name"],
                "version": p["version"],
                "purl": purl(p),
            });
            if let Some(l) = p["license"].as_str() {
                c["licenses"] = json!([{ "expression": l }]);
            }
            if let Some(sum) = sums.get(&key) {
                c["hashes"] = json!([{ "alg": "SHA-256", "content": sum }]);
            }
            if let Some(repo) = p["repository"].as_str() {
                c["externalReferences"] = json!([{ "type": "vcs", "url": repo }]);
            }
            c
        })
        .collect();
    let nodes: BTreeMap<&str, &Value> =
        meta["resolve"]["nodes"].as_array().into_iter().flatten().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    let dependencies: Vec<Value> = refs
        .iter()
        .map(|(id, r)| {
            let on: Vec<&String> = nodes
                .get(id)
                .map(|n| n["deps"].as_array().into_iter().flatten().filter_map(|d| refs.get(d["pkg"].as_str()?)).collect())
                .unwrap_or_default();
            json!({ "ref": r, "dependsOn": on })
        })
        .collect();
    // A serial number fixed by the content: the same lock file, the same SBOM.
    let digest = |seed: u64| lock.bytes().fold(seed, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let (a, b) = (digest(0xcbf2_9ce4_8422_2325), digest(0x8422_2325_cbf2_9ce4));
    let serial = format!("urn:uuid:{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, b >> 52, b & 0xffff_ffff_ffff);
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": serial,
        "version": 1,
        "metadata": {
            "timestamp": timestamp,
            "tools": { "components": [{ "type": "application", "name": "cargo xtask sbom" }] },
            "component": { "type": "application", "bom-ref": format!("pkg:cargo/cadaddy@{version}"), "name": "CADaddy", "version": version,
                "purl": format!("pkg:cargo/cadaddy@{version}"), "licenses": [{ "expression": "FSL-1.1-ALv2" }] },
        },
        "components": components,
        "dependencies": dependencies,
    })
}

/// `cargo xtask sbom [--out FILE]`.
pub fn sbom(root: &Path, rest: &[&str]) -> Result<(), String> {
    let out = match rest {
        [] => None,
        ["--out", p] => Some(std::path::PathBuf::from(p)),
        _ => return Err("usage: cargo xtask sbom [--out FILE]".into()),
    };
    let meta = metadata(root)?;
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let now = Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let bom = sbom_of(&meta, &lock, now.as_deref().unwrap_or("1970-01-01T00:00:00Z"));
    let version = bom["metadata"]["component"]["version"].as_str().unwrap_or("0.0.0").to_string();
    let path = out.unwrap_or_else(|| root.join(format!("target/sbom/cadaddy-{version}.cdx.json")));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&bom).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("sbom: {} components → {}", bom["components"].as_array().map_or(0, Vec::len), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn licence_expressions() {
        let a = allow(&["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause"]);
        for ok in [
            "MIT",
            "MIT OR Apache-2.0",
            "MIT/Apache-2.0",
            "MIT / Apache-2.0",
            "Apache-2.0 OR GPL-2.0-only",
            "(MIT OR Apache-2.0) AND BSD-3-Clause",
            "Apache-2.0 WITH LLVM-exception OR MIT",
        ] {
            assert_eq!(licence_ok(ok, &a), Some(true), "{ok}");
        }
        for bad in [
            "GPL-3.0-only",
            "AGPL-3.0-or-later",
            "MIT AND GPL-2.0+",
            "LGPL-2.1-or-later",
            "Apache-2.0 WITH Classpath-exception-2.0",
            "(MIT OR Apache-2.0) AND OFL-1.1",
        ] {
            assert_eq!(licence_ok(bad, &a), Some(false), "{bad}");
        }
        for unreadable in ["", "MIT OR", "(MIT", "MIT)", "AND MIT"] {
            assert_eq!(licence_ok(unreadable, &a), None, "{unreadable:?}");
        }
    }

    #[test]
    fn the_policy_is_read_from_deny_toml() {
        let p = policy(include_str!("../../deny.toml"));
        assert!(p.licences.contains(&"MIT".to_string()) && p.licences.contains(&"FSL-1.1-ALv2".to_string()), "{p:?}");
        assert!(!p.licences.iter().any(|l| l.contains("GPL")), "no GPL family licence is allowed: {p:?}");
        assert_eq!(p.git, ["https://github.com/storytold/craft-libs"]);
        assert!(p.banned.contains(&"openssl-sys".to_string()), "{p:?}");
    }

    /// A made-up graph: the app, a permissive library, a GPL one it uses, a dev-only GPL tool, a
    /// git dependency, an unpinned one, an unknown registry and a banned crate.
    fn graph() -> Value {
        let pkg = |name: &str, lic: Option<&str>, src: Option<&str>| json!({ "id": format!("{name}-id"), "name": name, "version": "1.0.0", "license": lic, "source": src, "repository": null });
        let io = Some("registry+https://github.com/rust-lang/crates.io-index");
        let dep = |name: &str, kind: Option<&str>| json!({ "pkg": format!("{name}-id"), "dep_kinds": [{ "kind": kind }] });
        json!({
            "workspace_members": ["cadaddy-id"],
            "packages": [
                pkg("cadaddy", Some("FSL-1.1-ALv2"), None),
                pkg("fine", Some("MIT OR Apache-2.0"), io),
                pkg("copyleft", Some("GPL-3.0-only"), io),
                pkg("testtool", Some("GPL-3.0-only"), io),
                pkg("pinned", Some("MIT"), Some("git+https://github.com/storytold/craft-libs?rev=abc#abc")),
                pkg("loose", Some("MIT"), Some("git+https://github.com/storytold/craft-libs#abc")),
                pkg("elsewhere", Some("MIT"), Some("registry+https://example.com/index")),
                pkg("openssl-sys", Some("MIT"), io),
                pkg("nolicence", None, io),
            ],
            "resolve": { "nodes": [
                { "id": "cadaddy-id", "deps": [dep("fine", None), dep("copyleft", None), dep("testtool", Some("dev")), dep("pinned", None), dep("loose", None), dep("nolicence", None)] },
            ] },
        })
    }

    #[test]
    fn the_graph_check_finds_each_problem() {
        let p = policy(include_str!("../../deny.toml"));
        let found = check(&graph(), &p);
        let has = |s: &str| found.iter().any(|f| f.contains(s));
        assert!(has("copyleft 1.0.0: licence \"GPL-3.0-only\""), "{found:#?}");
        assert!(!has("testtool"), "a dev-only dependency isn't shipped: {found:#?}");
        assert!(!has("fine 1.0.0") && !has("pinned 1.0.0"), "{found:#?}");
        assert!(has("loose 1.0.0: git source https://github.com/storytold/craft-libs is not pinned"), "{found:#?}");
        assert!(has("elsewhere 1.0.0: source registry+https://example.com/index"), "{found:#?}");
        assert!(has("openssl-sys 1.0.0: banned crate"), "{found:#?}");
        assert!(has("nolicence 1.0.0: no licence"), "{found:#?}");
    }

    #[test]
    fn the_sbom_lists_the_shipped_crates_with_hashes() {
        let lock = "[[package]]\nname = \"fine\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\nchecksum = \"ab12\"\n";
        let bom = sbom_of(&graph(), lock, "2026-10-19T00:00:00Z");
        assert_eq!(bom["bomFormat"], "CycloneDX");
        assert_eq!(bom["specVersion"], "1.5");
        let names: Vec<&str> = bom["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"fine") && names.contains(&"copyleft") && !names.contains(&"testtool"), "{names:?}");
        let fine = bom["components"].as_array().unwrap().iter().find(|c| c["name"] == "fine").unwrap();
        assert_eq!(fine["purl"], "pkg:cargo/fine@1.0.0");
        assert_eq!(fine["hashes"][0]["content"], "ab12");
        assert_eq!(fine["licenses"][0]["expression"], "MIT OR Apache-2.0");
        assert_eq!(sbom_of(&graph(), lock, "2026-10-19T00:00:00Z")["serialNumber"], bom["serialNumber"], "the same lock, the same serial");
        let pinned = bom["components"].as_array().unwrap().iter().find(|c| c["name"] == "pinned").unwrap();
        assert!(pinned["purl"].as_str().unwrap().contains("vcs_url=git%2Bhttps%3A//github.com/storytold/craft-libs%40abc"), "{pinned}");
    }
}
