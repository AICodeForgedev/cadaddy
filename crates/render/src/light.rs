//! Physically based lighting for rendered views (the Realistic visual style): the drawing's sun
//! (placed by date, time and place) and an analytic sky that lights everything from every
//! direction, its LIGHT objects (point, spot, distant), surfaces as microfacet materials (GGX
//! distribution, Smith masking, Schlick Fresnel: base colour, roughness, metalness), and a filmic
//! tone curve with exposure (RENDEREXPOSURE). The CPU rasterizer uses this module; the GPU shader
//! (`ui-egui/src/shaded/shader.wgsl`) implements the same equations.
//!
//! Units are relative: the sun gives an irradiance of [`SUN`] at full height; a light's candela
//! become irradiance through [`CANDELA`] (so a 1500 cd lamp 1.5 m away lights about like the
//! sky), with distances in metres from the drawing's units.

use cadaddy_doc::lighting::{Attenuation, LightKind, ToneMap};
use cadaddy_doc::{Drawing, EntityKind, Space};
use cadaddy_geom::Vec3;

/// The sun's irradiance overhead (before exposure).
pub const SUN: f32 = 3.2;
/// Irradiance per candela at 1 m.
pub const CANDELA: f32 = 1.0 / 900.0;
/// Most lights a view uses (the GPU shader holds as many).
pub const MAX_LIGHTS: usize = 16;

/// A light as the shading uses it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    /// Distant lights: towards the light; else the position.
    pub pos: Vec3,
    /// Spot: the direction it points.
    pub dir: Vec3,
    /// Linear colour times intensity (irradiance at 1 m, or of the parallel rays).
    pub rgb: [f32; 3],
    pub kind: LightKind,
    pub attenuation: Attenuation,
    /// Cosines of the spot's hotspot and falloff half-angles.
    pub cos_inner: f32,
    pub cos_outer: f32,
}

/// The sky: colours at the zenith and the horizon, and of the ground below.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub ground: [f32; 3],
}

/// Fog / depth cue applied after shading: objects fade towards a colour with distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParams {
    /// Linear RGB fog colour.
    pub color: [f32; 3],
    /// Depth where fog begins (as a fraction of the view's depth range, 0..1).
    pub near: f32,
    /// Depth where fog is fully opaque.
    pub far: f32,
}

/// Background for a rendered view.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Background {
    /// The analytic sky (default).
    #[default]
    Sky,
    /// A solid colour (linear RGB).
    Solid([f32; 3]),
    /// A vertical gradient (bottom, top; linear RGB).
    Gradient { bottom: [f32; 3], top: [f32; 3] },
    /// An image (the decoded raster, from the image cache).
    Image(std::sync::Arc<crate::images::Raster>),
}

/// Everything that lights a view.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    /// Towards the sun, and its linear colour times irradiance; `None` at night or switched off.
    pub sun: Option<(Vec3, [f32; 3])>,
    pub sky: Sky,
    pub lights: Vec<PointLight>,
    /// 2^RENDEREXPOSURE.
    pub exposure: f32,
    /// Metres per drawing unit (light falloff).
    pub unit_m: f64,
    /// Image-based lighting: an HDR environment map; replaces the analytic sky when set.
    pub env_map: Option<std::sync::Arc<crate::hdr::HdrImage>>,
    /// Fog / depth cue; `None` = no fog.
    pub fog: Option<FogParams>,
    /// Background override; `None` = the sky gradient.
    pub background: Background,
    /// White balance: colour multiplier from temperature/tint (linear RGB).
    pub white_balance: [f32; 3],
    /// Screen-space ambient occlusion strength (0 off … 2), and its reach in drawing units
    /// (0: by the model's size, [`ao_reach`]).
    pub ao: f32,
    pub ao_radius: f64,
    /// The sun's apparent size in degrees (0: sharp shadows), widening shadow edges with the
    /// distance from what casts them.
    pub sun_size: f32,
    /// The tone curve.
    pub tone: ToneMap,
}

