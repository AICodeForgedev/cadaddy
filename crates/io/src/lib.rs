//! CADaddy file formats: the native .cdy format, DXF/DWG read, SVG and SVGZ import,
//! SVG/PNG/PDF/DWFx/DWF export (plotting), DWFx underlay reading.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![allow(unknown_lints)] // the lint below exists only in newer clippy
#![allow(clippy::chunks_exact_to_as_chunks)]
#![forbid(unsafe_code)]

/// What a DXF file holds that CADaddy doesn't model, kept and written back (#528).
pub mod cdy;
pub mod dgn;
/// DWFx (XML Paper Specification / ECMA-388) and DWF 6 export/reading.
pub mod dwfx;
mod dwg_fixup;
mod dwg_keep;
pub mod dwg_native;
pub mod dwg_read;
pub mod dwg_write;
mod dxf_raw;
/// 3D model exchange: glTF/GLB, OBJ+MTL and STL.
pub mod ifc;
pub mod mesh3d_io;
pub mod threemf;

mod dxf_ext;
mod dxf_field;
mod dxf_geo;
mod dxf_image;
mod dxf_material;
mod dxf_mesh;
mod dxf_mleader;
mod dxf_r12;
mod dxf_read;
mod dxf_solid;
mod dxf_table;
mod dxf_underlay;
mod dxf_write;
mod dxf_xref;
pub mod eps;
pub mod pdf;
pub(crate) mod pdf_glyphs;
pub(crate) mod pdf_image;
pub mod pdf_markups;
pub mod pdf_merge;
pub mod pdf_read;
pub mod plotter;
pub mod pointcloud_io;
pub mod svg;
mod svg_read;
mod svg_style;
pub mod walkpkg;
pub mod wmf;

/// Multilines and their styles (MLINE, MLINESTYLE).
mod dxf_mline;
mod dxf_xclip;

use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::Bounds2;

pub use cadaddy_dxf::Version;
pub use dxf_read::{attrib_from_groups, entity_from_groups};
pub use dxf_write::{entity_groups, entity_records};
pub use pdf::{PdfOptions, pdf, plot};

/// Hook for DWFATTACH: one page (1-based) of a DWFx or DWF 6 file as a drawing in sheet inches,
/// and its report (`{sheet: [w, h], pages, name?, origin?, scale?}`, see [`dwfx::Page::report`]).
pub fn read_dwf_page(bytes: &[u8], page: u32) -> std::result::Result<(Drawing, serde_json::Value), String> {
    let p = dwfx::read_page(bytes, page).map_err(|e| e.to_string())?;
    let report = p.report();
    Ok((p.drawing, report))
}

/// Hook for DWFXOUT/DWFOUT/PUBLISH: spaces as one DWFx (`format: "dwfx"`, default) or DWF 6
/// (`"dwf"`) file, one sheet per space; `{scale?, fit?}` as [`dwfx::ExportOptions::from_json`].
pub fn write_dwfx_sheets(d: &Drawing, spaces: &[Space], opts: &serde_json::Value) -> std::result::Result<Vec<u8>, String> {
    let o = dwfx::ExportOptions::from_json(opts);
    let r = if opts.get("format").and_then(serde_json::Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("dwf")) {
        dwfx::export_dwf6_sheets(d, spaces, &o)
    } else {
        dwfx::export_sheets_with(d, spaces, &o)
    };
    r.map_err(|e| e.to_string())
}

/// How a drawing is saved as DXF or DWG.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// The file version; `None` saves in the drawing's own version ([`save_version`]).
    pub version: Option<Version>,
    /// Binary instead of ASCII DXF (DXF only; DWG is always binary).
    pub binary: bool,
}

/// The version a drawing is saved in when none is asked for: the version of the file it was
/// read from (`$ACADVER`, the nearest one CADaddy writes), and 2018 for a new drawing.
pub fn save_version(d: &Drawing) -> Version {
    d.header.get("ACADVER").and_then(cadaddy_doc::HVal::as_str).and_then(Version::for_acadver).unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{0}")]
    Format(String),
    #[error("unsupported file type `{0}`")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, IoError>;

/// `f` over `items` on all cores, in order (one at a time in the browser, which has no threads).
pub(crate) fn par_map<T: Sync, U: Send>(items: &[T], f: impl Fn(&T) -> U + Sync + Send) -> Vec<U> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        items.par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        items.iter().map(f).collect()
    }
}

