//! What the GPU can do for the 3D views (the shaded Model tab and Walk), and how they step down
//! on a weak or virtual GPU instead of failing or drawing black:
//!
//! - **MSAA**: 4× on a discrete GPU when the adapter supports it for the colour and depth formats;
//!   2× on an integrated one (it shares the system's memory and bus, and 4× colour and depth at a
//!   laptop's resolution is a large share of its frame), else none;
//! - **texture size**: render targets are kept within the device's largest 2D texture;
//! - **memory budget**: the render targets (colour, depth, the multisampled copies) are kept
//!   within a budget for the kind of GPU (discrete 512 MB, integrated 256 MB, virtual or
//!   software 64 MB); a view that wouldn't fit is drawn smaller and scaled up;
//! - **lighting quality**: Full on a discrete GPU; Balanced on an integrated one (hard shadows,
//!   no ambient occlusion: two of the three scene passes and the 32 shadow and 16 occlusion
//!   samples a pixel go); Performance (no shadow pass, no point lights) when asked for or when
//!   the view stays slow (see `shaded`);
//! - **the CPU**: a software adapter, a GPU error or device loss, or `cpu: true` draws the 3D
//!   views with the CPU rasterizers (the shaded view's, and Walk's own).
//!
//! The limits can be forced (tests, and machines that misreport): `ui.gpu {msaa?, maxTexture?,
//! budgetMb?, quality?, cpu?, dynamicScale?, reset?}`, or
//! `CADADDY_GPU_LIMITS="msaa=1,texture=1024,budget=64,quality=performance,cpu,dynscale=50"` at
//! start. `dynamicScale` (0.5–1, or `null` to measure again; `dynscale` in percent) is the scale
//! the 2D canvas and the 3D views draw a moving view at instead of the measured one (`crate::dynres`). Anything stepped down is said once, in the status line.

use egui_wgpu::wgpu;
use serde_json::{Value, json};

/// Samples the 3D views ask for on a discrete GPU.
pub const MSAA_WANTED: u32 = 4;
/// Samples they ask for on an integrated GPU.
pub const MSAA_INTEGRATED: u32 = 2;

/// The samples per pixel the 3D views start at on a kind of adapter, given the counts it
/// supports for the colour and depth formats (`supported(n)`): 4× on a discrete GPU, 2× on an
/// integrated one, none on a software or virtual one, each falling to the next count it has.
pub fn msaa_for(kind: wgpu::DeviceType, software: bool, supported: impl Fn(u32) -> bool) -> u32 {
    let want = match kind {
        _ if software => 1,
        wgpu::DeviceType::IntegratedGpu => MSAA_INTEGRATED,
        _ => MSAA_WANTED,
    };
    [4, 2].into_iter().find(|&n| n <= want && supported(n)).unwrap_or(1)
}

/// How much lighting the 3D views draw (what each costs is in [`Quality::passes`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Quality {
    /// No shadow pass, no ambient occlusion, no point lights: sun and sky only.
    Performance,
    /// Shadows with hard edges (9 samples a pixel, not 32); no ambient occlusion.
    Balanced,
    /// Soft shadows and ambient occlusion, as the drawing's lighting asks.
    #[default]
    Full,
}

