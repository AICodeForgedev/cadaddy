//! BIM coordination viewer commands: model tree, trade visibility, section box, viewpoints,
//! coordination views, colour schemes, isolate, 3D measure and BCF export.
//!
//! The coordination palette (UI) drives these commands to give the user a Navisworks-style
//! experience for reviewing MEP and structural coordination in 3D.
//!
//! Coordination-specific data (viewpoints, section box, colour schemes, coordination views) is
//! stored as extra JSON keys inside the drawing's `building_spec` text. The `BuildingSpec`
//! deserializer ignores unknown keys, so they survive round-trips through the spec parser.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::MepKind;
use serde_json::{Value, json};

use super::bspec::current;
use super::*;
use crate::{EngineError, Result, Session};

fn kind_str(k: MepKind) -> &'static str {
    match k {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("coordination.tree", "Coordination Tree", run_tree)
            .params("{level?} → {levels: [{name, elevation_ft, rooms, systems, elements}]}")
            .noundo(),
        CommandSpec::new("coordination.trades", "Trade Visibility", run_trades)
            .params("{set?: [{system, visible?}], level?} → {trades: [{system, kind, layer, visible, count}]}")
            .noundo(),
        CommandSpec::new("coordination.sectionbox", "Section Box", run_sectionbox).params("{on?, min?, max?, fit?} → {on, min, max}").noundo(),
        CommandSpec::new("coordination.viewpoints", "Coordination Viewpoints", run_viewpoints)
            .params("{save?: {name}, restore?, delete?} → {viewpoints}")
            .noundo(),
        CommandSpec::new("coordination.measure3d", "3D Measure", run_measure3d)
            .params("{from: [x,y,z], to: [x,y,z]} → {distance, dx, dy, dz}")
            .noundo(),
        CommandSpec::new("coordination.clashlist", "Clash List", run_clashlist).params("{tolerance_in?, group_by?} → {groups, total}").noundo(),
        CommandSpec::new("coordination.clashstatus", "Clash Status", run_clashstatus).params("{id, status}").noundo(),
        CommandSpec::new("coordination.bcf", "BCF Export", run_bcf).params("{path, tolerance_in?} → {topics, path}").noundo(),
        CommandSpec::new("coordination.mode", "Coordination Mode", run_mode).params("{on?, ghost_opacity?} → {on, ghost_opacity}").noundo(),
        CommandSpec::new("coordination.isolate", "Isolate Trade", run_isolate).params("{system?, level?, reset?} → {isolated, trades}").noundo(),
        CommandSpec::new("coordination.colors", "Colour Scheme", run_colors)
            .params("{save?: {name, colors}, load?, delete?, list?} → {schemes | active}")
            .noundo(),
        CommandSpec::new("coordination.views", "Coordination Views", run_views)
            .params("{save?: {name}, restore?, delete?, list?} → {views}")
            .noundo(),
    ]
}

// ─── Coordination data in building spec ───────────────────────────────────────

/// Read a coordination-specific key from the building spec JSON.
fn coord_get(d: &Drawing, key: &str) -> Value {
    d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.get(key).cloned()).unwrap_or(Value::Null)
}

/// Write a coordination-specific key into the building spec JSON.
fn coord_set(s: &mut Session, key: &str, val: &Value) -> Result<()> {
    let d = s.doc()?;
    let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
    if let Some(obj) = spec.as_object_mut() {
        obj.insert(key.into(), val.clone());
    }
    s.doc_mut()?.building_spec = Some(spec.to_string());
    Ok(())
}

fn r6(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ─── Model tree ───────────────────────────────────────────────────────────────

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.tree")?;
    let d = s.doc()?;
    let filter_level = p.get("level").and_then(Value::as_str);

    let mut levels = Vec::new();
    for level in sp.levels.iter().take(500) {
        if let Some(f) = filter_level
            && !level.name.eq_ignore_ascii_case(f)
        {
            continue;
        }
        let rooms: Vec<Value> = sp
            .rooms
            .iter()
            .filter(|r| r.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&level.name)))
            .take(1000)
            .map(|r| json!({ "name": r.name, "ceiling_ft": r.ceiling_ft.unwrap_or(level.ceiling_ft) }))
            .collect();

        let mut sys_counts: std::collections::BTreeMap<String, (String, usize)> = std::collections::BTreeMap::new();
        for run in &sp.mep.runs {
            if run.level.eq_ignore_ascii_case(&level.name) {
                let entry = sys_counts.entry(run.system.clone()).or_insert_with(|| {
                    let kind = sp.mep.systems.iter().find(|sy| sy.name == run.system).map_or("unknown", |sy| kind_str(sy.kind));
                    (kind.to_string(), 0)
                });
                entry.1 += 1;
            }
        }
        let systems: Vec<Value> = sys_counts.iter().map(|(name, (kind, count))| json!({ "name": name, "kind": kind, "count": count })).collect();

        let element_count = d
            .model
            .iter()
            .filter(|e| d.is_visible(e))
            .filter(|e| super::bspec::made_level(e).as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&level.name)))
            .take(50_000)
            .count();

        levels.push(json!({
            "name": level.name,
            "elevation_ft": level.elevation_ft,
            "height_ft": level.height_ft,
            "ceiling_ft": level.ceiling_ft,
            "rooms": rooms,
            "systems": systems,
            "elements": element_count,
        }));
    }
    Ok(json!({ "levels": levels }))
}

