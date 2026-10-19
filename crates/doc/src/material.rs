//! Materials: what a surface looks like in shaded and rendered views. A drawing holds its own
//! materials ([`crate::Drawing::materials`]); a layer can have one attached (MATERIALATTACH) and an
//! object one assigned (MATERIALASSIGN; otherwise its layer's), with how textures map onto it
//! (MATERIALMAP). Physically based: base colour, roughness, metalness, opacity and refraction, each
//! texture either a file (an image, a path relative to the drawing) or a recipe of CADaddy's own
//! material library, generated rather than stored.

use serde::{Deserialize, Serialize};

use crate::color::Rgb;

/// Names of the materials every drawing has and never saves as its own.
pub const GLOBAL: &str = "Global";
pub const BY_LAYER: &str = "ByLayer";
pub const BY_BLOCK: &str = "ByBlock";
/// Most materials one drawing holds.
pub const MAX_MATERIALS: usize = 4096;
/// Longest name, description or texture path kept.
pub const MAX_TEXT: usize = 1024;

/// Where a texture comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Texture {
    /// An image file: an absolute path, or one relative to the drawing's folder.
    File(String),
    /// A recipe of CADaddy's material library, by id (`brick-red`, `oak`…): generated.
    Library(String),
}

/// How a texture is laid onto a surface (MATERIALMAP).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Projection {
    /// Along one axis, onto a plane (the surface's own plane for flat objects).
    #[default]
    Planar,
    /// From the six sides of a box: each face takes the side it faces most.
    Box,
    /// Wrapped around a cylinder (Z axis).
    Cylindrical,
    /// Wrapped around a sphere.
    Spherical,
}

impl Projection {
    pub const ALL: [Projection; 4] = [Projection::Planar, Projection::Box, Projection::Cylindrical, Projection::Spherical];

    pub fn as_str(self) -> &'static str {
        match self {
            Projection::Planar => "planar",
            Projection::Box => "box",
            Projection::Cylindrical => "cylindrical",
            Projection::Spherical => "spherical",
        }
    }

    pub fn parse(s: &str) -> Option<Projection> {
        let s = s.trim().to_ascii_lowercase();
        Projection::ALL.into_iter().find(|p| {
            p.as_str() == s
                || p.as_str().starts_with(&s) && s.len() >= 2
                || (s == "cylinder" && *p == Projection::Cylindrical)
                || (s == "sphere" && *p == Projection::Spherical)
        })
    }

    /// The DXF code of a MATERIAL's mapper projection (1 planar, 2 box, 3 cylinder, 4 sphere).
    pub fn dxf(self) -> i64 {
        match self {
            Projection::Planar => 1,
            Projection::Box => 2,
            Projection::Cylindrical => 3,
            Projection::Spherical => 4,
        }
    }

    pub fn from_dxf(v: i64) -> Projection {
        match v {
            2 => Projection::Box,
            3 => Projection::Cylindrical,
            4 => Projection::Spherical,
            _ => Projection::Planar,
        }
    }
}

/// A texture mapping: projection, the size one tile of the texture covers (drawing units), its
/// offset and rotation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mapping {
    pub projection: Projection,
    /// Width and height one tile covers, in drawing units (> 0).
    pub size_u: f64,
    pub size_v: f64,
    pub offset_u: f64,
    pub offset_v: f64,
    /// Rotation, radians counterclockwise.
    pub rotation: f64,
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping { projection: Projection::Planar, size_u: 1.0, size_v: 1.0, offset_u: 0.0, offset_v: 0.0, rotation: 0.0 }
    }
}

impl Mapping {
    /// The mapping with sizes kept positive and finite and numbers finite (hostile input made safe).
    pub fn sane(mut self) -> Mapping {
        let size = |v: f64| if v.is_finite() && v > 1e-9 { v.min(1e9) } else { 1.0 };
        let num = |v: f64| if v.is_finite() { v.clamp(-1e9, 1e9) } else { 0.0 };
        self.size_u = size(self.size_u);
        self.size_v = size(self.size_v);
        self.offset_u = num(self.offset_u);
        self.offset_v = num(self.offset_v);
        self.rotation = num(self.rotation);
        self
    }