/// File extensions the write dispatch ([`write_framed`]) handles — the file formats CADaddy
/// can save to, sorted. Internal/redirect extensions (empty, `bundle`, `cdywalk`, `dwt`,
/// `dws`) are omitted: `dwt`/`dws` are DWG content, `cdywalk` is an internal package.
pub const SAVE_EXTENSIONS: &[&str] =
    &["3mf", "cdy", "dgn", "dwf", "dwfx", "dwg", "dxf", "eps", "glb", "gltf", "ifc", "obj", "pdf", "png", "stl", "svg", "wmf"];

/// File extensions the read dispatch ([`read`]) handles — the file formats CADaddy can open,
/// sorted. Content-based detection (`.cdy`, `.dwg`) also works without the correct extension.
pub const READ_EXTENSIONS: &[&str] = &["cdy", "dgn", "dwg", "dxf", "glb", "gltf", "ifc", "obj", "pdf", "stl", "svg", "svgz"];

fn ext(name: &str) -> String {
    std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Which DWG reader and writer: CADaddy's own codec, or the DWG bridge (acadrust).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DwgEngine {
    Native,
    Bridge,
}

/// The DWG engine for reading (`read`) or writing: the one `CADADDY_DWG_READ` /
/// `CADADDY_DWG_WRITE` names for that direction, else the one `CADADDY_DWG` names for both
/// (`native`, or `acadrust`/`bridge`), else [`DEFAULT_DWG_READ`] / [`DEFAULT_DWG_WRITE`].
pub fn dwg_engine(read: bool) -> DwgEngine {
    dwg_engine_from(read, |var| std::env::var(var).ok())
}

/// [`dwg_engine`] over an explicit variable lookup (so the precedence is testable without
/// touching the process environment).
pub fn dwg_engine_from(read: bool, lookup: impl Fn(&str) -> Option<String>) -> DwgEngine {
    let named = |var: &str| match lookup(var).map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("native") => Some(DwgEngine::Native),
        Some("acadrust" | "bridge") => Some(DwgEngine::Bridge),
        _ => None,
    };
    named(if read { "CADADDY_DWG_READ" } else { "CADADDY_DWG_WRITE" }).or_else(|| named("CADADDY_DWG")).unwrap_or(if read {
        DEFAULT_DWG_READ
    } else {
        DEFAULT_DWG_WRITE
    })
}

/// Reading DWG: the bridge until CADaddy's codec reads what it reads (docs/dwg-parity.md lists
/// what's left); `CADADDY_DWG=native` reads with the codec.
pub const DEFAULT_DWG_READ: DwgEngine = DwgEngine::Bridge;

/// Writing DWG: the bridge until CADaddy's files have been opened in AutoCAD and reading has
/// parity; `CADADDY_DWG=native` writes with the codec.
pub const DEFAULT_DWG_WRITE: DwgEngine = DwgEngine::Bridge;

/// A DWG read with `engine`. CADaddy's reader hands what it can't read yet (encrypted or damaged
/// files) to the bridge, and says so in the drawing's read notes.
pub fn read_dwg(bytes: &[u8], engine: DwgEngine) -> Result<Drawing> {
    if engine == DwgEngine::Native {
        match dwg_read::read(bytes) {
            Ok((d, _)) => return Ok(d),
            Err(e) => {
                let mut d = read_dwg_bridge(bytes)?;
                d.read_notes.insert(0, format!("DWG: read through the DWG bridge (CADaddy's reader: {e})"));
                return Ok(d);
            }
        }
    }
    read_dwg_bridge(bytes)
}

/// A drawing written as DWG of `version` with `engine` (CADaddy's writer hands a version it
/// doesn't write to the bridge).
pub fn write_dwg(d: &Drawing, version: Version, engine: DwgEngine) -> Result<Vec<u8>> {
    if engine == DwgEngine::Native
        && let Ok((bytes, _)) = dwg_write::write(d, version.code())
    {
        return Ok(bytes);
    }
    let (dxf, linetypes) = dxf_write::dwg_bridge(d, version);
    let tags = dxf.iter().filter(|&&b| b == b'\n').count() / 2;
    let bytes = cadaddy_dwg::dxf_to_dwg_named(&dxf, version.code(), &linetypes).map_err(IoError::Format)?;
    drop(dxf);
    let (floor, ceil) = tag_caps();
    check_reopens("DWG", tags, dwg_tag_limit(bytes.len(), floor, ceil))?;
    if version <= Version::R2004 && has_images(d) {
        return Ok(dwg_fixup::image_def_reactors(&bytes).unwrap_or(bytes));
    }
    Ok(bytes)
}