// ─── Trade visibility ─────────────────────────────────────────────────────────

fn run_trades(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.trades")?;
    let level_filter = p.get("level").and_then(Value::as_str);

    if let Some(sets) = p.get("set").and_then(Value::as_array) {
        let d = s.doc_mut()?;
        for item in sets.iter().take(100) {
            let Some(system) = item.get("system").and_then(Value::as_str) else {
                continue;
            };
            if let Some(vis) = item.get("visible").and_then(Value::as_bool) {
                let layer_name = format!("3D-MEP-{system}");
                if let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
                    layer.on = vis;
                }
            }
        }
    }

    let d = s.doc()?;
    let mut trades = Vec::new();
    for sys in &sp.mep.systems {
        let layer_name = format!("3D-MEP-{}", sys.name);
        let visible = d.layers.iter().find(|l| l.name.eq_ignore_ascii_case(&layer_name)).is_none_or(|l| l.on);
        let count = sp.mep.runs.iter().filter(|r| r.system == sys.name && level_filter.is_none_or(|lf| r.level.eq_ignore_ascii_case(lf))).count();
        // Collect levels that have runs for this system.
        let mut levels: Vec<String> = Vec::new();
        for run in &sp.mep.runs {
            if run.system == sys.name && !levels.iter().any(|l| l.eq_ignore_ascii_case(&run.level)) {
                levels.push(run.level.clone());
                if levels.len() >= 100 {
                    break;
                }
            }
        }
        trades.push(json!({
            "system": sys.name,
            "kind": kind_str(sys.kind),
            "layer": layer_name,
            "visible": visible,
            "count": count,
            "levels": levels,
        }));
    }
    Ok(json!({ "trades": trades }))
}

// ─── Section box ──────────────────────────────────────────────────────────────

fn run_sectionbox(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let sbox = coord_get(d, "coord_sectionbox");
    let mut on = sbox.get("on").and_then(Value::as_bool).unwrap_or(false);
    let mut lo = arr3(&sbox, "min").unwrap_or([0.0; 3]);
    let mut hi = arr3(&sbox, "max").unwrap_or([0.0; 3]);

    if let Some(v) = p.get("on").and_then(Value::as_bool) {
        on = v;
    }
    if let Some(a) = arr3(p, "min") {
        lo = a;
    }
    if let Some(a) = arr3(p, "max") {
        hi = a;
    }
    if p.get("fit").and_then(Value::as_bool).unwrap_or(false) {
        let (blo, bhi) = model_bounds(d);
        let pad = 10.0;
        lo = [blo.x - pad, blo.y - pad, blo.z - pad];
        hi = [bhi.x + pad, bhi.y + pad, bhi.z + pad];
        on = true;
    }

    let state = json!({ "on": on, "min": lo, "max": hi });
    coord_set(s, "coord_sectionbox", &state)?;
    Ok(state)
}

fn arr3(v: &Value, key: &str) -> Option<[f64; 3]> {
    let a = v.get(key).and_then(Value::as_array).filter(|a| a.len() == 3)?;
    Some([
        a.first().and_then(Value::as_f64).filter(|f| f.is_finite())?,
        a.get(1).and_then(Value::as_f64).filter(|f| f.is_finite())?,
        a.get(2).and_then(Value::as_f64).filter(|f| f.is_finite())?,
    ])
}

fn model_bounds(d: &Drawing) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut hi = Vec3::new(f64::MIN, f64::MIN, f64::MIN);
    for e in d.model.iter().take(500_000) {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        lo.x = lo.x.min(b.min.x);
        lo.y = lo.y.min(b.min.y);
        hi.x = hi.x.max(b.max.x);
        hi.y = hi.y.max(b.max.y);
        if let EntityKind::Mesh(m) = &e.kind {
            for v in &m.vertices {
                lo.z = lo.z.min(v.z);
                hi.z = hi.z.max(v.z);
            }
        }
    }
    if !lo.x.is_finite() {
        lo = Vec3::ZERO;
        hi = Vec3::new(100.0, 100.0, 20.0);
    }
    (lo, hi)
}

// ─── Viewpoints ───────────────────────────────────────────────────────────────