    /// Texture coordinates (in tiles) of a point on a surface with normal `n`.
    pub fn uv(&self, p: crate::geom::Vec3, n: crate::geom::Vec3) -> (f64, f64) {
        let (u, v) = match self.projection {
            // In the surface's own plane: across it horizontally, and up it (or along Y on a
            // horizontal surface).
            Projection::Planar => {
                let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                if !(len.is_finite() && len > 1e-12) || (n.z / len).abs() > 0.999 {
                    (p.x, p.y)
                } else {
                    let n = crate::geom::Vec3::new(n.x / len, n.y / len, n.z / len);
                    let h = (n.x * n.x + n.y * n.y).sqrt();
                    let u_axis = crate::geom::Vec3::new(-n.y / h, n.x / h, 0.0);
                    let v_axis = n.cross(u_axis);
                    (p.dot(u_axis), p.dot(v_axis))
                }
            }
            Projection::Box => {
                let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
                if az >= ax && az >= ay {
                    (p.x, p.y)
                } else if ax >= ay {
                    (p.y, p.z)
                } else {
                    (p.x, p.z)
                }
            }
            Projection::Cylindrical => {
                let r = (p.x * p.x + p.y * p.y).sqrt();
                (p.y.atan2(p.x) * r.max(1e-12), p.z)
            }
            Projection::Spherical => {
                let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-12);
                (p.y.atan2(p.x) * r, (p.z / r).clamp(-1.0, 1.0).asin() * r)
            }
        };
        let (s, c) = self.rotation.sin_cos();
        let (ru, rv) = (u * c + v * s, -u * s + v * c);
        ((ru - self.offset_u) / self.size_u, (rv - self.offset_v) / self.size_v)
    }
}

/// A material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Material {
    pub name: String,
    pub description: String,
    /// The base (diffuse) colour, under a base colour texture when there is one.
    pub color: Rgb,
    pub color_map: Option<Texture>,
    /// 0 = mirror smooth … 1 = fully rough.
    pub roughness: f64,
    pub roughness_map: Option<Texture>,
    /// 0 = dielectric … 1 = metal.
    pub metallic: f64,
    pub normal_map: Option<Texture>,
    /// How strongly the normal map bends the surface (0 … 2, 1 = as authored).
    pub normal_strength: f64,
    /// 1 = opaque … 0 = invisible.
    pub opacity: f64,
    /// Index of refraction of a transparent material (glass 1.5).
    pub refraction: f64,
    /// The material's own texture mapping (an object's MATERIALMAP overrides it).
    pub mapping: Mapping,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: String::new(),
            description: String::new(),
            color: Rgb(180, 180, 180),
            color_map: None,
            roughness: 0.5,
            roughness_map: None,
            metallic: 0.0,
            normal_map: None,
            normal_strength: 1.0,
            opacity: 1.0,
            refraction: 1.0,
            mapping: Mapping::default(),
        }
    }
}

fn cut(s: &str) -> String {
    s.trim().chars().take(MAX_TEXT).collect()
}

impl Material {
    pub fn new(name: &str) -> Self {
        Material { name: cut(name), ..Material::default() }
    }

    /// The material with every number in range and text capped (hostile input made safe).
    pub fn sane(mut self) -> Material {
        let unit = |v: f64, d: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { d };
        self.name = cut(&self.name);
        self.description = cut(&self.description);
        self.roughness = unit(self.roughness, 0.5);
        self.metallic = unit(self.metallic, 0.0);
        self.opacity = unit(self.opacity, 1.0);
        self.normal_strength = if self.normal_strength.is_finite() { self.normal_strength.clamp(0.0, 2.0) } else { 1.0 };
        self.refraction = if self.refraction.is_finite() { self.refraction.clamp(1.0, 5.0) } else { 1.0 };
        for t in [&mut self.color_map, &mut self.roughness_map, &mut self.normal_map] {
            if let Some(Texture::File(p) | Texture::Library(p)) = t.as_mut() {
                *p = cut(p);
            }
            if matches!(t, Some(Texture::File(p) | Texture::Library(p)) if p.is_empty()) {
                *t = None;
            }
        }
        self.mapping = self.mapping.sane();
        self
    }