/// The ambient occlusion's reach for a model whose bounding box has diagonal `diag`: an explicit
/// `radius` when set, else 2% of the diagonal.
pub fn ao_reach(radius: f64, diag: f64) -> f64 {
    if radius.is_finite() && radius > 0.0 {
        radius
    } else if diag.is_finite() && diag > 0.0 {
        diag * 0.02
    } else {
        1.0
    }
}

fn lin(c: cadaddy_doc::color::Rgb) -> [f32; 3] {
    let f = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    [f(c.0), f(c.1), f(c.2)]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// The sky for a sun `elevation` degrees up (below −6°: night).
pub fn sky(elevation: f64) -> Sky {
    let day = ((elevation as f32 + 6.0) / 16.0).clamp(0.0, 1.0);
    let low = (1.0 - (elevation as f32 / 25.0)).clamp(0.0, 1.0) * day;
    let zenith = mix3([0.008, 0.01, 0.03], [0.18, 0.32, 0.72], day);
    let horizon = mix3(mix3([0.01, 0.012, 0.03], [0.62, 0.70, 0.82], day), [0.95, 0.55, 0.30], low * 0.6);
    let ground = mix3([0.005, 0.005, 0.006], [0.20, 0.19, 0.17], day);
    Sky { zenith, horizon, ground }
}

/// The sun's colour (warmer near the horizon) and strength for an `elevation` degrees up.
fn sun_color(elevation: f64) -> [f32; 3] {
    let e = elevation as f32;
    let k = (e / 6.0).clamp(0.0, 1.0);
    let warm = (1.0 - e / 30.0).clamp(0.0, 1.0);
    scale3(mix3([1.0, 0.98, 0.94], [1.0, 0.62, 0.32], warm), k * SUN)
}

impl Lighting {
    /// The lighting of `space`: the drawing's sun and sky, its LIGHT objects that are on.
    pub fn of(d: &Drawing, space: &Space) -> Lighting {
        let sun = d.sun.clone().sane();
        let (el, _) = sun.elevation_azimuth();
        let sun_on = sun.on && el > -1.0;
        let unit_m = cadaddy_doc::material::metres_per_unit(d.header.i64("INSUNITS", 0), d.header.i64("MEASUREMENT", 0) == 1);
        let mut lights = Vec::new();
        if let Some(store) = d.space(space) {
            for e in store.iter() {
                let EntityKind::Light(l) = &e.kind else { continue };
                if !l.on || !d.is_visible(e) || lights.len() >= MAX_LIGHTS {
                    continue;
                }
                let l = l.clone().sane();
                let layer = d.layer(&e.common.layer).map(|x| x.color).unwrap_or(cadaddy_doc::color::Color::Index(7));
                let rgb = lin(e.common.color.resolve(layer, cadaddy_doc::color::Color::Index(7)));
                let k = match l.kind {
                    LightKind::Distant => l.intensity as f32 * SUN,
                    _ => l.intensity as f32 * CANDELA,
                };
                let half = |deg: f64| (deg.to_radians() / 2.0).cos() as f32;
                lights.push(PointLight {
                    pos: if l.kind == LightKind::Distant { -l.direction() } else { l.position },
                    dir: l.direction(),
                    rgb: scale3(rgb, k),
                    kind: l.kind,
                    attenuation: l.attenuation,
                    cos_inner: half(l.hotspot),
                    cos_outer: half(l.falloff).min(half(l.hotspot) - 1e-4),
                });
            }
        }
        let intensity = sun.intensity as f32;
        let mut sky = sky(el);
        sky.zenith = scale3(sky.zenith, intensity);
        sky.horizon = scale3(sky.horizon, intensity);
        sky.ground = scale3(sky.ground, intensity);
        if !sun.on {
            // Sun off: a dim neutral ambient, so lights carry the scene.
            sky = Sky { zenith: [0.03, 0.03, 0.035], horizon: [0.03, 0.03, 0.035], ground: [0.015, 0.015, 0.015] };
        }
        let look = sun.look.clone().sane();
        Lighting {
            sun: sun_on.then(|| (sun.direction(), scale3(sun_color(el), intensity))),
            sky,
            lights,
            exposure: 2f32.powf(sun.exposure as f32),
            unit_m: if unit_m.is_finite() && unit_m > 0.0 { unit_m } else { 1.0 },
            env_map: None,
            fog: None,
            background: Background::Sky,
            white_balance: [1.0, 1.0, 1.0],
            ao: look.ambient_occlusion as f32,
            ao_radius: look.ao_radius,
            sun_size: look.soft_shadows as f32,
            tone: look.tone_map,
        }
    }

    /// Linear light `c` as display bytes: white balance, exposure and the tone curve.
    pub fn display(&self, c: [f32; 3]) -> [u8; 3] {
        let wb = self.white_balance;
        tonemap_with([c[0] * wb[0], c[1] * wb[1], c[2] * wb[2]], self.exposure, self.tone)
    }
}

/// A surface's material as the shading needs it (linear base colour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub base: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a.dot(b) as f32
}

