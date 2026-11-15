//! Which graphics adapter CADaddy draws with when the computer has more than one (a laptop's
//! integrated and discrete GPUs, a desktop with a second card, a software renderer beside them),
//! what it found, and what it chose.
//!
//! The rule is CADaddy's own, not a library default: among the adapters that can show the
//! window, a discrete GPU first, then an integrated one, then one that doesn't say, then a
//! virtual one, then software. `CADADDY_GPU` changes it: `low-power` (or `integrated`) puts the
//! integrated GPU first, for a longer battery; any other text picks the adapter whose name
//! contains it. wgpu's own `WGPU_POWER_PREF` (`high`, `low`) is honoured when `CADADDY_GPU` isn't
//! set. Every adapter found and the one chosen are logged at start; Help ▸ About and Report a
//! Problem show the chosen one and whether another was available.

use egui_wgpu::wgpu;
use std::sync::{Mutex, PoisonError};

/// Adapters looked at (a hostile driver list can't make the log or the report huge).
const MAX_ADAPTERS: usize = 32;
/// The longest adapter name kept.
const MAX_NAME: usize = 160;

/// Which adapter the person asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preference {
    /// The fastest: a discrete GPU before an integrated one (the default).
    HighPerformance,
    /// The one that uses least power: an integrated GPU before a discrete one.
    LowPower,
    /// The adapter whose name contains this text (any case); the default rule when none does.
    Named(String),
}

impl Preference {
    /// The environment variable that chooses.
    pub const ENV: &'static str = "CADADDY_GPU";

    /// From the values of `CADADDY_GPU` and `WGPU_POWER_PREF`: `CADADDY_GPU` first
    /// (`high-performance`/`discrete`/`high`, `low-power`/`integrated`/`low`, `default`/`auto`, or
    /// part of an adapter's name), then wgpu's `low`/`high`; high performance otherwise.
    pub fn from_values(cadaddy_gpu: Option<&str>, wgpu_power_pref: Option<&str>) -> Preference {
        if let Some(v) = cadaddy_gpu.map(str::trim).filter(|v| !v.is_empty()) {
            let word = v.to_ascii_lowercase().replace(['_', ' '], "-");
            return match word.as_str() {
                "high-performance" | "high" | "discrete" | "default" | "auto" => Preference::HighPerformance,
                "low-power" | "low" | "integrated" | "battery" => Preference::LowPower,
                _ => Preference::Named(v.chars().take(MAX_NAME).collect()),
            };
        }
        match wgpu_power_pref.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("low") => Preference::LowPower,
            _ => Preference::HighPerformance,
        }
    }

    /// From this process's environment.
    pub fn from_env() -> Preference {
        Preference::from_values(std::env::var(Self::ENV).ok().as_deref(), std::env::var("WGPU_POWER_PREF").ok().as_deref())
    }

    /// The wgpu power preference closest to this one (for paths that only take that).
    pub fn power(&self) -> wgpu::PowerPreference {
        match self {
            Preference::LowPower => wgpu::PowerPreference::LowPower,
            _ => wgpu::PowerPreference::HighPerformance,
        }
    }

    /// How the log and the report name it.
    pub fn describe(&self) -> String {
        match self {
            Preference::HighPerformance => "high performance (discrete GPU first)".into(),
            Preference::LowPower => "low power (integrated GPU first)".into(),
            Preference::Named(n) => format!("the adapter named \"{n}\""),
        }
    }
}

/// One adapter as the rule sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub kind: wgpu::DeviceType,
    pub backend: wgpu::Backend,
    /// Whether it can show the window (true when there is no window: a headless render).
    pub presents: bool,
}

impl Candidate {
    pub fn new(name: &str, kind: wgpu::DeviceType, backend: wgpu::Backend, presents: bool) -> Candidate {
        Candidate { name: name.chars().take(MAX_NAME).collect(), kind, backend, presents }
    }

    pub fn of(info: &wgpu::AdapterInfo, presents: bool) -> Candidate {
        Candidate::new(&info.name, info.device_type, info.backend, presents)
    }

    /// A software renderer (the CPU draws), by its kind or its well-known name.
    pub fn software(&self) -> bool {
        self.kind == wgpu::DeviceType::Cpu || crate::gpucaps::soft_name(&self.name)
    }