fn run_viewpoints(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_viewpoints");
    let mut vps: Vec<Value> = stored.as_array().cloned().unwrap_or_default();

    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Unnamed");
        let st = s.state()?;
        let vf = st.zoom_frame();
        let vdir = st.view_dir;
        let entry = json!({
            "name": name,
            "center": [vf.center.x, vf.center.y],
            "height": vf.height,
            "view_dir": [vdir.x, vdir.y, vdir.z],
            "visual_style": st.visual_style.name(),
        });
        if let Some(pos) = vps.iter().position(|v| v.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = vps.get_mut(pos) {
                *slot = entry;
            }
        } else if vps.len() < 200 {
            vps.push(entry);
        }
    }

    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        vps.retain(|v| v.get("name").and_then(Value::as_str) != Some(del));
    }

    if let Some(restore) = p.get("restore").and_then(Value::as_str)
        && let Some(vp) = vps.iter().find(|v| v.get("name").and_then(Value::as_str) == Some(restore))
    {
        let center = vp
            .get("center")
            .and_then(Value::as_array)
            .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
        let height = vp.get("height").and_then(Value::as_f64);
        if let (Some(c), Some(h)) = (center, height) {
            s.execute("zoom", &json!({ "mode": "center", "center": c, "height": h }))?;
        }
        // Restore view direction if saved.
        if let Some(vd) = arr3(vp, "view_dir") {
            let _ = s.execute("vpoint", &json!({ "dir": vd }));
        }
        // Restore visual style if saved.
        if let Some(vs) = vp.get("visual_style").and_then(Value::as_str) {
            let _ = s.execute("vscurrent", &json!({ "style": vs }));
        }
    }

    coord_set(s, "coord_viewpoints", &json!(vps))?;
    Ok(json!({ "viewpoints": vps }))
}

// ─── 3D measure ───────────────────────────────────────────────────────────────

fn run_measure3d(_s: &mut Session, p: &Value) -> Result<Value> {
    let from = arr3(p, "from").ok_or_else(|| EngineError::Other("from must be [x,y,z]".into()))?;
    let to = arr3(p, "to").ok_or_else(|| EngineError::Other("to must be [x,y,z]".into()))?;
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let dz = to[2] - from[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let angle_xy = dy.atan2(dx).to_degrees();
    let horiz = (dx * dx + dy * dy).sqrt();
    let angle_z = dz.atan2(horiz).to_degrees();
    Ok(json!({
        "distance": r6(dist), "dx": r6(dx), "dy": r6(dy), "dz": r6(dz),
        "angle_xy": r6(angle_xy), "angle_z": r6(angle_z),
    }))
}

// ─── Clash list ───────────────────────────────────────────────────────────────

fn run_clashlist(s: &mut Session, p: &Value) -> Result<Value> {
    let raw = s.execute("model.clashes", &json!({ "tolerance_in": p.get("tolerance_in") }))?;
    let clashes = raw.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default();
    let group_by = p.get("group_by").and_then(Value::as_str).unwrap_or("severity");

    let items: Vec<Value> = clashes
        .into_iter()
        .enumerate()
        .map(|(i, mut c)| {
            if let Some(obj) = c.as_object_mut() {
                obj.insert("id".to_string(), json!(format!("CL-{:04}", i + 1)));
                obj.entry("status".to_string()).or_insert(json!("new"));
            }
            c
        })
        .collect();

    let mut groups: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
    for item in &items {
        let key = match group_by {
            "trade" => item.get("systems").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("Unknown").to_string(),
            "level" => "All".into(),
            _ => item.get("kind").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        };
        groups.entry(key).or_default().push(item.clone());
    }
    let grouped: Vec<Value> = groups.into_iter().map(|(key, clashes)| json!({ "key": key, "clashes": clashes })).collect();
    let total = items.len();
    Ok(json!({ "groups": grouped, "total": total }))
}

fn run_clashstatus(_s: &mut Session, p: &Value) -> Result<Value> {
    let _id = p.get("id").and_then(Value::as_str).ok_or_else(|| EngineError::Other("id required".into()))?;
    let status = p.get("status").and_then(Value::as_str).ok_or_else(|| EngineError::Other("status required".into()))?;
    if !["new", "active", "resolved", "approved"].contains(&status) {
        return Err(EngineError::Other("status must be new|active|resolved|approved".into()));
    }
    Ok(json!({ "ok": true }))
}

// ─── BCF export ───────────────────────────────────────────────────────────────

fn run_bcf(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("path required".into()))?;
    let raw = s.execute("model.clashes", &json!({ "tolerance_in": p.get("tolerance_in") }))?;
    let clashes = raw.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default();

    let io = |e: std::io::Error| EngineError::Other(format!("BCF write: {e}"));
    let mut buf: Vec<u8> = Vec::new();
    {
        use std::io::Write;
        let mut zip = zip_mini::ZipWriter::new(&mut buf);

        zip.start_file("bcf.version").map_err(io)?;
        write!(
            zip,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <Version VersionId=\"2.1\"><DetailedVersion>2.1</DetailedVersion></Version>"
        )
        .map_err(io)?;

        let project_id = uuid_v4();
        zip.start_file("project.bcfp").map_err(io)?;
        write!(
            zip,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <ProjectExtension><Project ProjectId=\"{project_id}\">\
            <Name>CADaddy Coordination</Name></Project></ProjectExtension>"
        )
        .map_err(io)?;

        for (i, clash) in clashes.iter().enumerate().take(1000) {
            let guid = uuid_v4();
            let kind = clash.get("kind").and_then(Value::as_str).unwrap_or("unknown");
            let msg = escape_xml(clash.get("message").and_then(Value::as_str).unwrap_or(""));
            let title = escape_xml(&format!("CL-{:04}: {kind}", i + 1));
            let date = iso_date();

            zip.start_file(&format!("{guid}/markup.bcf")).map_err(io)?;
            write!(
                zip,
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <Markup><Topic Guid=\"{guid}\" TopicType=\"Clash\" TopicStatus=\"New\">\
                <Title>{title}</Title><Description>{msg}</Description>\
                <CreationDate>{date}</CreationDate><CreationAuthor>CADaddy</CreationAuthor>\
                </Topic></Markup>"
            )
            .map_err(io)?;
        }
        zip.finish().map_err(io)?;
    }

    std::fs::write(path, &buf).map_err(|e| EngineError::Other(format!("BCF write: {e}")))?;
    Ok(json!({ "topics": clashes.len().min(1000), "path": path }))
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn iso_date() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = now / 86400;
    let secs = now % 86400;
    let (mut y, mut m, mut day) = (1970u64, 1u64, 1u64);
    let mut rem = days;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if rem < yd {
            break;
        }
        rem -= yd;
        y += 1;
    }
    let md = [31, if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for d in md {
        if rem < d {
            day = rem + 1;
            break;
        }
        rem -= d;
        m += 1;
    }
    format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn uuid_v4() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |t| t.as_nanos()).hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let a = h.finish();
    h.write_u64(a.wrapping_mul(6364136223846793005));
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        (a >> 48) & 0xfff,
        8 | ((b >> 60) & 3),
        (b >> 48) & 0xfff,
        b & 0xffffffffffff
    )
}