/// The microfacet BRDF times cos θ for light from `l` seen from `v` on a surface with normal `n`
/// (all unit): GGX distribution, height-correlated Smith visibility, Schlick Fresnel; Lambert
/// diffuse for the non-metal part.
pub fn brdf_cos(n: Vec3, v: Vec3, l: Vec3, s: &Surface) -> [f32; 3] {
    let nl = dot(n, l);
    if nl <= 0.0 {
        return [0.0; 3];
    }
    let nv = dot(n, v).max(1e-4);
    let h = (l + v).normalized();
    let nh = dot(n, h).max(0.0);
    let vh = dot(v, h).max(0.0);
    let a = (s.roughness.clamp(0.04, 1.0)).powi(2);
    let a2 = a * a;
    let d = a2 / (std::f32::consts::PI * (nh * nh * (a2 - 1.0) + 1.0).powi(2)).max(1e-8);
    let vis = 0.5 / (nl * (nv * nv * (1.0 - a2) + a2).sqrt() + nv * (nl * nl * (1.0 - a2) + a2).sqrt()).max(1e-8);
    let f0 = mix3([0.04; 3], s.base, s.metallic);
    let fw = (1.0 - vh).powi(5);
    let f = [f0[0] + (1.0 - f0[0]) * fw, f0[1] + (1.0 - f0[1]) * fw, f0[2] + (1.0 - f0[2]) * fw];
    let kd = 1.0 - s.metallic;
    let mut out = [0.0; 3];
    for i in 0..3 {
        let diffuse = (1.0 - f[i]) * kd * s.base[i] / std::f32::consts::PI;
        out[i] = (diffuse + d * vis * f[i]) * nl;
    }
    out
}

/// What the sky (or environment map) lights a surface with (its normal `n`): the sky above,
/// the ground below, by how much of each the surface faces; diffuse, plus a rough specular
/// share for metals.
pub fn ambient(lit: &Lighting, n: Vec3, s: &Surface) -> [f32; 3] {
    if let Some(env) = &lit.env_map {
        // Sample the environment map in the normal's direction for diffuse irradiance.
        let env_color = env.sample(n);
        return surround(env_color, s);
    }
    let up = (n.z as f32).clamp(-1.0, 1.0);
    let sky_part = mix3(lit.sky.horizon, lit.sky.zenith, up.max(0.0));
    let env = mix3(lit.sky.ground, sky_part, up * 0.5 + 0.5);
    // Light bounced off the surroundings: the sky's colour softened towards grey.
    let grey = (env[0] + env[1] + env[2]) / 3.0;
    surround(mix3(env, [grey; 3], 0.5), s)
}

/// What light `env` arriving from all around (radiance: baked, or the sky's) does on surface
/// `s`: diffuse, plus a rough specular share for metals.
pub fn surround(env: [f32; 3], s: &Surface) -> [f32; 3] {
    let kd = (1.0 - s.metallic) * 0.96;
    let spec = mix3([0.04; 3], s.base, s.metallic);
    let gloss = (1.0 - s.roughness).clamp(0.0, 1.0) * 0.5;
    [0, 1, 2].map(|i| env[i] * (s.base[i] * kd + spec[i] * (0.3 + gloss)))
}

