//! The shared project page (Q350): `share.export` writes the model files of a read-only page
//! anyone with the link can open in a browser (`cargo xtask share-page` adds the web viewer).
//!
//! The model is the drawing's building model (as `fieldview.export` makes it) or, for a drawing
//! read from an IFC file, its IFC objects: storeys as levels, spaces as rooms, every other object
//! with its class, trade, type and property sets. It is stripped by default
//! ([`cadaddy_walk3d::share::strip`]) of the names the drawing knows (its properties, title-block
//! attributes, the project in the building spec, the file's name, an IFC file's people,
//! organisations and addresses) and of anything that looks like an address, a phone number or a
//! company; the strip report goes beside the bundle, never in it.
//!
//! Written into `dir`: `model/share.json` (the manifest), `model/base.json`, `model/part-NNN.json`,
//! `thumbs/<level>.svg`, and with `allow_download` a model to download (the stripped package, or
//! with `strip: false` the source file itself).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_walk3d::field::{FieldElement, FieldLevel, FieldPackage, FieldRoom, FieldSpec, MAX_ELEMENTS, MAX_FIELDS, trade_info};
use cadaddy_walk3d::share::{self, DEFAULT_CHUNK, MANIFEST, MODEL_DIR, Scrubber, StripReport};
use serde_json::{Value, json};

use super::ifcmodel;
use super::*;
use crate::{Result, Session};

const CMD: &str = "share.export";
const FT_PER_M: f64 = 1.0 / 0.3048;
/// Largest source file scanned for names or copied for download.
const MAX_SOURCE_BYTES: u64 = 1 << 30;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Share Project Page", run_export)
            .params(
                "{dir: the page's folder, title?: page title (default \"Shared Model\"), strip?: bool (default true: names, addresses, people, firms, \
                 title blocks and stamps removed), allow_download?: bool (default false), chunk?: elements per part (default 1000), \
                 source?: the model file (scanned for names; copied for download when strip is off), report?: strip report path \
                 (default beside the folder: <dir>.strip-report.txt)} → {dir, files, bytes, elements, levels, rooms, parts, stripped, \
                 removed, report?, download?}",
            )
            .noundo(),
    ]
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// `IfcWallStandardCase` → `wallstandardcase`.
fn bare_class(c: &str) -> String {
    let c = c.trim().to_ascii_lowercase();
    c.strip_prefix("ifc").map(str::to_string).unwrap_or(c)
}

/// The trade an IFC class belongs to.
fn ifc_trade(class: &str, systems: &[String]) -> &'static str {
    let sys = systems.join(" ").to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| class.contains(w));
    if has(&["sprinkler", "firesuppression", "alarm"]) || sys.contains("sprinkler") || sys.contains("fire") {
        "fire"
    } else if has(&["communicationsappliance", "audiovisual"]) {
        "lowvolt"
    } else if has(&["duct", "airterminal", "fan", "unitaryequipment", "airtoair", "coil", "damper", "boiler", "chiller", "coolingtower"]) {
        "mech"
    } else if has(&["pipe", "sanitary", "valve", "pump", "tank", "wasteterminal", "flowterminal"]) {
        "plumb"
    } else if has(&["cable", "light", "outlet", "switching", "electric", "protectivedevice", "junctionbox", "lamp", "transformer"]) {
        "elec"
    } else if has(&["column", "beam", "footing", "member", "plate", "pile", "reinforc", "tendon", "slab"]) {
        "struct"
    } else if has(&["wall", "door", "window", "stair", "roof", "covering", "railing", "curtainwall", "ramp", "furnish", "furniture"]) {
        "arch"
    } else {
        "other"
    }
}