/// Minimal ZIP writer (store method, no compression).
mod zip_mini {
    use std::io::{self, Write};

    pub struct ZipWriter<'a> {
        buf: &'a mut Vec<u8>,
        entries: Vec<Entry>,
        data_start: usize,
    }

    struct Entry {
        name: String,
        offset: u32,
        size: u32,
        crc: u32,
    }

    impl<'a> ZipWriter<'a> {
        pub fn new(buf: &'a mut Vec<u8>) -> Self {
            ZipWriter { buf, entries: Vec::new(), data_start: 0 }
        }

        pub fn start_file(&mut self, name: &str) -> io::Result<()> {
            self.flush_pending();
            let offset = self.buf.len() as u32;
            let nb = name.as_bytes();
            self.buf.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            self.buf.extend_from_slice(&20u16.to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(&[0; 4]);
            self.buf.extend_from_slice(&[0; 4]); // crc placeholder
            self.buf.extend_from_slice(&[0; 4]); // size placeholder
            self.buf.extend_from_slice(&[0; 4]); // size placeholder
            self.buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            self.buf.extend_from_slice(nb);
            self.data_start = self.buf.len();
            self.entries.push(Entry { name: name.to_string(), offset, size: 0, crc: 0 });
            Ok(())
        }

        fn flush_pending(&mut self) {
            if let Some(entry) = self.entries.last_mut()
                && entry.size == 0
                && self.data_start > 0
            {
                let data = &self.buf[self.data_start..];
                let size = data.len() as u32;
                let crc = crc32(data);
                entry.size = size;
                entry.crc = crc;
                let off = entry.offset as usize;
                self.buf[off + 14..off + 18].copy_from_slice(&crc.to_le_bytes());
                self.buf[off + 18..off + 22].copy_from_slice(&size.to_le_bytes());
                self.buf[off + 22..off + 26].copy_from_slice(&size.to_le_bytes());
            }
        }

        pub fn finish(&mut self) -> io::Result<()> {
            self.flush_pending();
            let cd_offset = self.buf.len() as u32;
            let mut cd_size = 0u32;
            for e in &self.entries {
                let nb = e.name.as_bytes();
                let start = self.buf.len();
                self.buf.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
                self.buf.extend_from_slice(&20u16.to_le_bytes());
                self.buf.extend_from_slice(&20u16.to_le_bytes());
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 4]);
                self.buf.extend_from_slice(&e.crc.to_le_bytes());
                self.buf.extend_from_slice(&e.size.to_le_bytes());
                self.buf.extend_from_slice(&e.size.to_le_bytes());
                self.buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 2]);
                self.buf.extend_from_slice(&[0; 4]);
                self.buf.extend_from_slice(&e.offset.to_le_bytes());
                self.buf.extend_from_slice(nb);
                cd_size += (self.buf.len() - start) as u32;
            }
            self.buf.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
            self.buf.extend_from_slice(&[0; 4]);
            self.buf.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&cd_size.to_le_bytes());
            self.buf.extend_from_slice(&cd_offset.to_le_bytes());
            self.buf.extend_from_slice(&[0; 2]);
            Ok(())
        }
    }

    impl Write for ZipWriter<'_> {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
            }
        }
        !crc
    }
}