/// The light leaving point `p` (normal `n`, `v` towards the viewer) of surface `s`: the sky, the
/// sun (`sun_shadow` 0 = in its shadow … 1 = lit) and every light. Linear, before exposure.
pub fn shade(lit: &Lighting, p: Vec3, n: Vec3, v: Vec3, s: &Surface, sun_shadow: f32) -> [f32; 3] {
    shade_baked(lit, p, n, v, s, sun_shadow, None)
}

/// [`shade`] with the light baked onto the surface (LIGHTBAKE) in place of the sky's ambient.
pub fn shade_baked(lit: &Lighting, p: Vec3, n: Vec3, v: Vec3, s: &Surface, sun_shadow: f32, baked: Option<[f32; 3]>) -> [f32; 3] {
    shade_occluded(lit, p, n, v, s, sun_shadow, baked, 1.0)
}

/// [`shade_baked`] with the light arriving from all around (sky or baked) scaled by `open`, the
/// ambient occlusion (0 enclosed … 1 open).
#[allow(clippy::too_many_arguments)]
pub fn shade_occluded(lit: &Lighting, p: Vec3, n: Vec3, v: Vec3, s: &Surface, sun_shadow: f32, baked: Option<[f32; 3]>, open: f32) -> [f32; 3] {
    let open = if open.is_finite() { open.clamp(0.0, 1.0) } else { 1.0 };
    let mut c = match baked {
        Some(env) => surround(env, s),
        None => ambient(lit, n, s),
    }
    .map(|x| x * open);
    let mut add = |rgb: [f32; 3], f: [f32; 3]| {
        for i in 0..3 {
            c[i] += rgb[i] * f[i];
        }
    };
    if let Some((dir, rgb)) = lit.sun
        && sun_shadow > 0.0
    {
        add(scale3(rgb, sun_shadow), brdf_cos(n, v, dir, s));
    }
    for l in &lit.lights {
        if let Some((ldir, rgb, _)) = incoming(l, p, lit.unit_m) {
            add(rgb, brdf_cos(n, v, ldir, s));
        }
    }
    c
}

