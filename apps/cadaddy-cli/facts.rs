//! The facts the website may quote about CADaddy, worked out from the code (and, where the code
//! can't say, from the repository's own files), with nothing typed in by hand: the commands, the
//! file formats it opens and saves (proved by writing a made-up drawing in each and reading it
//! back), the DWG and DXF versions, the MCP tools, the interface languages, the installers, the
//! AutoCAD menu coverage and the features present. Used by `cargo xtask compare-facts` (which
//! writes `docs/compare-facts.json`) and by the test that checks `docs/website-claims.json`
//! against it. No network, no clock: the same code gives the same facts.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};

/// A small made-up drawing with something of each common kind.
fn sample() -> cadaddy_engine::doc::Drawing {
    let mut s = cadaddy_engine::Session::new();
    let cmds = [
        ("line", json!({ "points": [[0, 0], [100, 0], [100, 60]] })),
        ("circle", json!({ "center": [50, 30], "radius": 10 })),
        ("arc", json!({ "center": [20, 20], "radius": 8, "start": 0, "end": 90 })),
        ("rectang", json!({ "p1": [120, 0], "p2": [160, 40] })),
        ("text", json!({ "at": [10, 70], "height": 5, "text": "SAMPLE" })),
        ("dimlinear", json!({ "p1": [0, 0], "p2": [100, 0], "at": [50, -15] })),
    ];
    for (c, p) in cmds {
        let _ = s.execute(c, &p);
    }
    // A box as a mesh: what IFC and the 3D formats write.
    let corners = [[0, 0, 0], [10, 0, 0], [10, 10, 0], [0, 10, 0], [0, 0, 5], [10, 0, 5], [10, 10, 5], [0, 10, 5]];
    let vertices: Vec<Value> = corners.iter().map(|[x, y, z]| json!({ "x": x, "y": y, "z": z })).collect();
    let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
    let faces: Vec<Value> = quads.iter().map(|f| json!({ "v": f })).collect();
    let mesh: Result<cadaddy_engine::doc::Mesh, _> = serde_json::from_value(json!({ "vertices": vertices, "faces": faces }));
    if let Ok(m) = mesh {
        let _ = s.add_entity(cadaddy_engine::doc::EntityKind::Mesh(m));
    }
    s.doc().cloned().unwrap_or_default()
}

/// Formats written and read back from the same bytes (each proved here, not listed).
fn formats(d: &cadaddy_engine::doc::Drawing) -> (BTreeSet<String>, BTreeSet<String>) {
    let candidates = ["cdy", "dwg", "dxf", "pdf", "svg", "png", "glb", "gltf", "obj", "stl", "ifc", "3mf", "eps", "wmf"];
    let (mut save, mut open) = (BTreeSet::new(), BTreeSet::new());
    for ext in candidates {
        let name = format!("facts.{ext}");
        let Ok(bytes) = cadaddy_io::write(d, &name) else { continue };
        if bytes.is_empty() {
            continue;
        }
        save.insert(ext.to_string());
        let back = if ext == "pdf" {
            cadaddy_io::pdf_read::read(&bytes, &cadaddy_io::pdf_read::PdfOptions::default()).map(|_| ()).map_err(|e| e.to_string())
        } else {
            cadaddy_io::read(&bytes, &name).map(|_| ()).map_err(|e| e.to_string())
        };
        if back.is_ok() {
            open.insert(ext.to_string());
        }
    }
    (save, open)
}

/// DWG and DXF versions written and read back.
fn versions(d: &cadaddy_engine::doc::Drawing) -> Value {
    use cadaddy_io::{SaveOptions, Version};
    let all = [Version::R12, Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018];
    let mut out = Map::new();
    for (ext, binary) in [("dwg", false), ("dxf", false), ("dxfBinary", true)] {
        let file = if ext == "dwg" { "facts.dwg" } else { "facts.dxf" };
        let mut ok = Vec::new();
        for v in all {
            let opts = SaveOptions { version: Some(v), binary };
            let Ok(bytes) = cadaddy_io::write_with(d, file, &opts) else { continue };
            if cadaddy_io::read(&bytes, file).is_ok_and(|back| back.entity_count() > 0) {
                ok.push(format!("{v:?}"));
            }
        }
        out.insert(ext.into(), json!(ok));
    }
    Value::Object(out)
}

