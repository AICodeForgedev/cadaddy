//! BIM coordination viewer commands: model tree, trade visibility, section box, viewpoints,
//! 3D measure and BCF export.
//!
//! The coordination palette (UI) drives these commands to give the user a Navisworks-style
//! experience for reviewing MEP and structural coordination in 3D.
//!
//! Coordination-specific data (viewpoints, section box) is stored as extra JSON keys inside the
//! drawing's `building_spec` text. The `BuildingSpec` deserializer ignores unknown keys, so they
//! survive round-trips through the spec parser.

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
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("coordination.tree", "Coordination Tree", run_tree)
            .params("{level?} → {levels: [{name, elevation_ft, rooms, systems, elements}]}")
            .noundo(),
        CommandSpec::new("coordination.trades", "Trade Visibility", run_trades)
            .params("{set?: [{system, visible?}]} → {trades: [{system, kind, layer, visible, count}]}")
            .noundo(),
        CommandSpec::new("coordination.sectionbox", "Section Box", run_sectionbox)
            .params("{on?, min?, max?, fit?} → {on, min, max}")
            .noundo(),
        CommandSpec::new("coordination.viewpoints", "Coordination Viewpoints", run_viewpoints)
            .params("{save?: {name}, restore?, delete?} → {viewpoints}")
            .noundo(),
        CommandSpec::new("coordination.measure3d", "3D Measure", run_measure3d)
            .params("{from: [x,y,z], to: [x,y,z]} → {distance, dx, dy, dz}")
            .noundo(),
        CommandSpec::new("coordination.clashlist", "Clash List", run_clashlist)
            .params("{tolerance_in?, group_by?} → {groups, total}")
            .noundo(),
        CommandSpec::new("coordination.clashstatus", "Clash Status", run_clashstatus)
            .params("{id, status}")
            .noundo(),
        CommandSpec::new("coordination.bcf", "BCF Export", run_bcf)
            .params("{path, tolerance_in?} → {topics, path}")
            .noundo(),
    ]
}

// ─── Coordination data in building spec ───────────────────────────────────────

/// Read a coordination-specific key from the building spec JSON.
fn coord_get(d: &Drawing, key: &str) -> Value {
    d.building_spec
        .as_deref()
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| v.get(key).cloned())
        .unwrap_or(Value::Null)
}

/// Write a coordination-specific key into the building spec JSON.
fn coord_set(s: &mut Session, key: &str, val: &Value) -> Result<()> {
    let d = s.doc()?;
    let mut spec: Value = d
        .building_spec
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(json!({}));
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

        let mut sys_counts: std::collections::BTreeMap<String, (String, usize)> =
            std::collections::BTreeMap::new();
        for run in &sp.mep.runs {
            if run.level.eq_ignore_ascii_case(&level.name) {
                let entry = sys_counts.entry(run.system.clone()).or_insert_with(|| {
                    let kind = sp
                        .mep
                        .systems
                        .iter()
                        .find(|sy| sy.name == run.system)
                        .map_or("unknown", |sy| kind_str(sy.kind));
                    (kind.to_string(), 0)
                });
                entry.1 += 1;
            }
        }
        let systems: Vec<Value> = sys_counts
            .iter()
            .map(|(name, (kind, count))| json!({ "name": name, "kind": kind, "count": count }))
            .collect();

        let element_count = d
            .model
            .iter()
            .filter(|e| d.is_visible(e))
            .filter(|e| {
                super::bspec::made_level(e)
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case(&level.name))
            })
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
        let count = sp.mep.runs.iter().filter(|r| r.system == sys.name).count();
        trades.push(json!({
            "system": sys.name,
            "kind": kind_str(sys.kind),
            "layer": layer_name,
            "visible": visible,
            "count": count,
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
        let entry = json!({
            "name": name,
            "center": [vf.center.x, vf.center.y],
            "height": vf.height,
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
        let center = vp.get("center").and_then(Value::as_array).map(|a| {
            [
                a.first().and_then(Value::as_f64).unwrap_or(0.0),
                a.get(1).and_then(Value::as_f64).unwrap_or(0.0),
            ]
        });
        let height = vp.get("height").and_then(Value::as_f64);
        if let (Some(c), Some(h)) = (center, height) {
            s.execute("zoom", &json!({ "mode": "center", "center": c, "height": h }))?;
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
            "trade" => item
                .get("systems")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string(),
            "level" => "All".into(),
            _ => item.get("kind").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        };
        groups.entry(key).or_default().push(item.clone());
    }
    let grouped: Vec<Value> = groups
        .into_iter()
        .map(|(key, clashes)| json!({ "key": key, "clashes": clashes }))
        .collect();
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
        write!(zip, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <Version VersionId=\"2.1\"><DetailedVersion>2.1</DetailedVersion></Version>").map_err(io)?;

        let project_id = uuid_v4();
        zip.start_file("project.bcfp").map_err(io)?;
        write!(zip, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <ProjectExtension><Project ProjectId=\"{project_id}\">\
            <Name>CADaddy Coordination</Name></Project></ProjectExtension>").map_err(io)?;

        for (i, clash) in clashes.iter().enumerate().take(1000) {
            let guid = uuid_v4();
            let kind = clash.get("kind").and_then(Value::as_str).unwrap_or("unknown");
            let msg = escape_xml(clash.get("message").and_then(Value::as_str).unwrap_or(""));
            let title = escape_xml(&format!("CL-{:04}: {kind}", i + 1));
            let date = iso_date();

            zip.start_file(&format!("{guid}/markup.bcf")).map_err(io)?;
            write!(zip, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <Markup><Topic Guid=\"{guid}\" TopicType=\"Clash\" TopicStatus=\"New\">\
                <Title>{title}</Title><Description>{msg}</Description>\
                <CreationDate>{date}</CreationDate><CreationAuthor>CADaddy</CreationAuthor>\
                </Topic></Markup>").map_err(io)?;
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = now / 86400;
    let secs = now % 86400;
    let (mut y, mut m, mut day) = (1970u64, 1u64, 1u64);
    let mut rem = days;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if rem < yd { break; }
        rem -= yd;
        y += 1;
    }
    let md = [31, if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for d in md {
        if rem < d { day = rem + 1; break; }
        rem -= d;
        m += 1;
    }
    format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn uuid_v4() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::Instant::now().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let a = h.finish();
    h.write_u64(a.wrapping_mul(6364136223846793005));
    let b = h.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a as u32, (a >> 32) as u16, (a >> 48) & 0xfff,
        8 | ((b >> 60) & 3), (b >> 48) & 0xfff, b & 0xffffffffffff
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

    struct Entry { name: String, offset: u32, size: u32, crc: u32 }

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
                && entry.size == 0 && self.data_start > 0
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
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
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
}