impl Quality {
    pub fn name(self) -> &'static str {
        match self {
            Quality::Performance => "performance",
            Quality::Balanced => "balanced",
            Quality::Full => "full",
        }
    }

    pub fn from_name(n: &str) -> Option<Quality> {
        [Quality::Performance, Quality::Balanced, Quality::Full].into_iter().find(|q| q.name().eq_ignore_ascii_case(n.trim()))
    }

    /// The next level down, if there is one.
    pub fn lower(self) -> Option<Quality> {
        match self {
            Quality::Full => Some(Quality::Balanced),
            Quality::Balanced => Some(Quality::Performance),
            Quality::Performance => None,
        }
    }

    /// Soft shadows and ambient occlusion are drawn.
    pub fn soft(self) -> bool {
        self == Quality::Full
    }

    /// The sun's shadow map is drawn and read.
    pub fn shadows(self) -> bool {
        self != Quality::Performance
    }

    /// Point and spot lights are added up at each pixel.
    pub fn lights(self) -> bool {
        self != Quality::Performance
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GpuCaps {
    pub adapter: String,
    pub backend: String,
    /// A software or virtual adapter (WARP, llvmpipe, SwiftShader, a VM's display driver).
    pub software: bool,
    pub max_texture: u32,
    /// Samples per pixel for the 3D views: 4, 2 or 1.
    pub msaa: u32,
    /// The sample counts above 1 the adapter supports for the 3D views' formats (what `msaa`
    /// may be forced to).
    pub msaa_supported: Vec<u32>,
    /// Memory the 3D views' render targets may take, megabytes.
    pub budget_mb: u32,
    /// Draw the 3D views on the CPU.
    pub cpu: bool,
    /// The lighting the 3D views draw, and whether it was asked for (`ui.gpu`, the environment):
    /// a level asked for is never stepped down for a slow view.
    pub quality: Quality,
    pub quality_forced: bool,
    /// The scale the 2D canvas and the 3D views draw a moving view at, instead of the measured one.
    pub dynamic_scale: Option<f32>,
    /// Why, when something was stepped down.
    pub notes: Vec<String>,
}

impl Default for GpuCaps {
    /// No GPU: the CPU draws.
    fn default() -> Self {
        GpuCaps {
            adapter: "none".into(),
            backend: "none".into(),
            software: true,
            max_texture: 8192,
            msaa: 1,
            msaa_supported: Vec::new(),
            budget_mb: 64,
            cpu: true,
            quality: Quality::Full,
            quality_forced: false,
            dynamic_scale: None,
            notes: Vec::new(),
        }
    }
}

/// Names of adapters that are software or virtual.
fn soft_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["llvmpipe", "lavapipe", "softpipe", "swiftshader", "microsoft basic render", "warp", "virgl", "vmware svga", "virtualbox", "parallels display"]
        .iter()
        .any(|s| n.contains(s))
}

impl Quality {
    /// The level a kind of adapter starts at: an integrated GPU (it shares the system's memory
    /// and its bus) the balanced level, anything else everything. A software adapter draws
    /// everything too: what it draws is checked against the CPU's pictures, and nobody waits on
    /// its speed.
    pub fn of(kind: wgpu::DeviceType, software: bool) -> Quality {
        if kind == wgpu::DeviceType::IntegratedGpu && !software { Quality::Balanced } else { Quality::Full }
    }
}