/// Whether the drawing has a raster image anywhere.
fn has_images(d: &Drawing) -> bool {
    let any = |s: &cadaddy_doc::EntityStore| s.iter().any(|e| matches!(e.kind, cadaddy_doc::EntityKind::Image(_)));
    any(&d.model) || d.layouts.iter().any(|l| any(&l.entities)) || d.blocks.values().any(|b| any(&b.entities))
}

/// A DWG read through the bridge (acadrust).
fn read_dwg_bridge(bytes: &[u8]) -> Result<Drawing> {
    let (floor, ceil) = tag_caps();
    read_dwg_bridge_capped(bytes, floor, ceil)
}

/// The group-code limits in force: for DXF files (also the floor for a DWG's DXF) and the
/// ceiling for a DWG's. Tests lower them ([`tests::with_tag_caps`]) to try big files small.
fn tag_caps() -> (usize, usize) {
    #[cfg(test)]
    if let Some(c) = tests::TAG_CAPS.get() {
        return c;
    }
    (cadaddy_dxf::max_tags(), MAX_DWG_TAGS)
}

/// Most group codes of the DXF a DWG converts to that are read: the DWG is compressed, so its
/// DXF rendition runs to about 0.6 group codes per byte of DWG (a 98 MB DWG of 1.13 M objects
/// converts to 60 M), past the cap that guards DXF files. The converter's own byte limit
/// (`cadaddy_dwg::Limits`) still bounds what a hostile file can expand to.
const MAX_DWG_TAGS: usize = 120_000_000;
/// Group codes allowed per byte of DWG.
const DWG_TAGS_PER_BYTE: usize = 2;

/// The group-code limit for the DXF converted from a DWG of `dwg_len` bytes: scaled to the file
/// (a small file can't legitimately expand to millions), between `floor` and `ceil`.
fn dwg_tag_limit(dwg_len: usize, floor: usize, ceil: usize) -> usize {
    dwg_len.saturating_mul(DWG_TAGS_PER_BYTE).clamp(floor, ceil.max(floor))
}

/// Refuse a save whose file CADaddy itself would not reopen (`what` is the format), unless
/// oversize saves are allowed (`force`): `tags` group codes against what opening reads.
fn check_reopens(what: &str, tags: usize, limit: usize) -> Result<()> {
    if tags <= limit || cadaddy_dxf::oversize_allowed() {
        return Ok(());
    }
    Err(IoError::Format(format!(
        "this drawing is too large to save as {what}: the file would hold {tags} group codes and CADaddy opens at most {limit}, so it could not be reopened. \
         Save it as .cdy (no limit), or pass force:true to SAVE, SAVEAS or EXPORT to write the {what} anyway"
    )))
}

/// [`read_dwg_bridge`] with the group-code limit for the converted DXF between `floor` and `ceil`.
fn read_dwg_bridge_capped(bytes: &[u8], floor: usize, ceil: usize) -> Result<Drawing> {
    let limit = dwg_tag_limit(bytes.len(), floor, ceil);
    let renamed = dwg_fixup::underlay_classes_renamed(bytes);
    let bytes = renamed.as_deref().unwrap_or(bytes);
    let (dxf, notes) = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT).map_err(IoError::Format)?;
    // Custom records CADaddy kept through the DWG (`dwg_keep`), back where they were.
    let dxf = dwg_keep::unstash(&dxf).unwrap_or(dxf);
    let bad = |e: String| IoError::Format(format!("{e} (in the DXF converted from this DWG)"));
    // The text is let go once parsed: the drawing is built from the groups alone.
    let mut tags = cadaddy_dxf::parse_with_limit(&dxf, limit).map_err(|e| bad(e.to_string()))?;
    drop(dxf);
    dxf_read::mtext_dxf::bridge_carets_undone(&mut tags);
    let mut d = dxf_read::read_tags(tags).map_err(|e| bad(e.to_string()))?;
    // The bridge's notes first, then what reading the converted DXF dropped (undefined or out-of-range coordinates).
    d.read_notes.splice(0..0, notes);
    mark_dynamic_copies_anonymous(&mut d);
    Ok(d)
}