// ─── Coordination mode ───────────────────────────────────────────────────────

/// Enable or disable coordination visual mode. When on, architecture/structure layers are shown
/// ghosted (transparent) and MEP trade layers are solid in their system colours.
fn run_mode(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_mode");
    let mut on = stored.get("on").and_then(Value::as_bool).unwrap_or(false);
    let mut ghost_opacity = stored.get("ghost_opacity").and_then(Value::as_f64).unwrap_or(0.2);

    if let Some(v) = p.get("on").and_then(Value::as_bool) {
        on = v;
    }
    if let Some(v) = p.get("ghost_opacity").and_then(Value::as_f64) {
        ghost_opacity = v.clamp(0.0, 1.0);
    }

    // Apply ghosting: set transparency on non-MEP layers when on, restore when off.
    if on {
        let opacity_pct = ((1.0 - ghost_opacity) * 90.0).round().clamp(0.0, 90.0) as u8;
        let d = s.doc_mut()?;
        for layer in &mut d.layers {
            if !layer.name.starts_with("3D-MEP-") {
                layer.transparency = opacity_pct;
            } else {
                layer.transparency = 0;
            }
        }
    } else {
        let d = s.doc_mut()?;
        for layer in &mut d.layers {
            layer.transparency = 0;
        }
    }

    let state = json!({ "on": on, "ghost_opacity": ghost_opacity });
    coord_set(s, "coord_mode", &state)?;
    Ok(state)
}

// ─── Isolate trade ───────────────────────────────────────────────────────────

/// Hide all trades except the specified system (and optionally level), or reset to show all.
fn run_isolate(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "coordination.isolate")?;

    if p.get("reset").and_then(Value::as_bool).unwrap_or(false) {
        // Show all trade layers.
        let d = s.doc_mut()?;
        for sys in &sp.mep.systems {
            let layer_name = format!("3D-MEP-{}", sys.name);
            if let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
                layer.on = true;
            }
        }
        return Ok(json!({ "isolated": null, "trades": "all visible" }));
    }

    let system = p.get("system").and_then(Value::as_str);
    let level = p.get("level").and_then(Value::as_str);

    let d = s.doc_mut()?;
    for sys in &sp.mep.systems {
        let layer_name = format!("3D-MEP-{}", sys.name);
        let keep = system.is_none_or(|target| sys.name.eq_ignore_ascii_case(target));
        if let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
            layer.on = keep;
        }
    }

    let isolated = json!({ "system": system, "level": level });
    Ok(json!({ "isolated": isolated }))
}

// ─── Colour schemes ──────────────────────────────────────────────────────────

/// Manage named colour schemes for coordination views. Each scheme maps system names to RGB
/// colours. Stored as `coord_color_schemes` in the building spec.
fn run_colors(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_color_schemes");
    let mut schemes: Vec<Value> = stored.as_array().cloned().unwrap_or_default();

    // List all schemes.
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        let names: Vec<&str> = schemes.iter().filter_map(|s| s.get("name").and_then(Value::as_str)).collect();
        return Ok(json!({ "schemes": names }));
    }

    // Save a scheme.
    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Default");
        let colors = save.get("colors").cloned().unwrap_or(json!({}));
        let entry = json!({ "name": name, "colors": colors });
        if let Some(pos) = schemes.iter().position(|s| s.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = schemes.get_mut(pos) {
                *slot = entry;
            }
        } else if schemes.len() < 50 {
            schemes.push(entry);
        }
        coord_set(s, "coord_color_schemes", &json!(schemes))?;
        return Ok(json!({ "saved": name }));
    }

    // Load (apply) a scheme: set system colours on their layers.
    if let Some(load_name) = p.get("load").and_then(Value::as_str) {
        let scheme = schemes.iter().find(|s| s.get("name").and_then(Value::as_str) == Some(load_name));
        let Some(scheme) = scheme else {
            return Err(EngineError::Other(format!("colour scheme '{load_name}' not found")));
        };
        let colors = scheme.get("colors").cloned().unwrap_or(json!({}));
        let d = s.doc_mut()?;
        if let Some(obj) = colors.as_object() {
            for (system, color_val) in obj.iter().take(100) {
                let layer_name = format!("3D-MEP-{system}");
                if let Some(arr) = color_val.as_array()
                    && arr.len() == 3
                {
                    let r = arr.first().and_then(Value::as_u64).unwrap_or(255) as u8;
                    let g = arr.get(1).and_then(Value::as_u64).unwrap_or(255) as u8;
                    let b = arr.get(2).and_then(Value::as_u64).unwrap_or(255) as u8;
                    if let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&layer_name)) {
                        layer.color = cadaddy_color::Color::True(cadaddy_color::Rgb(r, g, b));
                    }
                }
            }
        }
        coord_set(s, "coord_active_scheme", &json!(load_name))?;
        return Ok(json!({ "active": load_name, "colors": colors }));
    }

    // Delete a scheme.
    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        schemes.retain(|s| s.get("name").and_then(Value::as_str) != Some(del));
        coord_set(s, "coord_color_schemes", &json!(schemes))?;
        return Ok(json!({ "deleted": del }));
    }

    // Default: return current state.
    let active = coord_get(s.doc()?, "coord_active_scheme");
    Ok(json!({ "schemes": schemes, "active": active }))
}