/// The package of a drawing read from an IFC file (metres).
fn ifc_package(d: &Drawing, project: &str) -> FieldPackage {
    let els = ifcmodel::elements(d);
    let mut pkg = FieldPackage::new(project);
    let mut storeys: Vec<(f64, String)> = Vec::new();
    for e in &els {
        if let Some(l) = &e.tag.level
            && !storeys.iter().any(|(_, n)| n == l)
            && storeys.len() < cadaddy_walk3d::field::MAX_LEVELS
        {
            storeys.push((e.tag.elevation.unwrap_or(0.0), l.clone()));
        }
    }
    storeys.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (i, (z, name)) in storeys.iter().enumerate() {
        let top = els.iter().filter(|e| e.tag.level.as_deref() == Some(name)).map(|e| e.max.z).fold(f64::NEG_INFINITY, f64::max);
        let next = storeys.get(i + 1).map(|(n, _)| *n);
        let h = next.map_or(if top.is_finite() { top - z } else { 3.6 }, |n| n - z);
        pkg.levels.push(FieldLevel { name: name.clone(), elevation_ft: r3(z * FT_PER_M), height_ft: r3((h * FT_PER_M).clamp(7.0, 100.0)) });
    }
    let ft = |v: cadaddy_geom::Vec3| [r3(v.x * FT_PER_M), r3(v.y * FT_PER_M), r3(v.z * FT_PER_M)];
    for e in els.iter().take(MAX_ELEMENTS) {
        let class = bare_class(&e.tag.class);
        let level = e.tag.level.clone().unwrap_or_default();
        let (min, max) = (ft(e.min), ft(e.max));
        if class == "space" {
            let long = e.tag.props.iter().find(|(k, _)| k.ends_with(".LongName")).map(|(_, v)| v.clone());
            pkg.rooms.push(FieldRoom {
                number: e.tag.name.clone(),
                name: long.or_else(|| e.tag.ty.clone()).unwrap_or_default(),
                level,
                outline: vec![[min[0], min[1]], [max[0], min[1]], [max[0], max[1]], [min[0], max[1]]],
                at: [r3((min[0] + max[0]) * 0.5), r3((min[1] + max[1]) * 0.5)],
                unit: String::new(),
            });
            continue;
        }
        if matches!(class.as_str(), "openingelement" | "annotation" | "grid" | "virtualelement") {
            continue;
        }
        let status = e.tag.props.iter().find(|(k, _)| k.rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case("Status"))).map(|(_, v)| v.clone());
        let mut props: Vec<FieldSpec> = e
            .tag
            .systems
            .iter()
            .map(|s| FieldSpec { key: "System".into(), value: s.clone(), source: "IFC system".into(), confidence: String::new() })
            .collect();
        for (k, v) in e.tag.props.iter().take(MAX_FIELDS) {
            let (set, name) = k.split_once('.').unwrap_or(("", k.as_str()));
            props.push(FieldSpec { key: name.to_string(), value: v.clone(), source: set.to_string(), confidence: String::new() });
        }
        props.truncate(MAX_FIELDS);
        let shown = e.tag.class.trim_start_matches("Ifc").trim_start_matches("IFC");
        pkg.elements.push(FieldElement {
            id: if e.tag.guid.is_empty() { e.handle.hex() } else { e.tag.guid.clone() },
            trade: ifc_trade(&class, &e.tag.systems).into(),
            kind: if e.tag.name.is_empty() { shown.to_string() } else { format!("{shown} {}", e.tag.name) },
            level,
            room: e.tag.space.clone().unwrap_or_default(),
            min,
            max,
            path: Vec::new(),
            size_in: None,
            mark: e.tag.ty.clone().unwrap_or_default(),
            source: format!("IFC {}", e.tag.class),
            status: status.unwrap_or_default(),
            props,
        });
    }
    let counts = pkg.trade_counts();
    pkg.trades = ["arch", "struct", "mech", "plumb", "elec", "fire", "lowvolt", "other"]
        .iter()
        .filter(|k| counts.contains_key(**k))
        .map(|k| trade_info(k))
        .collect();
    pkg
}

/// The open issues naming an element go in its status (`RFI-004 open`).
fn issue_status(pkg: &mut FieldPackage) {
    let mut by_el: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in pkg.issues.iter().filter(|i| i.is_open()) {
        for id in &i.elements {
            by_el.entry(id.clone()).or_default().push(format!("{} {}", i.id, i.status));
        }
    }
    for e in &mut pkg.elements {
        if let Some(list) = by_el.get(&e.id) {
            let extra = list.iter().take(4).cloned().collect::<Vec<_>>().join("; ");
            e.status = if e.status.is_empty() { extra } else { format!("{}; {extra}", e.status) };
        }
    }
}

