//! Point clouds: POINTCLOUDATTACH places a scan file (LAS, PTS, XYZ) in the drawing as a
//! reference under everything else; POINTCLOUDCLIP sets or removes its clipping box;
//! POINTCLOUDCOLORMAP colours it by the scan's colours, intensity, elevation or the object's
//! colour; POINTCLOUD.INFO reports its points and the octree's level-of-detail counts. The
//! points are read through the renderer's cloud cache (the host installs the reader).

use cadaddy_doc::pointcloud::{ColorMode, PointCloud};
use cadaddy_doc::{Entity, EntityKind};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::{CommandSpec, bad, has_doc, point_param, str_param, targets};
use crate::{Result, Session};

/// Reads a point cloud file's bytes (`io`'s reader, installed by the host).
pub type CloudReader = fn(&[u8], &str) -> std::result::Result<cadaddy_doc::pointcloud::Cloud, String>;

static READER: std::sync::OnceLock<CloudReader> = std::sync::OnceLock::new();

/// Install the point cloud file reader (once).
pub fn set_reader(f: CloudReader) {
    let _ = READER.set(f);
}

/// Largest scan file read.
const MAX_FILE_BYTES: u64 = 2 << 30;

/// The renderer's loader: the file read and sorted into the octree.
fn load(path: &str) -> Option<std::sync::Arc<cadaddy_doc::pointcloud::Cloud>> {
    let reader = READER.get()?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
            return None;
        }
        let bytes = crate::readfile::read(path).ok()?;
        reader(&bytes, path).ok().map(|c| std::sync::Arc::new(c.build()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (reader, path, MAX_FILE_BYTES);
        None
    }
}

pub fn specs() -> Vec<CommandSpec> {
    cadaddy_render::clouds::set_cloud_loader(load);
    vec![
        CommandSpec::new("pointcloudattach", "Attach Point Cloud", attach)
            .menu(&["Insert", "Point Cloud..."])
            .alias(&["pcattach"])
            .params("{path: .las | .pts | .xyz, at?: [x,y,z] (default 0,0,0), scale?: 1, rotation?: degrees, name?} → {handle, points, bounds}")
            .enabled(has_doc),
        CommandSpec::new("pointcloudclip", "Clip Point Cloud", clip)
            .params("{handle?, box?: [[x,y,z],[x,y,z]] in drawing coordinates | off?: true}")
            .enabled(has_doc),
        CommandSpec::new("pointcloudcolormap", "Point Cloud Color Map", colormap)
            .params("{handle?, mode: rgb|intensity|elevation|single}")
            .enabled(has_doc),
        CommandSpec::new("pointcloud.info", "Point Cloud Information", info)
            .params("{handle?, budget?: points (default 200000)} → {points, shown (after the clip), levels: [nodes per octree level], lod: points drawn for the budget}")
            .noundo()
            .enabled(has_doc),
    ]
}

fn vec3(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    let f = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
    Some(Vec3::new(f(0)?, f(1)?, f(2).unwrap_or(0.0)))
}

fn attach(s: &mut Session, p: &Value) -> Result<Value> {
    let path =
        str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad("pointcloudattach", "`path` is required"))?.to_string();
    cadaddy_render::clouds::forget_cloud(&path);
    let cloud = cadaddy_render::clouds::cloud(&path)
        .ok_or_else(|| bad("pointcloudattach", format!("{path}: not a point cloud this build can read (LAS, PTS, XYZ)")))?;
    let at = p.get("at").and_then(vec3).or_else(|| point_param(p, "at").map(|q| q.to3(0.0))).unwrap_or(Vec3::ZERO);
    let scale = p.get("scale").and_then(Value::as_f64).filter(|v| v.is_finite() && *v != 0.0).unwrap_or(1.0);
    let rotation = p.get("rotation").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0).to_radians();
    let name = str_param(p, "name")
        .map(str::to_string)
        .unwrap_or_else(|| std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let pc = PointCloud {
        path,
        name,
        insert: at,
        scale,
        rotation,
        clip: None,
        color_mode: ColorMode::Rgb,
        bounds: cloud.bounds(),
        count: cloud.points.len() as u64,
    };
    let common = s.current_common();
    let space = s.space();
    let d = s.doc_mut()?;
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::PointCloud(Box::new(pc.clone())));
    e.common = common;
    // A reference under the model: drawn first.
    d.space_mut(&space).ok_or_else(|| bad("pointcloudattach", "no current space"))?.push_front(e);
    Ok(
        json!({ "handle": h.hex(), "points": pc.count, "bounds": [[pc.bounds[0].x, pc.bounds[0].y, pc.bounds[0].z], [pc.bounds[1].x, pc.bounds[1].y, pc.bounds[1].z]] }),
    )
}