    /// Whether `name` is one of the materials every drawing has ([`GLOBAL`], [`BY_LAYER`],
    /// [`BY_BLOCK`]).
    pub fn is_builtin(name: &str) -> bool {
        [GLOBAL, BY_LAYER, BY_BLOCK].iter().any(|b| b.eq_ignore_ascii_case(name.trim()))
    }
}

/// A pattern of the material library: what its textures look like.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pattern {
    /// `plain`, `speckle`, `brick`, `tile`, `planks`, `wood`, `carpet`, `ceiling`, `marble`,
    /// `brushed`.
    pub kind: String,
    /// Strength of the colour variation (0 … 1).
    pub contrast: f64,
    /// Octaves of noise detail.
    pub detail: u32,
    /// Size of a brick, tile or plank, metres (width, height).
    pub unit: [f64; 2],
    /// Joint (mortar, grout) width, metres.
    pub joint: f64,
    pub joint_color: Option<String>,
    /// Colour variation from one brick, tile or plank to the next (0 … 1).
    pub variation: f64,
    /// Growth rings across a tile of wood.
    pub rings: u32,
    /// Carpet tiles: tile size in metres (0: broadloom).
    pub grid: f64,
    /// Speckle over tiles (0 … 1).
    pub speckle: f64,
}

/// A material of CADaddy's library (`assets/materials/library.json`): how its textures are made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub category: String,
    /// Base colour, `#rrggbb`.
    pub color: String,
    pub roughness: f64,
    pub metallic: f64,
    pub opacity: Option<f64>,
    pub refraction: Option<f64>,
    pub pattern: Pattern,
    /// The real size one tile of the textures covers, metres (width, height).
    pub tile: [f64; 2],
}

#[derive(Deserialize)]
struct Library {
    materials: Vec<Recipe>,
}

/// `#rrggbb` as a colour.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// CADaddy's material library.
pub fn library() -> &'static [Recipe] {
    static LIB: std::sync::OnceLock<Vec<Recipe>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| {
        serde_json::from_str::<Library>(include_str!("../../../assets/materials/library.json")).map(|l| l.materials).unwrap_or_default()
    })
}

/// The library recipe `id`.
pub fn recipe(id: &str) -> Option<&'static Recipe> {
    library().iter().find(|r| r.id.eq_ignore_ascii_case(id.trim()))
}

/// Metres per drawing unit for INSUNITS `units` (unitless: millimetres for metric drawings
/// (`metric`), else inches).
pub fn metres_per_unit(units: i64, metric: bool) -> f64 {
    match units {
        1 => 0.0254,
        2 => 0.3048,
        3 => 1609.344,
        4 => 0.001,
        5 => 0.01,
        6 => 1.0,
        7 => 1000.0,
        14 => 0.1,
        _ if metric => 0.001,
        _ => 0.0254,
    }
}

impl Recipe {
    /// The recipe as a drawing material, its textures from the library and one tile sized for a
    /// drawing whose unit is `unit_m` metres.
    pub fn material(&self, unit_m: f64) -> Material {
        let tex = Some(Texture::Library(self.id.clone()));
        let unit_m = if unit_m.is_finite() && unit_m > 0.0 { unit_m } else { 1.0 };
        let plain = self.pattern.kind == "plain";
        Material {
            name: self.name.clone(),
            description: format!("CADaddy library: {}", self.category),
            color: parse_hex(&self.color).unwrap_or(Rgb(180, 180, 180)),
            color_map: if plain { None } else { tex.clone() },
            roughness: self.roughness,
            roughness_map: if plain { None } else { tex.clone() },
            metallic: self.metallic,
            normal_map: if plain { None } else { tex },
            normal_strength: 1.0,
            opacity: self.opacity.unwrap_or(1.0),
            refraction: self.refraction.unwrap_or(1.0),
            mapping: Mapping { size_u: self.tile[0] / unit_m, size_v: self.tile[1] / unit_m, ..Mapping::default() },
        }
        .sane()
    }
}