    /// "Name (discrete GPU, Dx12)".
    pub fn describe(&self) -> String {
        let kind = match self.kind {
            wgpu::DeviceType::DiscreteGpu => "discrete GPU",
            wgpu::DeviceType::IntegratedGpu => "integrated GPU",
            wgpu::DeviceType::VirtualGpu => "virtual GPU",
            wgpu::DeviceType::Cpu => "software",
            wgpu::DeviceType::Other => "GPU",
        };
        let kind = if self.software() && self.kind != wgpu::DeviceType::Cpu { "software" } else { kind };
        format!("{} ({kind}, {:?})", self.name, self.backend)
    }
}

/// The order an adapter is tried in: lower first. Software (by kind or name) is always last;
/// an adapter that doesn't say what it is ("other": usually OpenGL) comes after the GPUs that do.
fn rank(c: &Candidate, low_power: bool) -> u8 {
    if c.software() {
        return 5;
    }
    match c.kind {
        wgpu::DeviceType::DiscreteGpu if low_power => 1,
        wgpu::DeviceType::IntegratedGpu if low_power => 0,
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::Other => 2,
        wgpu::DeviceType::VirtualGpu => 3,
        wgpu::DeviceType::Cpu => 5,
    }
}

/// The adapter to draw with, as an index into `candidates`: among those that can show the window,
/// the named one when one is asked for and found, else the first by [`rank`] (equal ranks keep the
/// order the system listed them in). `None` when none can show the window.
pub fn choose(candidates: &[Candidate], preference: &Preference) -> Option<usize> {
    let usable = || candidates.iter().enumerate().take(MAX_ADAPTERS).filter(|(_, c)| c.presents);
    if let Preference::Named(want) = preference {
        let want = want.to_lowercase();
        if let Some((i, _)) = usable().find(|(_, c)| c.name.to_lowercase().contains(&want)) {
            return Some(i);
        }
    }
    let low_power = *preference == Preference::LowPower;
    usable().min_by_key(|(_, c)| rank(c, low_power)).map(|(i, _)| i)
}

/// What was found and chosen at the last start of the window's graphics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub found: Vec<Candidate>,
    pub chosen: Option<usize>,
    /// The preference, described.
    pub preference: String,
    /// Why the choice isn't what was asked for (a name that matched nothing).
    pub note: Option<String>,
}

impl Choice {
    /// Choose from `found` by `preference`.
    pub fn make(found: Vec<Candidate>, preference: &Preference) -> Choice {
        let found: Vec<Candidate> = found.into_iter().take(MAX_ADAPTERS).collect();
        let chosen = choose(&found, preference);
        let note = match preference {
            Preference::Named(n) if chosen.and_then(|i| found.get(i)).is_some_and(|c| !c.name.to_lowercase().contains(&n.to_lowercase())) => {
                Some(format!("no usable adapter is named \"{n}\": chose by the default rule"))
            }
            _ => None,
        };
        Choice { found, chosen, preference: preference.describe(), note }
    }

    pub fn chosen(&self) -> Option<&Candidate> {
        self.chosen.and_then(|i| self.found.get(i))
    }

    /// The hardware adapters that could have drawn instead (not the chosen one, not software;
    /// the same adapter listed by a second backend counts once).
    pub fn others(&self) -> Vec<&Candidate> {
        let chosen = self.chosen().map(|c| c.name.as_str());
        let mut out: Vec<&Candidate> = Vec::new();
        for c in self.found.iter().filter(|c| c.presents && !c.software() && Some(c.name.as_str()) != chosen) {
            if !out.iter().any(|o| o.name == c.name) {
                out.push(c);
            }
        }
        out
    }