/// The bridge hands AutoCAD's anonymous copies of dynamic blocks (`*U7`) back under the dynamic
/// block's name, without the anonymous flag: they stay anonymous here, as when read from a DXF,
/// so block lists agree between the two formats. A DXF written from such a drawing names the copy
/// the same way, so the DXF reader marks them too.
pub(crate) fn mark_dynamic_copies_anonymous(d: &mut Drawing) {
    let copies: Vec<String> =
        d.blocks.iter().filter(|(k, b)| !b.anonymous && cadaddy_doc::dynblock::is_named_anonymous_copy(d, k)).map(|(k, _)| k.clone()).collect();
    for k in copies {
        if let Some(b) = d.blocks.get_mut(&k).map(std::sync::Arc::make_mut) {
            b.anonymous = true;
        }
    }
}

/// A DXF with the records CADaddy's DWG writers kept in their `CADADDY_KEEP` dictionary put
/// back: one of our DWGs saved as DXF by another program, which carries the dictionary along.
fn kept_back(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    match dwg_keep::has_key(bytes).then(|| dwg_keep::unstash(bytes)).flatten() {
        Some(d) => std::borrow::Cow::Owned(d),
        None => std::borrow::Cow::Borrowed(bytes),
    }
}

/// `bytes` without a leading UTF-8 byte-order mark (some editors and exporters write one).
fn without_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// Read a drawing; the format comes from the content (DXF ASCII/binary, DWG) or the name
/// (SVG and gzip-compressed SVGZ); a .cdy file is known by its content.
pub fn read(bytes: &[u8], name: &str) -> Result<Drawing> {
    // Nothing to read is never an empty drawing, whatever the format.
    if without_bom(bytes).iter().all(u8::is_ascii_whitespace) {
        return Err(IoError::Format(if bytes.is_empty() {
            "the file is empty (0 bytes)".into()
        } else {
            "the file has no content (only blank space)".into()
        }));
    }
    if cadaddy_walk3d::package::is_package(bytes) {
        return walkpkg::read(bytes);
    }
    if cdy::is_cdy(bytes) {
        return cdy::read_cdy(bytes);
    }
    if cadaddy_dwg::is_dwg(bytes) {
        return read_dwg(bytes, dwg_engine(true));
    }
    if dgn::is_v7(bytes) || (dgn::is_v8(bytes) && ext(name) == "dgn") {
        return dgn::read(bytes);
    }
    match ext(name).as_str() {
        "dgn" => dgn::read(bytes),
        "svg" | "svgz" => svg_read::read(bytes, name),
        "pdf" => {
            // a sheet opened on its own: an E sheet's scale note is used (nothing to register to)
            let r = pdf_read::read(bytes, &pdf_read::PdfOptions { e_sheet_at_note_scale: true, ..pdf_read::PdfOptions::default() })?;
            let mut d = r.drawing;
            d.read_notes.extend(r.notes);
            Ok(d)
        }
        "dxf" | "" => dxf_read::read(&kept_back(without_bom(bytes))),
        "obj" => mesh3d_io::read_obj(bytes, name),
        "stl" => mesh3d_io::read_stl(bytes),
        "glb" | "gltf" => mesh3d_io::read_gltf(bytes, name),
        "ifc" => ifc::read_ifc(bytes),
        e => {
            // A name that says one thing and content that is another: say so, not "unsupported".
            let wrong = |e: &str| match e {
                "dwg" => IoError::Format("not a DWG file (no AC10xx header)".into()),
                "cdy" => IoError::Format("not a CADaddy .cdy file".into()),
                e => IoError::Unsupported(e.to_string()),
            };
            if bytes.len() > 4 { dxf_read::read(without_bom(bytes)).map_err(|_| wrong(e)) } else { Err(wrong(e)) }
        }
    }
}

/// Write a drawing in the format chosen by the name's extension. Images (PNG, SVG, PDF) show
/// model space fitted to its extents. DXF and DWG are written in the drawing's version
/// ([`save_version`]), DXF as ASCII.
pub fn write(d: &Drawing, name: &str) -> Result<Vec<u8>> {
    write_framed(d, name, None)
}