/// The MCP tools, as a client lists them.
fn mcp_tools() -> Vec<String> {
    let mut s = cadaddy_mcp::Server::new(Box::new(cadaddy_mcp::Headless::default()));
    let call = |s: &mut cadaddy_mcp::Server, m: &str| -> Value {
        let line = json!({ "jsonrpc": "2.0", "id": 1, "method": m, "params": { "protocolVersion": cadaddy_mcp::PROTOCOL_VERSION } }).to_string();
        s.handle_line(&line).and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null)
    };
    call(&mut s, "initialize");
    let r = call(&mut s, "tools/list");
    let mut names: Vec<String> =
        r["result"]["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    names.sort();
    names
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_default()
}

/// The menu coverage line of the generated parity checklist: "Menu breadth: 298 / 491 items".
fn parity(root: &Path) -> Value {
    let text = read(root, "docs/parity-checklist.md");
    let line = text.lines().find(|l| l.contains("Menu breadth:")).unwrap_or_default();
    let nums: Vec<u64> = line.split(|c: char| !c.is_ascii_digit()).filter_map(|t| t.parse().ok()).collect();
    match nums.as_slice() {
        [covered, items, _pct, registered, ..] => json!({ "menuCovered": covered, "menuItems": items, "registeredCommands": registered }),
        _ => Value::Null,
    }
}

/// The interface languages: English and each UI catalog.
fn languages(root: &Path) -> Vec<String> {
    let mut out = vec!["en".to_string()];
    if let Ok(dir) = std::fs::read_dir(root.join("crates/ui-egui/src/i18n")) {
        let mut codes: Vec<String> =
            dir.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".tsv")).map(str::to_string)).collect();
        codes.sort();
        out.extend(codes);
    }
    out
}

/// Installers the packaging scripts make, and the release file names the updater looks for.
fn installers(root: &Path) -> Value {
    let has = |rel: &str, needle: &str| read(root, rel).contains(needle);
    let mut kinds = Vec::new();
    if has("packaging/windows/package.ps1", ".msi") {
        kinds.push("windows-msi");
    }
    if has("packaging/macos/package.sh", ".dmg") {
        kinds.push("macos-dmg");
    }
    if has("packaging/linux/package.sh", ".AppImage") {
        kinds.push("linux-appimage");
    }
    if has("packaging/linux/package.sh", ".deb") {
        kinds.push("linux-deb");
    }
    let updater = read(root, "apps/cadaddy/src/updater.rs");
    let mut assets: Vec<&str> =
        ["CADaddy-Windows.msi", "CADaddy-Mac.dmg", "CADaddy-Linux.AppImage"].into_iter().filter(|a| updater.contains(&format!("\"{a}\""))).collect();
    assets.sort_unstable();
    let plist = read(root, "packaging/macos/Info.plist.in");
    let mac_min = plist
        .split("<key>LSMinimumSystemVersion</key>")
        .nth(1)
        .and_then(|r| r.split("<string>").nth(1))
        .and_then(|r| r.split("</string>").next())
        .unwrap_or_default()
        .trim()
        .to_string();
    json!({ "kinds": kinds, "releaseAssets": assets, "macosMinimum": mac_min })
}

/// Commands present for the features the site names (each true only when its command exists).
fn features(ids: &BTreeSet<String>) -> Value {
    let groups: [(&str, &[&str]); 9] = [
        ("solids3d", &["box", "extrude", "revolve", "union", "subtract", "intersect"]),
        ("rendering", &["render", "materials"]),
        ("walk", &["3dwalk"]),
        ("ifcExport", &["export"]),
        ("pdfToDwg", &["pdfimport"]),
        ("pdfMarkups", &["markupimport"]),
        ("sheetSetPublish", &["sheetset"]),
        ("takeoff", &["count", "area", "counttable"]),
        ("imageTrace", &["imagetrace"]),
    ];
    let mut m = Map::new();
    for (name, need) in groups {
        let missing: Vec<&&str> = need.iter().filter(|c| !ids.contains(**c)).collect();
        m.insert(name.into(), json!(missing.is_empty()));
    }
    Value::Object(m)
}