// ─── Coordination views (full state) ─────────────────────────────────────────

/// A coordination view saves the full viewing state: camera, section box, trade visibility,
/// colour scheme and visual style. Stored as `coord_views` in the building spec.
fn run_views(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let stored = coord_get(d, "coord_views");
    let mut views: Vec<Value> = stored.as_array().cloned().unwrap_or_default();

    // Save: capture full state.
    if let Some(save) = p.get("save") {
        let name = save.get("name").and_then(Value::as_str).unwrap_or("Unnamed");

        // Camera state.
        let st = s.state()?;
        let vf = st.zoom_frame();
        let vdir = st.view_dir;
        let vstyle = st.visual_style.name();

        // Section box state.
        let d = s.doc()?;
        let sbox = coord_get(d, "coord_sectionbox");
        let mode = coord_get(d, "coord_mode");
        let active_scheme = coord_get(d, "coord_active_scheme");

        // Trade visibility snapshot.
        let trade_vis: Vec<Value> =
            d.layers.iter().filter(|l| l.name.starts_with("3D-MEP-")).take(200).map(|l| json!({ "layer": l.name, "on": l.on })).collect();

        let entry = json!({
            "name": name,
            "center": [vf.center.x, vf.center.y],
            "height": vf.height,
            "view_dir": [vdir.x, vdir.y, vdir.z],
            "visual_style": vstyle,
            "sectionbox": sbox,
            "coord_mode": mode,
            "color_scheme": active_scheme,
            "trades": trade_vis,
        });

        if let Some(pos) = views.iter().position(|v| v.get("name").and_then(Value::as_str) == Some(name)) {
            if let Some(slot) = views.get_mut(pos) {
                *slot = entry;
            }
        } else if views.len() < 200 {
            views.push(entry);
        }
        coord_set(s, "coord_views", &json!(views))?;
        return Ok(json!({ "saved": name, "views": views }));
    }

    // Delete.
    if let Some(del) = p.get("delete").and_then(Value::as_str) {
        views.retain(|v| v.get("name").and_then(Value::as_str) != Some(del));
        coord_set(s, "coord_views", &json!(views))?;
        return Ok(json!({ "views": views }));
    }

    // Restore: apply full state.
    if let Some(restore_name) = p.get("restore").and_then(Value::as_str) {
        let view = views.iter().find(|v| v.get("name").and_then(Value::as_str) == Some(restore_name)).cloned();
        let Some(view) = view else {
            return Err(EngineError::Other(format!("coordination view '{restore_name}' not found")));
        };

        // Camera.
        let center = view
            .get("center")
            .and_then(Value::as_array)
            .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)]);
        let height = view.get("height").and_then(Value::as_f64);
        if let (Some(c), Some(h)) = (center, height) {
            let _ = s.execute("zoom", &json!({ "mode": "center", "center": c, "height": h }));
        }
        if let Some(vd) = arr3(&view, "view_dir") {
            let _ = s.execute("vpoint", &json!({ "dir": vd }));
        }
        if let Some(vs) = view.get("visual_style").and_then(Value::as_str) {
            let _ = s.execute("vscurrent", &json!({ "style": vs }));
        }

        // Section box.
        if let Some(sbox) = view.get("sectionbox") {
            coord_set(s, "coord_sectionbox", sbox)?;
        }

        // Coordination mode.
        if let Some(mode) = view.get("coord_mode") {
            coord_set(s, "coord_mode", mode)?;
            let on = mode.get("on").and_then(Value::as_bool).unwrap_or(false);
            if on {
                let ghost_opacity = mode.get("ghost_opacity").and_then(Value::as_f64).unwrap_or(0.2);
                let _ = s.execute("coordination.mode", &json!({ "on": true, "ghost_opacity": ghost_opacity }));
            }
        }

        // Colour scheme.
        if let Some(scheme_name) = view.get("color_scheme").and_then(Value::as_str) {
            let _ = s.execute("coordination.colors", &json!({ "load": scheme_name }));
        }

        // Trade visibility.
        if let Some(trades) = view.get("trades").and_then(Value::as_array) {
            let d = s.doc_mut()?;
            for tv in trades.iter().take(200) {
                let lname = tv.get("layer").and_then(Value::as_str);
                let on = tv.get("on").and_then(Value::as_bool);
                if let (Some(lname), Some(on)) = (lname, on)
                    && let Some(layer) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(lname))
                {
                    layer.on = on;
                }
            }
        }

        coord_set(s, "coord_views", &json!(views))?;
        return Ok(json!({ "restored": restore_name }));
    }

    // List.
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        let names: Vec<&str> = views.iter().filter_map(|v| v.get("name").and_then(Value::as_str)).collect();
        return Ok(json!({ "views": names }));
    }

    Ok(json!({ "views": views }))
}