/// The extended data application an object's own texture mapping (MATERIALMAP) is kept under:
/// part of the object, so copies carry it and DXF and DWG files keep it.
pub const MAP_APP: &str = "CADADDY_MATERIALMAP";

/// The range of `xdata` that is [`MAP_APP`]'s.
fn map_range(xdata: &[crate::RawTag]) -> Option<std::ops::Range<usize>> {
    let start = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(MAP_APP))?;
    let len = xdata.get(start + 1..)?.iter().take_while(|t| t.code != 1001).count();
    Some(start..start + 1 + len)
}

/// An object's own texture mapping (MATERIALMAP), if it has one.
pub fn object_mapping(xdata: &[crate::RawTag]) -> Option<Mapping> {
    let r = map_range(xdata)?;
    let g = xdata.get(r.start + 1..r.end)?;
    let projection = Projection::from_dxf(g.iter().find(|t| t.code == 1070)?.value.trim().parse().ok()?);
    let reals: Vec<f64> = g.iter().filter(|t| t.code == 1040).filter_map(|t| t.value.trim().parse().ok()).collect();
    let [size_u, size_v, offset_u, offset_v, rotation] = reals.get(..5)?.try_into().ok()?;
    Some(Mapping { projection, size_u, size_v, offset_u, offset_v, rotation }.sane())
}

/// Give an object its own texture mapping (`None`: its material's), keeping other applications'
/// extended data as it is.
pub fn set_object_mapping(xdata: &mut Vec<crate::RawTag>, m: Option<&Mapping>) {
    let at = map_range(xdata).map(|r| {
        xdata.drain(r.clone());
        r.start
    });
    let Some(m) = m.map(|m| m.sane()) else { return };
    let t = |code: i32, value: String| crate::RawTag { code, value };
    let mut groups = vec![t(1001, MAP_APP.into()), t(1070, m.projection.dxf().to_string())];
    groups.extend([m.size_u, m.size_v, m.offset_u, m.offset_v, m.rotation].into_iter().map(|v| t(1040, v.to_string())));
    let at = at.unwrap_or(xdata.len()).min(xdata.len());
    xdata.splice(at..at, groups);
}