/// Like [`write`], with the DXF/DWG version and binary DXF chosen by `opts`. Binary applies to
/// DXF only; asking for it with another format is an error.
pub fn write_with(d: &Drawing, name: &str, opts: &SaveOptions) -> Result<Vec<u8>> {
    let version = opts.version.unwrap_or_else(|| save_version(d));
    match ext(name).as_str() {
        "cdy" if !opts.binary => cdy::write_cdy(d),
        // A .cdy file without its preview (a job's checkpoint): internal, not offered to save as.
        "ckpt" if !opts.binary => cdy::write_cdy_checkpoint(d),
        "dxf" | "" if version == Version::R12 => {
            let tags = dxf_r12::tags(d);
            check_reopens("DXF", tags.len(), tag_caps().0)?;
            Ok(if opts.binary { cadaddy_dxf::write_binary(&tags, false) } else { cadaddy_dxf::write_ascii_as(&tags, false).into_bytes() })
        }
        _ if version == Version::R12 => Err(IoError::Format("Release 12 is a DXF version: save a .dxf file".into())),
        "dxf" | "" => {
            let tags = dxf_write::tags(d, version);
            check_reopens("DXF", tags.len(), tag_caps().0)?;
            Ok(if opts.binary {
                cadaddy_dxf::write_binary(&tags, version.utf8())
            } else {
                cadaddy_dxf::write_ascii_as(&tags, version.utf8()).into_bytes()
            })
        }
        // The DWG bridge reads our R2000 DXF and writes the DWG at the version asked for. Without
        // one, at least 2004: R2000 DWG has no true colours, transparency, table styles or
        // gradients, so a drawing from an R2000 DXF doesn't lose them by default.
        "dwg" if !opts.binary => {
            let version = opts.version.unwrap_or_else(|| version.max(Version::R2004));
            write_dwg(d, version, dwg_engine(false))
        }
        e if opts.binary => Err(IoError::Format(format!("binary output is for DXF files, not `{e}`"))),
        "bundle" if name.to_ascii_lowercase().ends_with(".obj.bundle") => {
            let stem = name.get(..name.len() - ".obj.bundle".len()).unwrap_or("export");
            let stem = std::path::Path::new(stem).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
            Ok(mesh3d_io::obj_bundle(d, &stem))
        }
        "glb" => Ok(mesh3d_io::write_glb(d)),
        "gltf" => Ok(mesh3d_io::write_gltf(d)),
        "cdywalk" => walkpkg::write(d),
        "stl" => Ok(mesh3d_io::write_stl(d)),
        "ifc" => ifc::write_ifc(d, name),
        "dgn" => dgn::write(d).map(|(b, _)| b),
        "3mf" => threemf::write_3mf(d),
        // The OBJ text; its MTL and textures come from `mesh3d_io::write_obj`.
        "obj" => {
            let stem = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
            Ok(mesh3d_io::write_obj(d, &format!("{stem}.mtl")).obj.into_bytes())
        }
        _ => write_framed(d, name, None),
    }
}

/// Like [`write`], but image formats (PNG, SVG, PDF) show `window` (a model-space rectangle in
/// drawing units, centred and scaled to fit the image without distortion) instead of the
/// extents. Formats that are not images (DXF, DWG) ignore the window. The window is checked with
/// [`check_window`].
pub fn write_framed(d: &Drawing, name: &str, window: Option<Bounds2>) -> Result<Vec<u8>> {
    // A drawing template (.dwt) and a standards file (.dws) are DWG content.
    if matches!(ext(name).as_str(), "dwt" | "dws") {
        return write_framed(d, &format!("{name}.dwg"), None);
    }
    if let Some(w) = &window
        && is_image(name)
    {
        check_window(w)?;
    }
    match ext(name).as_str() {
        "dxf" | "" | "dwg" | "cdy" | "ckpt" | "glb" | "gltf" | "stl" | "obj" | "bundle" | "cdywalk" | "ifc" | "3mf" | "dgn" => {
            write_with(d, name, &SaveOptions::default())
        }
        "svg" => Ok(svg::export_window(d, &Space::Model, window).into_bytes()),
        "eps" => Ok(eps::export(d, &Space::Model, window).into_bytes()),
        "wmf" => Ok(wmf::export(d, &Space::Model, window)),
        "png" => png_framed(d, &Space::Model, 2400, 1600, window),
        "pdf" => pdf::pdf(d, &Space::Model, &PdfOptions { compress: true, window, ..PdfOptions::default() }),
        "dwfx" => dwfx::export(d, &Space::Model),
        "dwf" => dwfx::export_dwf6(d, &Space::Model),
        e => Err(IoError::Unsupported(e.to_string())),
    }
}