/// Every fact, keyed for `docs/compare-facts.json`.
pub fn compute(root: &Path) -> Value {
    let specs = cadaddy_engine::cmd::command_specs();
    let ids: BTreeSet<String> = specs.iter().map(|c| c.id.to_string()).collect();
    let menus = specs.iter().filter(|c| !c.menu.is_empty()).count();
    let d = sample();
    let (save, open) = formats(&d);
    let tools = mcp_tools();
    let langs = languages(root);
    let license = read(root, "LICENSE.md");
    let fsl = ["FSL-1.1-ALv2", "FSL-1.1"].into_iter().find(|l| license.contains(l)).unwrap_or_default();
    let keys = read(root, "apps/cadaddy/src/release_keys.txt");
    let signed = !keys.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) && root.join("apps/cadaddy/src/relsig.rs").exists();
    // Names typed at the command line (API forms such as `arc.csa` have a dot).
    let typed = ids.iter().filter(|i| !i.contains('.')).count();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commands": { "count": ids.len(), "typed": typed, "inMenus": menus, "callableAsJson": ids.len(), "ids": ids },
        "formats": { "save": save, "open": open },
        "versions": versions(&d),
        "mcp": { "count": tools.len(), "tools": tools },
        "languages": { "count": langs.len(), "codes": langs },
        "installers": installers(root),
        "parity": parity(root),
        "features": features(&ids),
        "license": fsl,
        "signedUpdates": signed,
    })
}

/// A claim's check against the facts: `{"fact": "a.b", "atLeast" | "equals" | "includes": …}`.
/// `Err` says what doesn't hold.
pub fn check(facts: &Value, check: &Value) -> Result<(), String> {
    let path = check["fact"].as_str().ok_or("a check names a `fact`")?;
    let v = path.split('.').try_fold(facts, |v, k| v.get(k)).ok_or_else(|| format!("no fact `{path}`"))?;
    if let Some(n) = check.get("atLeast") {
        let (have, want) = (v.as_f64().unwrap_or(f64::NAN), n.as_f64().unwrap_or(f64::INFINITY));
        // A missing or non-numeric fact fails too (NaN compares as nothing).
        if have.partial_cmp(&want).is_none_or(std::cmp::Ordering::is_lt) {
            return Err(format!("`{path}` is {v}, the claim needs at least {n}"));
        }
    }
    if let Some(e) = check.get("equals")
        && v != e
    {
        return Err(format!("`{path}` is {v}, the claim says {e}"));
    }
    if let Some(list) = check.get("includes").and_then(Value::as_array) {
        let have: Vec<&Value> = v.as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let missing: Vec<&Value> = list.iter().filter(|x| !have.contains(x)).collect();
        if !missing.is_empty() {
            return Err(format!("`{path}` lacks {}", Value::Array(missing.into_iter().cloned().collect())));
        }
    }
    Ok(())
}

/// The facts as lines for llms.txt (a block to paste under "Facts").
pub fn llms_lines(f: &Value) -> String {
    let list = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let mut out = String::new();
    out.push_str(&format!("- Version: {}\n", f["version"].as_str().unwrap_or_default()));
    out.push_str(&format!(
        "- Commands: {} typed at the command line ({} including API forms), each callable as JSON\n",
        f["commands"]["typed"], f["commands"]["count"]
    ));
    out.push_str(&format!("- Opens: {}\n", list(&f["formats"]["open"])));
    out.push_str(&format!("- Saves: {}\n", list(&f["formats"]["save"])));
    out.push_str(&format!("- DWG versions written and read back: {}\n", list(&f["versions"]["dwg"])));
    out.push_str(&format!("- DXF versions written and read back: {}\n", list(&f["versions"]["dxf"])));
    out.push_str(&format!("- MCP tools ({}): {}\n", f["mcp"]["count"], list(&f["mcp"]["tools"])));
    out.push_str(&format!("- Interface languages: {}\n", f["languages"]["count"]));
    out.push_str(&format!("- AutoCAD menu coverage: {} of {} items\n", f["parity"]["menuCovered"], f["parity"]["menuItems"]));
    out
}