impl crate::Drawing {
    /// The drawing's material named `name` (any case).
    pub fn material(&self, name: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The material object `e` shows: its own, else its layer's (`None`: Global, the plain
    /// look). `block_material` is what ByBlock resolves to (the block reference's).
    pub fn material_of(&self, e: &crate::Entity, block_material: Option<&str>) -> Option<&Material> {
        let own = e.common.material.as_deref().map(str::trim).filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case(BY_LAYER));
        let name = match own {
            Some(m) if m.eq_ignore_ascii_case(BY_BLOCK) => block_material?,
            Some(m) => m,
            None => self.layer(&e.common.layer)?.material.as_deref()?,
        };
        if name.eq_ignore_ascii_case(GLOBAL) { None } else { self.material(name) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vec3;

    #[test]
    fn hostile_values_are_made_safe() {
        let m = Material {
            name: "x".repeat(5000),
            roughness: f64::NAN,
            metallic: 9.0,
            opacity: -1.0,
            refraction: f64::INFINITY,
            normal_strength: 99.0,
            color_map: Some(Texture::File("  ".into())),
            mapping: Mapping { size_u: 0.0, size_v: f64::NAN, rotation: f64::INFINITY, ..Mapping::default() },
            ..Material::default()
        }
        .sane();
        assert_eq!(m.name.len(), MAX_TEXT);
        assert_eq!((m.roughness, m.metallic, m.opacity, m.refraction, m.normal_strength), (0.5, 1.0, 0.0, 1.0, 2.0));
        assert_eq!(m.color_map, None);
        assert_eq!((m.mapping.size_u, m.mapping.size_v, m.mapping.rotation), (1.0, 1.0, 0.0));
        assert!(Material::is_builtin(" bylayer "));
        assert_eq!(Projection::parse("BOX"), Some(Projection::Box));
        assert_eq!(Projection::parse("cylinder"), Some(Projection::Cylindrical));
        assert_eq!(Projection::parse("zz"), None);
        assert_eq!(Projection::from_dxf(Projection::Spherical.dxf()), Projection::Spherical);
    }

    #[test]
    fn the_library_loads_and_makes_materials() {
        let lib = library();
        assert!(lib.len() >= 15, "{}", lib.len());
        let mut ids = std::collections::HashSet::new();
        for r in lib {
            assert!(ids.insert(r.id.clone()), "duplicate id {}", r.id);
            assert!(parse_hex(&r.color).is_some(), "{}: colour", r.id);
            assert!(r.tile[0] > 0.0 && r.tile[1] > 0.0, "{}: tile", r.id);
            assert!(!r.name.is_empty() && !r.category.is_empty(), "{}", r.id);
        }
        // Sized for the drawing: a 0.45 m brick tile is 450 mm or about 17.7 in.
        let brick = recipe("BRICK-RED").unwrap();
        assert!((brick.material(metres_per_unit(4, true)).mapping.size_u - 450.0).abs() < 1e-9);
        assert!((brick.material(metres_per_unit(1, false)).mapping.size_u - 0.45 / 0.0254).abs() < 1e-9);
        assert_eq!(brick.material(0.001).color_map, Some(Texture::Library("brick-red".into())));
        // Glass is plain and see-through.
        let glass = recipe("glass-clear").unwrap().material(1.0);
        assert!(glass.color_map.is_none() && glass.opacity < 0.5 && glass.refraction > 1.4);
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn an_objects_mapping_lives_in_its_xdata() {
        let other = crate::RawTag { code: 1001, value: "ACME".into() };
        let mut x = vec![other.clone(), crate::RawTag { code: 1000, value: "a".into() }];
        let m = Mapping { projection: Projection::Cylindrical, size_u: 250.0, size_v: 125.0, offset_u: 1.0, offset_v: 2.0, rotation: 0.5 };
        set_object_mapping(&mut x, Some(&m));
        assert_eq!(object_mapping(&x), Some(m));
        assert_eq!(x.first(), Some(&other));
        set_object_mapping(&mut x, Some(&Mapping::default()));
        assert_eq!(object_mapping(&x), Some(Mapping::default()));
        assert_eq!(x.iter().filter(|t| t.code == 1001).count(), 2, "replaced, not added");
        set_object_mapping(&mut x, None);
        assert_eq!(object_mapping(&x), None);
        assert_eq!(x.len(), 2);
        // Damaged data reads as none.
        let bad = vec![crate::RawTag { code: 1001, value: MAP_APP.into() }, crate::RawTag { code: 1070, value: "x".into() }];
        assert_eq!(object_mapping(&bad), None);
    }

    #[test]
    fn mapping_gives_tiles() {
        let m = Mapping { size_u: 2.0, size_v: 0.5, ..Mapping::default() };
        assert_eq!(m.uv(Vec3::new(4.0, 1.0, 9.0), Vec3::Z), (2.0, 2.0));
        // Planar on a wall: across it and up it, so the texture stands upright.
        let wall = Mapping::default().uv(Vec3::new(0.0, 3.0, 2.0), Vec3::new(1.0, 0.0, 0.0));
        assert!((wall.0 - 3.0).abs() < 1e-12 && (wall.1 - 2.0).abs() < 1e-12, "{wall:?}");
        // Box: a wall facing X takes Y and Z.
        let b = Mapping { projection: Projection::Box, ..Mapping::default() };
        assert_eq!(b.uv(Vec3::new(5.0, 2.0, 3.0), Vec3::new(1.0, 0.0, 0.0)), (2.0, 3.0));
        // Rotated a quarter turn.
        let r = Mapping { rotation: std::f64::consts::FRAC_PI_2, ..Mapping::default() };
        let (u, v) = r.uv(Vec3::new(1.0, 0.0, 0.0), Vec3::Z);
        assert!(u.abs() < 1e-12 && (v + 1.0).abs() < 1e-12);
    }
}