impl GpuCaps {
    /// From the app's wgpu device.
    pub fn detect(rs: &egui_wgpu::RenderState) -> GpuCaps {
        let info = rs.adapter.get_info();
        let limits = rs.device.limits();
        // The formats the shaded view and Walk draw into. Without the device's adapter-specific
        // format features only the counts every WebGPU device has (1 and 4) may be asked for.
        let adapter_specific = rs.device.features().contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
        let samples = |format: wgpu::TextureFormat, n: u32| rs.adapter.get_texture_format_features(format).flags.sample_count_supported(n);
        let supported = |n: u32| {
            (n == 4 || adapter_specific)
                && [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::TextureFormat::Depth32Float]
                    .iter()
                    .all(|f| samples(*f, n))
        };
        let msaa_supported: Vec<u32> = [2, 4].into_iter().filter(|&n| supported(n)).collect();
        let msaa_ok = !msaa_supported.is_empty();
        let software = info.device_type == wgpu::DeviceType::Cpu || info.device_type == wgpu::DeviceType::VirtualGpu || soft_name(&info.name);
        let budget_mb = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => 512,
            wgpu::DeviceType::IntegratedGpu => 256,
            _ => 64,
        };
        let mut c = GpuCaps {
            adapter: info.name.clone(),
            backend: format!("{:?}", info.backend),
            software,
            max_texture: limits.max_texture_dimension_2d.max(1),
            msaa: msaa_for(info.device_type, software, |n| msaa_supported.contains(&n)),
            msaa_supported,
            budget_mb: if software { 64 } else { budget_mb },
            cpu: false,
            quality: Quality::of(info.device_type, software),
            quality_forced: false,
            dynamic_scale: None,
            notes: Vec::new(),
        };
        if c.quality == Quality::Balanced {
            c.notes.push(format!("{} is integrated graphics: 3D views use hard shadows and no ambient occlusion", info.name));
        }
        if !msaa_ok {
            c.notes.push("no multisampling: edges are drawn without smoothing".into());
        } else if !software && c.msaa < MSAA_WANTED {
            c.notes.push(format!("{} is integrated graphics: 3D edges are smoothed at {}× (not {MSAA_WANTED}×)", info.name, c.msaa));
        }
        if software {
            c.notes.push(format!("{} is a software or virtual GPU: 3D views are simplified", info.name));
        }
        if let Ok(spec) = std::env::var("CADADDY_GPU_LIMITS") {
            c.force(&from_spec(&spec));
        }
        c
    }

    /// Force limits: `{msaa?, maxTexture?, budgetMb?, cpu?}`.
    pub fn force(&mut self, p: &Value) {
        if let Some(m) = p.get("msaa").and_then(Value::as_u64) {
            // The largest count the adapter has that is no more than asked for.
            self.msaa = [4u32, 2].into_iter().find(|&n| u64::from(n) <= m && self.msaa_supported.contains(&n)).unwrap_or(1);
            self.notes.push(format!("multisampling forced to {}×", self.msaa));
        }
        if let Some(t) = p.get("maxTexture").and_then(Value::as_u64) {
            self.max_texture = u32::try_from(t).unwrap_or(u32::MAX).clamp(64, self.max_texture.max(64));
            self.notes.push(format!("textures limited to {} px", self.max_texture));
        }
        if let Some(b) = p.get("budgetMb").and_then(Value::as_u64) {
            self.budget_mb = u32::try_from(b).unwrap_or(u32::MAX).clamp(1, 65_536);
            self.notes.push(format!("3D view memory limited to {} MB", self.budget_mb));
        }
        if let Some(q) = p.get("quality").and_then(Value::as_str).and_then(Quality::from_name) {
            self.quality = q;
            self.quality_forced = true;
            self.notes.push(format!("3D lighting set to {}", q.name()));
        }
        match p.get("dynamicScale") {
            Some(Value::Null) => self.dynamic_scale = None,
            Some(v) => {
                if let Some(s) = v.as_f64().filter(|s| s.is_finite()) {
                    let s = (s as f32).clamp(crate::dynres::floor(), 1.0);
                    self.dynamic_scale = Some(s);
                    self.notes.push(format!("2D and 3D views drawn at {} % while moving", (s * 100.0).round()));
                }
            }
            None => {}
        }
        if let Some(c) = p.get("cpu").and_then(Value::as_bool) {
            self.cpu = c;
            if c {
                self.notes.push("3D views drawn by the CPU".into());
            }
        }
    }

    /// The size to draw a `w` × `h` view at: within the largest texture and the memory budget
    /// (colour and depth, multisampled, plus the resolved image), keeping its shape.
    pub fn target_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (mut w, mut h) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let side = f64::from(self.max_texture.max(1));
        let fit = (side / w).min(side / h).min(1.0);
        w *= fit;
        h *= fit;
        // Bytes a pixel takes: colour + depth per sample, and the resolved colour.
        let per_px = f64::from(self.msaa) * 8.0 + 4.0;
        let budget = f64::from(self.budget_mb) * 1024.0 * 1024.0;
        let need = w * h * per_px;
        if need > budget {
            let k = (budget / need).sqrt();
            w *= k;
            h *= k;
        }
        ((w.floor() as u32).max(1), (h.floor() as u32).max(1))
    }

    /// What `ui.gpu` reports.
    pub fn json(&self) -> Value {
        json!({
            "adapter": self.adapter, "backend": self.backend, "software": self.software,
            "maxTexture": self.max_texture, "msaa": self.msaa, "msaaSupported": self.msaa_supported, "budgetMb": self.budget_mb, "cpu": self.cpu,
            "quality": self.quality.name(), "dynamicScale": self.dynamic_scale, "notes": self.notes,
        })
    }

    /// The status line's notice, when something was stepped down.
    pub fn notice(&self) -> Option<String> {
        (!self.notes.is_empty()).then(|| format!("3D graphics: {}.", self.notes.join("; ")))
    }
}