    /// The log's lines: every adapter found, then the one chosen and why.
    pub fn log_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .found
            .iter()
            .enumerate()
            .map(|(i, c)| format!("graphics adapter {}: {}{}", i + 1, c.describe(), if c.presents { "" } else { " — can't show the window" }))
            .collect();
        match self.chosen() {
            Some(c) => lines.push(format!("graphics adapter chosen: {} by {}", c.describe(), self.preference)),
            None => lines.push(format!("no graphics adapter can show the window ({} found)", self.found.len())),
        }
        if let Some(n) = &self.note {
            lines.push(format!("graphics: {n}"));
        }
        lines
    }

    /// One line for Report a Problem: the chosen adapter and the others.
    pub fn summary(&self) -> String {
        let chosen = self.chosen().map_or_else(|| "none".to_string(), Candidate::describe);
        let others = self.others();
        if others.is_empty() {
            format!("{chosen}; no other GPU")
        } else {
            let list: Vec<String> = others.iter().map(|c| c.describe()).collect();
            format!("{chosen} by {}; also available: {}", self.preference, list.join(", "))
        }
    }
}

static LAST: Mutex<Option<Choice>> = Mutex::new(None);

/// The choice made at the last start of the window's graphics (none yet, or a headless run).
pub fn last() -> Option<Choice> {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn record(choice: &Choice) {
    for line in choice.log_lines() {
        log::info!("{line}");
    }
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(choice.clone());
}

/// Choose from wgpu's adapters (`surface`: the window's, if there is one), log what was found and
/// chosen, and keep it for About and Report a Problem. The window's adapter selector and the
/// headless renderers call this.
pub fn pick(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>, preference: &Preference) -> Result<wgpu::Adapter, String> {
    let adapters: Vec<&wgpu::Adapter> = adapters.iter().take(MAX_ADAPTERS).collect();
    let found: Vec<Candidate> = adapters.iter().map(|a| Candidate::of(&a.get_info(), surface.is_none_or(|s| a.is_surface_supported(s)))).collect();
    let choice = Choice::make(found, preference);
    record(&choice);
    choice
        .chosen
        .and_then(|i| adapters.get(i))
        .map(|a| (*a).clone())
        .ok_or_else(|| format!("no graphics adapter can show the window ({} found)", choice.found.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{Backend, DeviceType};

    fn c(name: &str, kind: DeviceType) -> Candidate {
        Candidate::new(name, kind, Backend::Dx12, true)
    }

    /// A laptop's list as the system gives it: the integrated GPU that drives the screen first,
    /// the discrete one second, the software renderer last. Taking the first listed (what a
    /// renderer asking for no preference got) draws on the integrated GPU.
    fn laptop() -> Vec<Candidate> {
        vec![
            c("Sample Integrated Graphics", DeviceType::IntegratedGpu),
            c("Sample Discrete Graphics 4000", DeviceType::DiscreteGpu),
            c("Microsoft Basic Render Driver", DeviceType::Cpu),
        ]
    }

    #[test]
    fn a_dual_gpu_laptop_draws_on_the_discrete_gpu() {
        let list = laptop();
        assert_eq!(choose(&list, &Preference::HighPerformance), Some(1));
        // The same with the discrete one listed first, and with a second backend listing both.
        let mut flipped = list.clone();
        flipped.swap(0, 1);
        assert_eq!(choose(&flipped, &Preference::HighPerformance), Some(0));
        let mut two_backends = list.clone();
        two_backends.push(Candidate::new("Sample Integrated Graphics", DeviceType::IntegratedGpu, Backend::Vulkan, true));
        two_backends.push(Candidate::new("Sample Discrete Graphics 4000", DeviceType::DiscreteGpu, Backend::Vulkan, true));
        assert_eq!(choose(&two_backends, &Preference::HighPerformance), Some(1), "the first-listed backend's discrete GPU");
        let choice = Choice::make(two_backends, &Preference::HighPerformance);
        assert_eq!(choice.others().len(), 1, "the integrated GPU counts once: {:?}", choice.others());
    }

    #[test]
    fn low_power_and_a_name_choose_otherwise() {
        let list = laptop();
        assert_eq!(choose(&list, &Preference::LowPower), Some(0));
        assert_eq!(choose(&list, &Preference::Named("discrete graphics".into())), Some(1));
        assert_eq!(choose(&list, &Preference::Named("INTEGRATED".into())), Some(0));
        // Software only when named, or when nothing else can draw.
        assert_eq!(choose(&list, &Preference::Named("basic render".into())), Some(2));
        assert_eq!(choose(&list[2..], &Preference::HighPerformance), Some(0));
        // A name that matches nothing falls back to the default rule, and says so.
        let choice = Choice::make(list, &Preference::Named("no such card".into()));
        assert_eq!(choice.chosen, Some(1));
        assert!(choice.note.as_deref().is_some_and(|n| n.contains("no such card")), "{:?}", choice.note);
    }

    #[test]
    fn only_an_adapter_that_can_show_the_window_is_chosen() {
        let mut list = laptop();
        if let Some(d) = list.get_mut(1) {
            d.presents = false;
        }
        assert_eq!(choose(&list, &Preference::HighPerformance), Some(0));
        assert_eq!(choose(&list, &Preference::Named("discrete".into())), Some(0));
        assert_eq!(choose(&[], &Preference::HighPerformance), None);
        let none: Vec<Candidate> = list.into_iter().map(|c| Candidate { presents: false, ..c }).collect();
        assert_eq!(choose(&none, &Preference::HighPerformance), None);
        assert!(Choice::make(none, &Preference::HighPerformance).log_lines().last().is_some_and(|l| l.contains("no graphics adapter")));
    }

    /// A GPU that doesn't say what it is (OpenGL's "other") comes after the integrated GPU that
    /// does; a software renderer reported as a GPU (by name) still comes last.
    #[test]
    fn unknown_and_software_named_adapters_rank_after_real_gpus() {
        let list = vec![
            c("llvmpipe (LLVM, 256 bits)", DeviceType::Other),
            c("Sample GL Renderer", DeviceType::Other),
            c("Sample iGPU", DeviceType::IntegratedGpu),
        ];
        assert_eq!(choose(&list, &Preference::HighPerformance), Some(2));
        assert_eq!(choose(&list[..2], &Preference::HighPerformance), Some(1));
        assert_eq!(
            choose(&[c("Sample vGPU", DeviceType::VirtualGpu), c("Sample GL Renderer", DeviceType::Other)], &Preference::HighPerformance),
            Some(1)
        );
    }

    #[test]
    fn the_preference_comes_from_the_environment_values() {
        use Preference::*;
        assert_eq!(Preference::from_values(None, None), HighPerformance);
        assert_eq!(Preference::from_values(Some("low-power"), None), LowPower);
        assert_eq!(Preference::from_values(Some(" Integrated "), Some("high")), LowPower);
        assert_eq!(Preference::from_values(Some("LOW_POWER"), None), LowPower);
        assert_eq!(Preference::from_values(Some("discrete"), Some("low")), HighPerformance, "CADADDY_GPU wins");
        assert_eq!(Preference::from_values(Some(""), Some("low")), LowPower, "an empty CADADDY_GPU defers to wgpu's");
        assert_eq!(Preference::from_values(None, Some("none")), HighPerformance);
        assert_eq!(Preference::from_values(Some("Sample Card"), None), Named("Sample Card".into()));
        assert_eq!(LowPower.power(), wgpu::PowerPreference::LowPower);
        let long = "x".repeat(10_000);
        assert!(matches!(Preference::from_values(Some(&long), None), Named(n) if n.len() == MAX_NAME));
    }

    #[test]
    fn the_log_and_the_report_name_every_adapter_and_the_choice() {
        let choice = Choice::make(laptop(), &Preference::HighPerformance);
        let lines = choice.log_lines();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[0].contains("Sample Integrated Graphics (integrated GPU, Dx12)"), "{lines:?}");
        assert!(lines[3].starts_with("graphics adapter chosen: Sample Discrete Graphics 4000 (discrete GPU"), "{lines:?}");
        let summary = choice.summary();
        assert!(summary.contains("also available: Sample Integrated Graphics") && !summary.contains("Basic Render"), "{summary}");
        let one = Choice::make(vec![c("Sample GPU", DeviceType::IntegratedGpu)], &Preference::HighPerformance);
        assert!(one.summary().ends_with("no other GPU"), "{}", one.summary());
        // A hostile list is capped.
        let many: Vec<Candidate> = (0..1000).map(|i| c(&format!("Card {i}"), DeviceType::Other)).collect();
        assert_eq!(Choice::make(many, &Preference::HighPerformance).found.len(), MAX_ADAPTERS);
    }
}