/// Read coordination state as a public helper for the renderer: section box, coordination mode.
/// Returns `(sbox_on, sbox_min, sbox_max, coord_mode_on, ghost_opacity)`.
pub fn coord_render_state(d: &Drawing) -> CoordRenderState {
    let sbox = coord_get(d, "coord_sectionbox");
    let sbox_on = sbox.get("on").and_then(Value::as_bool).unwrap_or(false);
    let sbox_min = arr3(&sbox, "min").unwrap_or([f64::NEG_INFINITY; 3]);
    let sbox_max = arr3(&sbox, "max").unwrap_or([f64::INFINITY; 3]);

    let mode = coord_get(d, "coord_mode");
    let mode_on = mode.get("on").and_then(Value::as_bool).unwrap_or(false);
    let ghost_opacity = mode.get("ghost_opacity").and_then(Value::as_f64).unwrap_or(0.2);

    CoordRenderState { sbox_on, sbox_min, sbox_max, mode_on, ghost_opacity }
}

/// Coordination rendering state read from the drawing.
#[derive(Clone, Debug, Default)]
pub struct CoordRenderState {
    pub sbox_on: bool,
    pub sbox_min: [f64; 3],
    pub sbox_max: [f64; 3],
    pub mode_on: bool,
    pub ghost_opacity: f64,
}

impl CoordRenderState {
    /// Whether a point is inside the section box (or the box is off).
    pub fn inside_sbox(&self, p: Vec3) -> bool {
        if !self.sbox_on {
            return true;
        }
        p.x >= self.sbox_min[0]
            && p.x <= self.sbox_max[0]
            && p.y >= self.sbox_min[1]
            && p.y <= self.sbox_max[1]
            && p.z >= self.sbox_min[2]
            && p.z <= self.sbox_max[2]
    }