/// `msaa=1,texture=1024,budget=64,cpu` as `force`'s parameters.
pub fn from_spec(spec: &str) -> Value {
    let mut v = json!({});
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, val) = part.split_once('=').map_or((part, ""), |(k, v)| (k.trim(), v.trim()));
        let n = val.parse::<u64>().ok();
        match (k.to_ascii_lowercase().as_str(), n) {
            ("msaa", Some(n)) => v["msaa"] = json!(n),
            ("texture" | "maxtexture", Some(n)) => v["maxTexture"] = json!(n),
            ("budget" | "budgetmb", Some(n)) => v["budgetMb"] = json!(n),
            ("quality", _) if Quality::from_name(val).is_some() => v["quality"] = json!(val.to_ascii_lowercase()),
            ("cpu" | "software", _) => v["cpu"] = json!(val != "0" && val != "false"),
            ("dynscale" | "dynamicscale", Some(n)) => v["dynamicScale"] = json!(n as f64 / 100.0),
            _ => {}
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_fit_the_texture_and_the_budget() {
        let c = GpuCaps { cpu: false, msaa: 4, msaa_supported: vec![2, 4], max_texture: 2048, budget_mb: 512, ..GpuCaps::default() };
        assert_eq!(c.target_size(1600, 900), (1600, 900));
        assert_eq!(c.target_size(4096, 1024), (2048, 512), "within the largest texture");
        // 4× MSAA: 36 bytes a pixel; 8 MB holds about 233k pixels.
        let small = GpuCaps { budget_mb: 8, ..c.clone() };
        let (w, h) = small.target_size(1600, 900);
        assert!(f64::from(w) * f64::from(h) * 36.0 <= 8.0 * 1024.0 * 1024.0, "{w}x{h}");
        assert!((f64::from(w) / f64::from(h) - 16.0 / 9.0).abs() < 0.01);
        let one = GpuCaps { msaa: 1, ..small };
        assert!(one.target_size(1600, 900).0 > w, "no MSAA: more pixels in the same memory");
    }

    #[test]
    fn forced_limits() {
        let mut c =
            GpuCaps { cpu: false, msaa: 4, msaa_supported: vec![2, 4], max_texture: 16384, budget_mb: 512, notes: Vec::new(), ..GpuCaps::default() };
        c.force(&from_spec("msaa=1, texture=1024, budget=64"));
        assert_eq!((c.msaa, c.max_texture, c.budget_mb, c.cpu), (1, 1024, 64, false));
        assert!(c.notice().is_some_and(|n| n.contains("1024")));
        c.force(&from_spec("cpu"));
        assert!(c.cpu);
        // The 2D canvas's scale while moving, in percent, held to the floor; null measures again.
        c.force(&from_spec("dynscale=70"));
        assert_eq!(c.dynamic_scale, Some(0.7));
        c.force(&json!({"dynamicScale": 0.1}));
        assert_eq!(c.dynamic_scale, Some(crate::dynres::floor()));
        assert_eq!(c.json()["dynamicScale"], json!(0.5));
        c.force(&json!({"dynamicScale": null}));
        assert_eq!(c.dynamic_scale, None);
        // Forcing can't ask for more than the GPU has.
        let mut weak = GpuCaps { msaa: 1, max_texture: 2048, cpu: false, ..GpuCaps::default() };
        weak.force(&json!({"msaa": 4, "maxTexture": 99999}));
        assert_eq!((weak.msaa, weak.max_texture), (1, 2048));
        // An integrated GPU at 2× can be forced up to the 4× it has, or down to none.
        let mut igpu = GpuCaps { msaa: 2, msaa_supported: vec![2, 4], cpu: false, ..GpuCaps::default() };
        igpu.force(&from_spec("msaa=4"));
        assert_eq!(igpu.msaa, 4);
        igpu.force(&from_spec("msaa=3"));
        assert_eq!(igpu.msaa, 2, "the largest count it has, no more than asked");
        igpu.force(&from_spec("msaa=1"));
        assert_eq!(igpu.msaa, 1);
        let mut only2 = GpuCaps { msaa: 2, msaa_supported: vec![2], cpu: false, ..GpuCaps::default() };
        only2.force(&from_spec("msaa=8"));
        assert_eq!(only2.msaa, 2);
        assert!(soft_name("llvmpipe (LLVM 15.0.7, 256 bits)") && soft_name("Microsoft Basic Render Driver") && !soft_name("Apple M3"));
    }

    /// Multisampling by the kind of adapter: 4× on a discrete GPU, 2× on an integrated one (its
    /// shared memory and bus are what 4× colour and depth cost), none on software; each falls to
    /// the next count the adapter supports.
    #[test]
    fn multisampling_by_adapter() {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};
        let all = |n: u32| n == 2 || n == 4;
        assert_eq!(msaa_for(DiscreteGpu, false, all), 4);
        assert_eq!(msaa_for(Other, false, all), 4, "an adapter that doesn't say keeps today's 4×");
        assert_eq!(msaa_for(IntegratedGpu, false, all), 2);
        assert_eq!([msaa_for(Cpu, true, all), msaa_for(VirtualGpu, true, all), msaa_for(IntegratedGpu, true, all)], [1; 3]);
        assert_eq!(msaa_for(DiscreteGpu, false, |n| n == 2), 2, "no 4×: 2×");
        assert_eq!(msaa_for(IntegratedGpu, false, |n| n == 4), 1, "never more than its kind asks for");
        assert_eq!(msaa_for(DiscreteGpu, false, |_| false), 1);
    }

    /// The lighting quality: by the kind of adapter, a level down at a time, and as asked for.
    #[test]
    fn lighting_quality_by_adapter_and_as_asked() {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, VirtualGpu};
        assert_eq!(Quality::of(DiscreteGpu, false), Quality::Full);
        assert_eq!(Quality::of(IntegratedGpu, false), Quality::Balanced);
        assert_eq!([Quality::of(Cpu, false), Quality::of(VirtualGpu, false), Quality::of(IntegratedGpu, true)], [Quality::Full; 3]);
        assert_eq!(Quality::Full.lower().and_then(Quality::lower), Some(Quality::Performance));
        assert_eq!(Quality::Performance.lower(), None);
        assert!(Quality::Full.soft() && !Quality::Balanced.soft() && Quality::Balanced.shadows() && !Quality::Performance.shadows());
        assert!(Quality::Balanced.lights() && !Quality::Performance.lights());
        let mut c = GpuCaps { cpu: false, quality: Quality::Balanced, ..GpuCaps::default() };
        assert!(!c.quality_forced);
        c.force(&from_spec("quality=Performance"));
        assert_eq!((c.quality, c.quality_forced), (Quality::Performance, true));
        assert_eq!(c.json()["quality"], "performance");
        c.force(&json!({"quality": "full"}));
        assert_eq!(c.quality, Quality::Full);
        // A word that is no level changes nothing.
        c.force(&from_spec("quality=ultra"));
        assert_eq!(c.quality, Quality::Full);
    }
}