/// The point clouds among `targets`.
fn clouds(s: &Session, p: &Value, cmd: &str) -> Result<Vec<cadaddy_doc::Handle>> {
    let d = s.doc()?;
    let hs: Vec<_> = targets(s, p)?.into_iter().filter(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, EntityKind::PointCloud(_)))).collect();
    if hs.is_empty() {
        return Err(bad(cmd, "select a point cloud (`handle`)"));
    }
    Ok(hs)
}

fn clip(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = clouds(s, p, "pointcloudclip")?;
    let off = p.get("off").and_then(Value::as_bool).unwrap_or(false);
    let corners = match (off, p.get("box").and_then(Value::as_array)) {
        (true, _) => None,
        (false, Some(b)) => match (b.first().and_then(vec3), b.get(1).and_then(vec3)) {
            (Some(a), Some(c)) => Some((a, c)),
            _ => return Err(bad("pointcloudclip", "`box` is two corners [[x,y,z],[x,y,z]]")),
        },
        (false, None) => return Err(bad("pointcloudclip", "`box` or `off: true` is required")),
    };
    let d = s.doc_mut()?;
    for h in &hs {
        d.modify_entity(*h, |e| {
            if let EntityKind::PointCloud(pc) = &mut e.kind {
                pc.clip = corners.map(|(a, c)| {
                    // The box in the cloud's own coordinates (its rotation turns the box).
                    let (a, c) = (pc.unplace(a), pc.unplace(c));
                    [Vec3::new(a.x.min(c.x), a.y.min(c.y), a.z.min(c.z)), Vec3::new(a.x.max(c.x), a.y.max(c.y), a.z.max(c.z))]
                });
            }
        })?;
    }
    Ok(json!({ "clipped": hs.len(), "off": off }))
}

fn colormap(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = clouds(s, p, "pointcloudcolormap")?;
    let mode = match str_param(p, "mode").unwrap_or_default().to_ascii_lowercase().as_str() {
        "rgb" | "true" | "scan" => ColorMode::Rgb,
        "intensity" => ColorMode::Intensity,
        "elevation" => ColorMode::Elevation,
        "single" | "object" => ColorMode::Single,
        other => return Err(bad("pointcloudcolormap", format!("unknown mode `{other}` (rgb, intensity, elevation or single)"))),
    };
    let d = s.doc_mut()?;
    for h in &hs {
        d.modify_entity(*h, |e| {
            if let EntityKind::PointCloud(pc) = &mut e.kind {
                pc.color_mode = mode;
            }
        })?;
    }
    Ok(json!({ "changed": hs.len() }))
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let h = *clouds(s, p, "pointcloud.info")?.first().ok_or_else(|| bad("pointcloud.info", "no point cloud"))?;
    let Some(EntityKind::PointCloud(pc)) = s.doc()?.entity(h).map(|e| e.kind.clone()) else { return Err(bad("pointcloud.info", "no point cloud")) };
    let cloud = cadaddy_render::clouds::cloud(&pc.path).ok_or_else(|| bad("pointcloud.info", format!("{}: can't be read", pc.path)))?;
    let budget = p.get("budget").and_then(Value::as_u64).map_or(cadaddy_render::clouds::DRAW_BUDGET, |b| b.clamp(1, 50_000_000) as usize);
    let shown = cloud.points.iter().filter(|q| pc.shows(Vec3::new(f64::from(q[0]), f64::from(q[1]), f64::from(q[2])))).count();
    Ok(json!({
        "path": pc.path,
        "points": cloud.points.len(),
        "shown": shown,
        "colors": !cloud.colors.is_empty(),
        "intensity": !cloud.intensity.is_empty(),
        "levels": cloud.level_sizes(),
        "lod": cloud.lod(budget, None).len(),
    }))
}
