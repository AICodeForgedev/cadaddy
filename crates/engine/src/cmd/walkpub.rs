//! PUBLISH WALKTHROUGH (`walk.publish`): the drawing as a walkthrough package (`.cdywalk`,
//! [`cadaddy_walk3d::package`]) someone without CADaddy's files can walk — CADaddy on the desktop,
//! or its web build in a browser, opens it straight into Walk. The package holds the drawing
//! (with a building spec: its own, else one drafted from the walk rules, so it walks the same with
//! no `.walk.json` beside it), the built model as glTF for other viewers, and the baked lightmap.

use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::package::{LIGHTMAP_JSON, LIGHTMAP_PNG, MANIFEST, MODEL, Package};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("walk.publish", "Publish Walkthrough", run)
            .menu(&["View", "Walk and Fly", "Publish Walkthrough"])
            .params("{path?: where to write the .cdywalk (default beside the drawing, same name; on the web a download)} → {path, bytes, files: [names], spec: \"drawing\"|\"drafted\", model: bool (the built model is in it as glTF), lightmap: bool}"),
    ]
}

/// The package's default path: the drawing's file with `.cdywalk`, else its title.
fn default_path(s: &Session) -> Result<String> {
    let st = s.state()?;
    Ok(match &st.path {
        Some(p) => std::path::Path::new(p).with_extension(cadaddy_walk3d::package::EXTENSION).to_string_lossy().into_owned(),
        None => format!(
            "{}.{}",
            std::path::Path::new(&st.title).file_stem().map_or("Drawing".into(), |s| s.to_string_lossy().into_owned()),
            cadaddy_walk3d::package::EXTENSION
        ),
    })
}

/// The package of the open drawing.
pub fn package(s: &mut Session) -> Result<(Package, Value)> {
    let cmd = "walk.publish";
    let hooks = super::file::io().ok_or_else(|| crate::EngineError::Other("File formats are not available in this build.".into()))?;
    let mut d = s.doc()?.clone();
    let spec_from = if d.building_spec.as_deref().is_some_and(|t| BuildingSpec::from_json(t).is_ok()) {
        "drawing"
    } else {
        d.building_spec = Some(BuildingSpec::from_rules(&super::wallgen::rules_for(s, &json!({}))?).to_json());
        "drafted"
    };
    let bytes = (hooks.write)(&d, "walkthrough.cdywalk").map_err(|e| bad(cmd, e))?;
    let mut p = Package::decode(&bytes).map_err(|e| bad(cmd, e))?;
    // The built model, from a copy (the drawing itself is left as it is).
    let mut scratch = Session::empty();
    scratch.open_drawing(d, "walkthrough", None);
    let built = scratch.execute("model.build", &json!({})).is_ok();
    if built {
        p.put(MODEL, (hooks.write)(scratch.doc()?, "model.glb").map_err(|e| bad(cmd, e))?);
    }
    let lightmap = match super::lightbake::current(s) {
        Some(lm) => match cadaddy_render::lightmap::to_png(&lm) {
            Some(png) => {
                p.put(LIGHTMAP_PNG, png);
                p.put(LIGHTMAP_JSON, super::lightbake::index_json(&lm).to_string().into_bytes());
                true
            }
            None => false,
        },
        None => false,
    };
    let title = s.state()?.title.clone();
    if let Some(mut m) = p.manifest() {
        m["title"] = json!(title);
        m["spec"] = json!(spec_from);
        m["files"] = json!(p.files.iter().map(|f| f.0.clone()).collect::<Vec<_>>());
        p.put(MANIFEST, serde_json::to_vec_pretty(&m).unwrap_or_default());
    }
    let info = json!({"files": p.files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), "spec": spec_from, "model": built, "lightmap": lightmap});
    Ok((p, info))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "walk.publish";
    let path = match str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => x.to_string(),
        None => default_path(s)?,
    };
    let (pkg, mut info) = package(s)?;
    let bytes = pkg.encode().map_err(|e| bad(cmd, e))?;
    super::file::store_keeping(&path, &bytes, false)?;
    s.echo(format!("Walkthrough published: {path} (open it in CADaddy, on the desktop or the web, to walk it)."));
    info["path"] = json!(path);
    info["bytes"] = json!(bytes.len());
    Ok(info)
}