/// The quoted strings of an IFC file's people, organisations, addresses and project, site and
/// building entities, and its header's author and organisation.
fn ifc_names(text: &str) -> Vec<String> {
    const WHO: [&str; 9] = [
        "IFCPERSON(",
        "IFCORGANIZATION(",
        "IFCPOSTALADDRESS(",
        "IFCTELECOMADDRESS(",
        "IFCPROJECT(",
        "IFCSITE(",
        "IFCBUILDING(",
        "IFCACTOR(",
        "FILE_NAME(",
    ];
    let mut out = Vec::new();
    for line in text.lines().take(5_000_000) {
        let up = line.trim_start().to_ascii_uppercase();
        let Some(at) = WHO.iter().filter_map(|w| up.find(w).map(|a| a + w.len())).min() else { continue };
        let body = line.trim_start().get(at..).unwrap_or_default();
        let mut parts = body.split('\'');
        // Odd pieces are inside quotes.
        parts.next();
        for (i, p) in parts.enumerate() {
            if i % 2 == 0 && !p.is_empty() && out.len() < 10_000 {
                // A GlobalId is 22 characters of [0-9A-Za-z_$]; not a name.
                let guid = p.len() == 22 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
                if !guid {
                    out.push(p.to_string());
                }
            }
        }
    }
    out
}

/// The JSON strings under private keys of a value (the building spec's project block).
fn private_values(v: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > 4 || out.len() > 10_000 {
        return;
    }
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter().take(10_000) {
                match x {
                    Value::String(s) if share::private_key(k) => out.push(s.clone()),
                    Value::Object(_) if share::private_key(k) => all_strings(x, 0, out),
                    _ => private_values(x, depth + 1, out),
                }
            }
        }
        Value::Array(a) => a.iter().take(10_000).for_each(|x| private_values(x, depth + 1, out)),
        _ => {}
    }
}

fn all_strings(v: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > 4 || out.len() > 10_000 {
        return;
    }
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Object(m) => m.values().take(1_000).for_each(|x| all_strings(x, depth + 1, out)),
        Value::Array(a) => a.iter().take(1_000).for_each(|x| all_strings(x, depth + 1, out)),
        _ => {}
    }
}

/// The names the drawing knows: its properties, title-block attributes, the building spec's
/// project, IFC site and building names, the source file's name and an IFC file's people.
fn known_names(d: &Drawing, source: Option<&Path>, source_text: Option<&str>) -> Vec<String> {
    let p = &d.properties;
    let mut out: Vec<String> =
        [&p.title, &p.subject, &p.author, &p.keywords, &p.comments, &p.last_saved_by].into_iter().filter(|s| !s.trim().is_empty()).cloned().collect();
    out.extend(p.custom.iter().map(|(_, v)| v.clone()));
    // Keywords and comments are lists, often.
    for s in [&p.keywords, &p.comments] {
        out.extend(s.split([',', ';', '\n']).map(str::trim).filter(|w| !w.is_empty()).map(str::to_string));
    }
    // Title blocks: the attributes on sheets, and paper-space texts.
    for l in &d.layouts {
        for e in l.entities.iter().take(200_000) {
            match &e.kind {
                EntityKind::Insert(i) => out.extend(i.attribs.iter().take(200).map(|a| a.text.value.clone())),
                EntityKind::Text(t) => out.push(t.value.clone()),
                _ => {}
            }
        }
    }
    if let Some(v) = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
        private_values(&v, 0, &mut out);
    }
    for e in ifcmodel::elements(d).iter().take(MAX_ELEMENTS) {
        out.extend(e.tag.site.clone());
        out.extend(e.tag.building.clone());
    }
    if let Some(src) = source {
        out.extend(src.file_stem().map(|s| s.to_string_lossy().into_owned()));
        out.extend(src.file_name().map(|s| s.to_string_lossy().into_owned()));
    }
    if let Some(t) = source_text {
        out.extend(ifc_names(t));
    }
    out
}