/// Whether `name` is written as an image (PNG, SVG, PDF), i.e. whether framing applies to it.
pub fn is_image(name: &str) -> bool {
    matches!(ext(name).as_str(), "png" | "svg" | "pdf" | "eps" | "wmf")
}

/// Largest coordinate magnitude accepted in an export window.
pub const MAX_WINDOW_COORD: f64 = 1.0e12;

/// Validate an export window: finite corners within ±[`MAX_WINDOW_COORD`], and a width and height
/// that are positive and not vanishingly small next to the coordinates (at least 1e-9 of the
/// largest coordinate magnitude, and at least 1e-9 drawing units).
pub fn check_window(w: &Bounds2) -> Result<()> {
    let coords = [w.min.x, w.min.y, w.max.x, w.max.y];
    if coords.iter().any(|c| !c.is_finite()) {
        return Err(IoError::Format("export window: coordinates must be finite numbers".into()));
    }
    let mag = coords.iter().fold(0.0_f64, |m, c| m.max(c.abs()));
    if mag > MAX_WINDOW_COORD {
        return Err(IoError::Format(format!("export window: coordinates must lie within ±{MAX_WINDOW_COORD:e}")));
    }
    let min_side = mag.max(1.0) * 1e-9;
    let (width, height) = (w.max.x - w.min.x, w.max.y - w.min.y);
    if !(width >= min_side && height >= min_side) {
        return Err(IoError::Format(format!("export window: width and height must be positive (got {width} x {height})")));
    }
    Ok(())
}

/// Render a space to PNG, fitted to its extents.
pub fn png(d: &Drawing, space: &Space, width: u32, height: u32) -> Result<Vec<u8>> {
    png_framed(d, space, width, height, None)
}

/// Render a space to PNG showing `window` (centred, fitted without margin), or fitted to its
/// extents (5% margin) when `window` is `None`. Objects are drawn with their transparency, as on
/// screen.
pub fn png_framed(d: &Drawing, space: &Space, width: u32, height: u32, window: Option<Bounds2>) -> Result<Vec<u8>> {
    // Curves need be no finer than half a pixel of the image: a thumbnail of a large drawing was
    // tessellated at plot detail (0.001 units), most of a .cdy save's time at 200k objects.
    let frame = window.unwrap_or_else(|| d.extents(space));
    let scale = png_view(&frame, width, height, window).scale;
    let defaults = cadaddy_render::Options::default();
    let tolerance = if frame.is_empty() || !(0.5 / scale).is_finite() { defaults.tolerance } else { (0.5 / scale).max(1e-9) };
    let list = cadaddy_render::build(d, space, &cadaddy_render::Options { transparency: true, tolerance, ..defaults });
    let view = png_view(&list.bounds, width, height, window);
    cadaddy_render::raster::render_png(&list, &view, &cadaddy_render::raster::RasterOptions::default())
        .ok_or_else(|| IoError::Format("render failed".into()))
}

/// Render a space to PNG fitted to its extents, building the display list with `opts` (a
/// preview's level of detail, for example).
pub fn png_with(d: &Drawing, space: &Space, width: u32, height: u32, opts: &cadaddy_render::Options) -> Result<Vec<u8>> {
    let list = cadaddy_render::build(d, space, opts);
    let view = png_view(&list.bounds, width, height, None);
    cadaddy_render::raster::render_png(&list, &view, &cadaddy_render::raster::RasterOptions::default())
        .ok_or_else(|| IoError::Format("render failed".into()))
}

/// The raster view a PNG export uses.
pub fn png_view(extents: &Bounds2, width: u32, height: u32, window: Option<Bounds2>) -> cadaddy_render::raster::View {
    match window {
        Some(w) => cadaddy_render::raster::View::fit(&w, width, height, 0.0),
        None => cadaddy_render::raster::View::fit(extents, width, height, 0.05),
    }
}

pub fn read_dxf(bytes: &[u8]) -> Result<Drawing> {
    dxf_read::read(bytes)
}
/// The drawing as R2000 ASCII DXF, whatever its own version ([`write_with`] chooses one).
pub fn write_dxf(d: &Drawing) -> String {
    dxf_write::write(d)
}

/// Regions, written as closed polylines.
mod dxf_region;

#[cfg(test)]
mod tests;