/// The light `l` sends to point `p`: the direction towards it, its colour times irradiance there
/// (falloff and spot cone applied), and its distance (infinite for a distant light). `None` where
/// it gives nothing.
pub fn incoming(l: &PointLight, p: Vec3, unit_m: f64) -> Option<(Vec3, [f32; 3], f64)> {
    match l.kind {
        LightKind::Distant => Some((l.pos, l.rgb, f64::INFINITY)),
        LightKind::Point | LightKind::Spot => {
            let to = l.pos - p;
            let dist = to.len();
            if !(dist.is_finite() && dist > 1e-9) {
                return None;
            }
            let ldir = to * (1.0 / dist);
            let dm = (dist * unit_m).max(0.05) as f32;
            let fall = match l.attenuation {
                Attenuation::None => 1.0,
                Attenuation::InverseLinear => 1.0 / dm,
                Attenuation::InverseSquare => 1.0 / (dm * dm),
            };
            let cone = if l.kind == LightKind::Spot {
                let c = dot(-ldir, l.dir);
                ((c - l.cos_outer) / (l.cos_inner - l.cos_outer).max(1e-4)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            (fall * cone > 0.0).then(|| (ldir, scale3(l.rgb, fall * cone), dist))
        }
    }
}

/// The environment in direction `d`: the HDR map when set, else the analytic sky without the
/// sun's disc. For path-traced rays that leave the scene.
pub fn env_or_sky(lit: &Lighting, d: Vec3) -> [f32; 3] {
    if let Some(env) = &lit.env_map {
        return env.sample(d);
    }
    sky_only(lit, d)
}

/// The sky seen in direction `d` without the sun's disc (rays that leave the scene: the sun
/// itself is sampled directly), linear.
pub fn sky_only(lit: &Lighting, d: Vec3) -> [f32; 3] {
    let up = (d.z as f32).clamp(-1.0, 1.0);
    if up < 0.0 {
        return mix3(lit.sky.horizon, lit.sky.ground, (-up * 4.0).min(1.0));
    }
    mix3(lit.sky.horizon, lit.sky.zenith, up.powf(0.6))
}

/// A filmic tone curve (the ACES fitted curve), exposure applied, then sRGB-encoded bytes.
pub fn tonemap(c: [f32; 3], exposure: f32) -> [u8; 3] {
    tonemap_with(c, exposure, ToneMap::Filmic)
}

/// Linear light `c` through exposure and the tone curve `op` (0 … 1, linear). The GPU shader's
/// `tone_curve` is the same.
pub fn tone_curve(c: [f32; 3], exposure: f32, op: ToneMap) -> [f32; 3] {
    let x = c.map(|v| if v.is_finite() { (v * exposure * 0.8).max(0.0) } else { 0.0 });
    match op {
        ToneMap::Filmic => x.map(|x| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)),
        // Extended Reinhard, white at 4.
        ToneMap::Reinhard => x.map(|x| (x * (1.0 + x / 16.0) / (1.0 + x)).clamp(0.0, 1.0)),
        ToneMap::Neutral => neutral(x),
        ToneMap::Linear => x.map(|x| x.clamp(0.0, 1.0)),
    }
}

/// A neutral curve: colours pass unchanged below a knee; above it the brightest channel is
/// compressed towards white and the colour desaturates as it does.
fn neutral(c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.76;
    const DESATURATE: f32 = 0.15;
    let lo = c[0].min(c[1]).min(c[2]);
    let offset = if lo < 0.08 { lo - 6.25 * lo * lo } else { 0.04 };
    let c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c.map(|v| v.clamp(0.0, 1.0));
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    let c = c.map(|v| v * new_peak / peak);
    let g = 1.0 - 1.0 / (DESATURATE * (peak - new_peak) + 1.0);
    c.map(|v| (v + (new_peak - v) * g).clamp(0.0, 1.0))
}

/// [`tone_curve`], then sRGB-encoded bytes.
pub fn tonemap_with(c: [f32; 3], exposure: f32, op: ToneMap) -> [u8; 3] {
    tone_curve(c, exposure, op).map(|y| {
        let s = if y <= 0.003_130_8 { y * 12.92 } else { 1.055 * y.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    })
}

/// Apply fog to a shaded colour: blend towards the fog colour by the fog factor at `depth`
/// (0 = at the viewer, 1 = far plane).
pub fn apply_fog(c: [f32; 3], depth_frac: f32, fog: &FogParams) -> [f32; 3] {
    let t = ((depth_frac - fog.near) / (fog.far - fog.near).max(1e-6)).clamp(0.0, 1.0);
    mix3(c, fog.color, t)
}

/// Tone map with white balance applied: the balance multiplier scales linear light before the
/// tone curve.
pub fn tonemap_wb(c: [f32; 3], exposure: f32, wb: [f32; 3]) -> [u8; 3] {
    tonemap([c[0] * wb[0], c[1] * wb[1], c[2] * wb[2]], exposure)
}

/// Compute a white-balance RGB multiplier from colour temperature (kelvin) and tint.
/// Approximates the ratio of the Planckian locus colour at `temperature` to D65 (6500 K).
pub fn white_balance_rgb(temperature: f64, tint: f64) -> [f32; 3] {
    let t = temperature.clamp(1000.0, 40000.0);
    // Planckian ratio: lower K → more red, less blue; higher K → less red, more blue.
    // Using a log-ratio that gives smooth, perceptually reasonable shifts.
    let ratio = (6500.0 / t).ln();
    let r = (1.0 + ratio * 0.8).clamp(0.3, 3.0) as f32;
    let b = (1.0 - ratio * 0.8).clamp(0.3, 3.0) as f32;
    // Tint shifts green vs magenta.
    let g = (1.0 - tint.clamp(-1.0, 1.0) * 0.3).clamp(0.3, 3.0) as f32;
    [r, g, b]
}

/// The rendered view's background at image fraction `t` (0 bottom … 1 top), respecting the
/// background setting.
pub fn view_background(lit: &Lighting, t: f32) -> [f32; 3] {
    match &lit.background {
        Background::Sky => background(lit, t),
        Background::Solid(c) => *c,
        Background::Gradient { bottom, top } => mix3(*bottom, *top, t.clamp(0.0, 1.0)),
        Background::Image(raster) => {
            // Sample the image at this vertical position, horizontally centred.
            let y = ((1.0 - t) * raster.height as f32).clamp(0.0, (raster.height.saturating_sub(1)) as f32) as u32;
            let x = raster.width / 2;
            match raster.pixel(x, y) {
                Some(p) => {
                    let f = |v: u8| (f32::from(v) / 255.0).powf(2.2);
                    [f(p[0]), f(p[1]), f(p[2])]
                }
                None => background(lit, t),
            }
        }
    }
}

/// The sky seen in direction `d` (a rendered view's background), linear.
pub fn sky_color(lit: &Lighting, d: Vec3) -> [f32; 3] {
    sky_at(lit, d)
}

/// A rendered view's background at image height `t` (0 bottom … 1 top): horizon to zenith.
pub fn background(lit: &Lighting, t: f32) -> [f32; 3] {
    mix3(lit.sky.horizon, lit.sky.zenith, t.clamp(0.0, 1.0).powf(0.8))
}

fn sky_at(lit: &Lighting, d: Vec3) -> [f32; 3] {
    let up = (d.z as f32).clamp(-1.0, 1.0);
    if up < 0.0 {
        return mix3(lit.sky.horizon, lit.sky.ground, (-up * 4.0).min(1.0));
    }
    let mut c = mix3(lit.sky.horizon, lit.sky.zenith, up.powf(0.6));
    if let Some((sd, rgb)) = lit.sun {
        let k = dot(d, sd).max(0.0).powi(800) * 20.0 + dot(d, sd).max(0.0).powi(8) * 0.15;
        c = [c[0] + rgb[0] * k, c[1] + rgb[1] * k, c[2] + rgb[2] * k];
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::lighting::{Light, Sun};
    use cadaddy_doc::{Common, Drawing, Space};

    fn grey(r: f32) -> Surface {
        Surface { base: [0.5, 0.5, 0.5], roughness: r, metallic: 0.0 }
    }

    #[test]
    fn energy_and_direction_behave() {
        let n = Vec3::Z;
        let v = Vec3::new(0.0, -0.6, 0.8).normalized();
        // Light from behind the surface gives nothing; head on gives the most.
        assert_eq!(brdf_cos(n, v, -Vec3::Z, &grey(0.5)), [0.0; 3]);
        let head = brdf_cos(n, v, Vec3::Z, &grey(0.5))[0];
        let grazing = brdf_cos(n, v, Vec3::new(0.0, 0.95, 0.31).normalized(), &grey(0.5))[0];
        assert!(head > grazing && head > 0.0);
        // A white diffuse surface never reflects more than it receives (within the microfacet
        // term's highlight).
        let white = Surface { base: [1.0; 3], roughness: 1.0, metallic: 0.0 };
        assert!(brdf_cos(n, n, n, &white)[0] < 0.6);
        // A smooth surface has a sharper highlight than a rough one at the mirror direction.
        let mirror = Vec3::new(0.0, 0.6, 0.8).normalized();
        assert!(brdf_cos(n, v, mirror, &grey(0.1))[0] > brdf_cos(n, v, mirror, &grey(0.9))[0]);
        // Tone curve: black stays black, bright saturates, exposure brightens.
        assert_eq!(tonemap([0.0; 3], 1.0), [0, 0, 0]);
        assert_eq!(tonemap([100.0; 3], 1.0), [255, 255, 255]);
        assert!(tonemap([0.2; 3], 2.0)[0] > tonemap([0.2; 3], 1.0)[0]);
    }

    #[test]
    fn the_drawings_sun_and_lights() {
        let mut d = Drawing::new_metric();
        let day = Lighting::of(&d, &Space::Model);
        assert!(day.sun.is_some() && day.lights.is_empty());
        // At night the sun is gone and the sky dark.
        d.sun = Sun { minutes: 0, ..Sun::default() };
        let night = Lighting::of(&d, &Space::Model);
        assert!(night.sun.is_none() && night.sky.zenith[2] < 0.05);
        // A point light 2 m above a floor lights it; switched off it doesn't.
        d.sun.on = false;
        let lamp = Light { position: Vec3::new(0.0, 0.0, 2000.0), ..Light::default() };
        d.add(&Space::Model, Common::default(), EntityKind::Light(lamp.clone())).unwrap();
        let lit = Lighting::of(&d, &Space::Model);
        assert_eq!(lit.lights.len(), 1);
        let below = shade(&lit, Vec3::ZERO, Vec3::Z, Vec3::Z, &grey(0.6), 1.0)[0];
        let aside = shade(&lit, Vec3::new(6000.0, 0.0, 0.0), Vec3::Z, Vec3::Z, &grey(0.6), 1.0)[0];
        assert!(below > aside * 3.0, "{below} {aside}");
        // A spot lights inside its cone only.
        let spot = Light {
            kind: LightKind::Spot,
            position: Vec3::new(0.0, 0.0, 2000.0),
            target: Vec3::ZERO,
            hotspot: 20.0,
            falloff: 30.0,
            ..Light::default()
        };
        let mut e = Drawing::new_metric();
        e.sun.on = false;
        e.add(&Space::Model, Common::default(), EntityKind::Light(spot)).unwrap();
        let sl = Lighting::of(&e, &Space::Model);
        let inside = shade(&sl, Vec3::ZERO, Vec3::Z, Vec3::Z, &grey(0.6), 1.0)[0];
        let outside = shade(&sl, Vec3::new(3000.0, 0.0, 0.0), Vec3::Z, Vec3::Z, &grey(0.6), 1.0)[0];
        assert!(inside > outside * 5.0, "{inside} {outside}");
        // The sun's shadow takes away its share.
        let sunny = Lighting::of(&Drawing::new_metric(), &Space::Model);
        let up = Vec3::Z;
        assert!(shade(&sunny, Vec3::ZERO, up, up, &grey(0.6), 1.0)[0] > shade(&sunny, Vec3::ZERO, up, up, &grey(0.6), 0.0)[0]);
    }

    #[test]
    fn fog_blends_towards_the_fog_colour() {
        let fog = FogParams { color: [1.0, 1.0, 1.0], near: 0.2, far: 0.8 };
        // Before the fog starts: no fog.
        let c = apply_fog([0.5, 0.0, 0.0], 0.1, &fog);
        assert!((c[0] - 0.5).abs() < 1e-4 && c[1].abs() < 1e-4, "{c:?}");
        // At full fog distance: all fog colour.
        let c = apply_fog([0.5, 0.0, 0.0], 1.0, &fog);
        assert!((c[0] - 1.0).abs() < 1e-4 && (c[1] - 1.0).abs() < 1e-4, "{c:?}");
        // At the midpoint: half blended.
        let c = apply_fog([0.0, 0.0, 0.0], 0.5, &fog);
        assert!((c[0] - 0.5).abs() < 0.01, "{c:?}");
    }

    #[test]
    fn white_balance_shifts_colour() {
        // Daylight (6500K): neutral.
        let wb = white_balance_rgb(6500.0, 0.0);
        assert!((wb[0] - 1.0).abs() < 0.1 && (wb[2] - 1.0).abs() < 0.1, "{wb:?}");
        // Warm (3000K): more red than blue.
        let warm = white_balance_rgb(3000.0, 0.0);
        assert!(warm[0] > warm[2], "warm: {warm:?}");
        // Cool (10000K): more blue than red.
        let cool = white_balance_rgb(10000.0, 0.0);
        assert!(cool[2] > cool[0], "cool: {cool:?}");
        // Tint shifts green.
        let tinted = white_balance_rgb(6500.0, 0.5);
        assert!(tinted[1] < 1.0, "tinted: {tinted:?}");
    }

    #[test]
    fn tone_curves_run_from_black_to_white_and_differ() {
        for op in ToneMap::ALL {
            assert_eq!(tonemap_with([0.0; 3], 1.0, op), [0, 0, 0], "{op:?}");
            assert_eq!(tonemap_with([1e6; 3], 1.0, op), [255, 255, 255], "{op:?}");
            assert_eq!(tonemap_with([f32::NAN, f32::INFINITY, -1.0], 1.0, op)[0], 0, "{op:?}");
            // Rising.
            let mut last = 0u8;
            for k in 0..200 {
                let v = tonemap_with([k as f32 * 0.02; 3], 1.0, op)[0];
                assert!(v >= last, "{op:?} at {k}");
                last = v;
            }
        }
        let mid = ToneMap::ALL.map(|op| tonemap_with([0.4, 0.2, 0.1], 1.0, op));
        for (i, a) in mid.iter().enumerate() {
            for b in mid.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        // The neutral curve keeps a mid colour as it is (bar a small toe offset); linear clips.
        let n = tone_curve([0.5, 0.3, 0.2], 1.25, ToneMap::Neutral);
        assert!((n[0] - 0.46).abs() < 0.01 && (n[1] - 0.26).abs() < 0.01, "{n:?}");
        assert_eq!(tone_curve([2.0, 0.5, 0.0], 1.25, ToneMap::Linear), [1.0, 0.5, 0.0]);
        // Filmic stays the default and is what `tonemap` gives.
        assert_eq!(tonemap([0.3, 0.2, 0.1], 1.0), tonemap_with([0.3, 0.2, 0.1], 1.0, ToneMap::Filmic));
        assert_eq!(ToneMap::default(), ToneMap::Filmic);
    }

    #[test]
    fn display_applies_white_balance_exposure_and_curve() {
        let mut lit = Lighting::of(&Drawing::new_metric(), &Space::Model);
        assert_eq!(lit.display([0.3; 3]), tonemap([0.3; 3], lit.exposure));
        lit.tone = ToneMap::Linear;
        lit.white_balance = [2.0, 1.0, 1.0];
        let c = lit.display([0.3; 3]);
        assert!(c[0] > c[1], "{c:?}");
        assert_eq!(ao_reach(0.0, 100.0), 2.0);
        assert_eq!(ao_reach(5.0, 100.0), 5.0);
        assert_eq!(ao_reach(f64::NAN, f64::NAN), 1.0);
    }

    #[test]
    fn tonemap_with_white_balance() {
        let neutral = tonemap([0.5, 0.5, 0.5], 1.0);
        let balanced = tonemap_wb([0.5, 0.5, 0.5], 1.0, [1.5, 1.0, 0.7]);
        // Red should be brighter, blue dimmer.
        assert!(balanced[0] > neutral[0] && balanced[2] < neutral[2], "{balanced:?} vs {neutral:?}");
    }

    #[test]
    fn view_background_respects_settings() {
        let mut lit = Lighting::of(&Drawing::new_metric(), &Space::Model);
        // Sky default: the top (zenith) should differ from the bottom (horizon).
        let sky_top = view_background(&lit, 1.0);
        let sky_bot = view_background(&lit, 0.0);
        assert!(sky_top != sky_bot, "sky varies vertically: top {sky_top:?} bot {sky_bot:?}");
        // Solid background.
        lit.background = Background::Solid([0.1, 0.2, 0.3]);
        let solid = view_background(&lit, 0.5);
        assert!((solid[0] - 0.1).abs() < 1e-4 && (solid[1] - 0.2).abs() < 1e-4, "{solid:?}");
        // Gradient background.
        lit.background = Background::Gradient { bottom: [0.0, 0.0, 0.0], top: [1.0, 1.0, 1.0] };
        let mid = view_background(&lit, 0.5);
        assert!((mid[0] - 0.5).abs() < 0.01, "{mid:?}");
        let top = view_background(&lit, 1.0);
        assert!((top[0] - 1.0).abs() < 0.01, "{top:?}");
    }

    #[test]
    fn environment_map_ambient_lighting() {
        // A bright red environment map: ambient should be reddish.
        let hdr = crate::hdr::HdrImage { width: 4, height: 2, data: vec![[5.0f32, 0.1, 0.1]; 8].into_iter().flatten().collect() };
        let mut lit = Lighting::of(&Drawing::new_metric(), &Space::Model);
        lit.env_map = Some(std::sync::Arc::new(hdr));
        let c = ambient(&lit, Vec3::Z, &grey(0.6));
        assert!(c[0] > c[1] * 3.0, "env map makes ambient red: {c:?}");
    }
}