    /// The 6 section planes of the section box (for clipping), empty if the box is off.
    pub fn sbox_planes(&self) -> Vec<cadaddy_doc::SectionPlane> {
        if !self.sbox_on {
            return Vec::new();
        }
        let lo = self.sbox_min;
        let hi = self.sbox_max;
        let big = 1e6;
        vec![
            // +X face: keep what is below max X (normal = -X, origin at max X).
            cadaddy_doc::SectionPlane {
                origin: Vec3::new(hi[0], (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(-1.0, 0.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -X face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new(lo[0], (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(1.0, 0.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // +Y face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, hi[1], (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(0.0, -1.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -Y face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, lo[1], (lo[2] + hi[2]) / 2.0),
                normal: Vec3::new(0.0, 1.0, 0.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // +Z face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]),
                normal: Vec3::new(0.0, 0.0, -1.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
            // -Z face.
            cadaddy_doc::SectionPlane {
                origin: Vec3::new((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]),
                normal: Vec3::new(0.0, 0.0, 1.0),
                width: big,
                height: big,
                live: true,
                name: String::new(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure3d_basic() {
        let mut s = Session::new();
        let r = run_measure3d(&mut s, &json!({ "from": [0, 0, 0], "to": [3, 4, 0] }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("distance")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((v - 5.0).abs() < 0.01, "distance = {v}");
    }

    #[test]
    fn measure3d_vertical() {
        let mut s = Session::new();
        let r = run_measure3d(&mut s, &json!({ "from": [0, 0, 0], "to": [0, 0, 10] }));
        let dz = r.as_ref().ok().and_then(|v| v.get("dz")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((dz - 10.0).abs() < 0.01);
    }

    #[test]
    fn uuid_format() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().nth(14), Some('4'));
    }

    #[test]
    fn zip_mini_valid() {
        let mut buf = Vec::new();
        {
            let mut z = zip_mini::ZipWriter::new(&mut buf);
            z.start_file("test.txt").ok();
            std::io::Write::write_all(&mut z, b"hello world").ok();
            z.finish().ok();
        }
        assert_eq!(&buf[..4], &[0x50, 0x4b, 0x03, 0x04]);
        assert!(buf.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
    }

    #[test]
    fn iso_date_format() {
        let d = iso_date();
        assert!(d.ends_with('Z'));
        assert_eq!(d.len(), 20);
    }

    #[test]
    fn xml_escape() {
        assert_eq!(escape_xml("a<b>c&d"), "a&lt;b&gt;c&amp;d");
    }

    #[test]
    fn arr3_parses() {
        let v = json!({ "pt": [1.0, 2.0, 3.0] });
        assert_eq!(arr3(&v, "pt"), Some([1.0, 2.0, 3.0]));
        assert_eq!(arr3(&v, "nope"), None);
    }

    #[test]
    fn coordination_mode_toggles() {
        let mut s = Session::new();
        let r = run_mode(&mut s, &json!({ "on": true, "ghost_opacity": 0.3 }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("on")).and_then(Value::as_bool).unwrap_or(false);
        assert!(v, "mode should be on");
        let opacity = r.as_ref().ok().and_then(|v| v.get("ghost_opacity")).and_then(Value::as_f64).unwrap_or(0.0);
        assert!((opacity - 0.3).abs() < 0.01);

        // Turn off.
        let r = run_mode(&mut s, &json!({ "on": false }));
        assert!(r.is_ok());
        let v = r.as_ref().ok().and_then(|v| v.get("on")).and_then(Value::as_bool).unwrap_or(true);
        assert!(!v, "mode should be off");
    }

    #[test]
    fn color_scheme_crud() {
        let mut s = Session::new();
        // Save.
        let r = run_colors(
            &mut s,
            &json!({
                "save": { "name": "Test Scheme", "colors": { "Supply": [255, 0, 0], "Return": [0, 0, 255] } }
            }),
        );
        assert!(r.is_ok());
        let saved = r.as_ref().ok().and_then(|v| v.get("saved")).and_then(Value::as_str);
        assert_eq!(saved, Some("Test Scheme"));

        // List.
        let r = run_colors(&mut s, &json!({ "list": true }));
        assert!(r.is_ok());
        let schemes = r.as_ref().ok().and_then(|v| v.get("schemes")).and_then(Value::as_array);
        assert_eq!(schemes.map(|a| a.len()), Some(1));

        // Delete.
        let r = run_colors(&mut s, &json!({ "delete": "Test Scheme" }));
        assert!(r.is_ok());
        let r = run_colors(&mut s, &json!({ "list": true }));
        let schemes = r.as_ref().ok().and_then(|v| v.get("schemes")).and_then(Value::as_array);
        assert_eq!(schemes.map(|a| a.len()), Some(0));
    }

    #[test]
    fn coord_views_save_and_delete() {
        let mut s = Session::new();
        let r = run_views(&mut s, &json!({ "save": { "name": "View A" } }));
        assert!(r.is_ok());
        let views = r.as_ref().ok().and_then(|v| v.get("views")).and_then(Value::as_array);
        assert_eq!(views.map(|a| a.len()), Some(1));

        // Delete.
        let r = run_views(&mut s, &json!({ "delete": "View A" }));
        assert!(r.is_ok());
        let views = r.as_ref().ok().and_then(|v| v.get("views")).and_then(Value::as_array);
        assert_eq!(views.map(|a| a.len()), Some(0));
    }

    #[test]
    fn coord_render_state_defaults() {
        let d = cadaddy_doc::Drawing::new_imperial();
        let crs = coord_render_state(&d);
        assert!(!crs.sbox_on);
        assert!(!crs.mode_on);
        assert!((crs.ghost_opacity - 0.2).abs() < 0.01);
    }

    #[test]
    fn sbox_planes_generates_six_when_on() {
        let crs = CoordRenderState { sbox_on: true, sbox_min: [-10.0, -10.0, 0.0], sbox_max: [10.0, 10.0, 20.0], mode_on: false, ghost_opacity: 0.2 };
        let planes = crs.sbox_planes();
        assert_eq!(planes.len(), 6);
        // A point inside should pass all planes.
        let inside = Vec3::new(0.0, 0.0, 10.0);
        for p in &planes {
            assert!(p.side(inside) >= 0.0, "inside point should be on kept side");
        }
        // A point outside should fail at least one.
        let outside = Vec3::new(20.0, 0.0, 10.0);
        assert!(planes.iter().any(|p| p.side(outside) < 0.0), "outside point should be clipped");
    }

    #[test]
    fn sbox_planes_empty_when_off() {
        let crs = CoordRenderState::default();
        assert!(crs.sbox_planes().is_empty());
    }

    #[test]
    fn inside_sbox_check() {
        let crs = CoordRenderState { sbox_on: true, sbox_min: [0.0, 0.0, 0.0], sbox_max: [10.0, 10.0, 10.0], ..CoordRenderState::default() };
        assert!(crs.inside_sbox(Vec3::new(5.0, 5.0, 5.0)));
        assert!(!crs.inside_sbox(Vec3::new(15.0, 5.0, 5.0)));
        // When off, everything is inside.
        let off = CoordRenderState::default();
        assert!(off.inside_sbox(Vec3::new(100.0, 100.0, 100.0)));
    }
}