/// Names that are the model's own words (level names, room numbers, sheet-like codes) are not
/// scrubbed: they are what the page shows.
fn ordinary(pkg: &FieldPackage, name: &str) -> bool {
    let n = name.trim();
    pkg.levels.iter().any(|l| l.name.eq_ignore_ascii_case(n))
        || pkg.rooms.iter().any(|r| r.number.eq_ignore_ascii_case(n))
        || pkg.trades.iter().any(|t| t.label.eq_ignore_ascii_case(n))
        // A sheet number (`A-101`, `M2.01`).
        || (n.len() <= 8 && n.chars().any(|c| c.is_ascii_digit()) && n.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::writefile::write(path, bytes).map_err(|e| bad(CMD, format!("{}: {e}", path.display())))
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = str_param(p, "dir").map(str::trim).filter(|d| !d.is_empty()).ok_or_else(|| bad(CMD, "dir required: the page's folder"))?;
    let dir = PathBuf::from(dir);
    let title: String = str_param(p, "title").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Shared Model").chars().take(120).collect();
    let strip = bool_or(p, "strip", true);
    let allow_download = bool_or(p, "allow_download", false);
    let chunk = p.get("chunk").and_then(Value::as_u64).map_or(DEFAULT_CHUNK, |c| usize::try_from(c).unwrap_or(DEFAULT_CHUNK));
    let source = str_param(p, "source").map(str::trim).filter(|t| !t.is_empty()).map(PathBuf::from);
    let report_path = match str_param(p, "report").map(str::trim).filter(|t| !t.is_empty()) {
        Some(r) => PathBuf::from(r),
        None => {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "share".into());
            dir.with_file_name(format!("{name}.strip-report.txt"))
        }
    };
    if strip && report_path.starts_with(&dir) {
        return Err(bad(CMD, "the strip report can't go in the page's folder (it lists the names removed)"));
    }

    // The model.
    let is_ifc = !ifcmodel::elements(s.doc()?).is_empty();
    let mut pkg = if is_ifc {
        ifc_package(s.doc()?, &title)
    } else {
        let r = s.execute("fieldview.export", &json!({ "inline": true, "project": title }))?;
        let text = serde_json::to_vec(&r["package"]).map_err(|e| bad(CMD, e.to_string()))?;
        FieldPackage::from_json(&text).map_err(|e| bad(CMD, e))?
    };
    if pkg.elements.is_empty() && pkg.rooms.is_empty() {
        return Err(bad(CMD, "nothing to share: open an IFC model, or build the model first (model.build)"));
    }
    pkg.assign_rooms();
    issue_status(&mut pkg);

    // Stripped of names.
    let source_meta = source.as_ref().and_then(|s| std::fs::metadata(s).ok()).filter(|m| m.is_file() && m.len() <= MAX_SOURCE_BYTES);
    let source_text = match (&source, &source_meta) {
        (Some(src), Some(_)) if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("ifc")) => {
            std::fs::read(src).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
        }
        _ => None,
    };
    let mut report = StripReport::default();
    if strip {
        let known: Vec<String> =
            known_names(s.doc()?, source.as_deref(), source_text.as_deref()).into_iter().filter(|n| !ordinary(&pkg, n)).collect();
        report = share::strip(&mut pkg, &title, &Scrubber::new(known));
    } else {
        pkg.project = title.clone();
        pkg.notes.clear();
    }

    // The files.
    crate::writefile::check(&dir.join(MODEL_DIR).join(MANIFEST)).map_err(|e| bad(CMD, e.to_string()))?;
    let download = if allow_download {
        Some(match (&source, &source_meta, strip) {
            (Some(src), Some(_), false) => {
                let ext: String = src.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "bin".into());
                let ext: String = ext.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
                let bytes = std::fs::read(src).map_err(|e| bad(CMD, format!("{}: {e}", src.display())))?;
                (format!("model.{ext}"), bytes)
            }
            // Stripped, the download is the stripped model (the source still has the names).
            _ => ("model.field.json".to_string(), pkg.to_json().map_err(|e| bad(CMD, e))?.into_bytes()),
        })
    } else {
        None
    };
    let (manifest, files) =
        share::write_bundle(&dir, &pkg, &title, chunk, strip, download.as_ref().map(|(n, b)| (n.as_str(), b.as_slice()))).map_err(|e| bad(CMD, e))?;

    // The strip report, beside the folder (never in it: it lists what was removed).
    let mut out = json!({
        "dir": dir.display().to_string(),
        "files": files,
        "bytes": manifest.total_bytes(),
        "elements": pkg.elements.len(),
        "levels": pkg.levels.len(),
        "rooms": pkg.rooms.len(),
        "parts": manifest.parts.len(),
        "stripped": strip,
        "removed": report.removed.len(),
        "source": if is_ifc { "ifc" } else { "building model" },
    });
    if strip {
        write(&report_path, report.text(&title).as_bytes())?;
        out["report"] = json!(report_path.display().to_string());
    }
    if let Some(d) = &manifest.download {
        out["download"] = json!(d.file);
    }
    Ok(out)
}
