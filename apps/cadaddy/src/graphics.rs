//! Graphics instance configuration, applied before wgpu can initialize any driver: Windows'
//! DirectX 12 default, and the fallbacks tried when the window's graphics can't start (a weak,
//! old or virtual GPU): the GL backend, then a software adapter (WARP, llvmpipe, lavapipe).

use eframe::{
    NativeOptions,
    egui_wgpu::WgpuSetup,
    wgpu::{Backends, Dx12Compiler},
};

/// Default to DirectX 12 with FXC before creating the wgpu instance. Explicit
/// `WGPU_BACKEND` and `WGPU_DX12_COMPILER` choices, read by the caller, take precedence.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn configure(options: &mut NativeOptions, backend_override: Option<Backends>, compiler_override: Option<Dx12Compiler>) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        // eframe's default includes GL. Creating that backend can crash inside AMD's
        // atio6axx.dll before adapter selection or any Rust error handling runs, so the
        // window never appears. Choosing an adapter afterwards is too late: only leaving GL
        // out of the instance avoids it. This matches the verified WGPU_BACKEND=dx12
        // workaround without mutating process environment variables.
        create.instance_descriptor.backends = backend_override.unwrap_or(Backends::DX12);
        // Auto can load another program's dxcompiler.dll without a matching dxil.dll.
        // That fails wgpu's indirect-validation pipeline and loses the device before
        // the first frame (issue #608). FXC uses the Windows-provided compiler runtime.
        create.instance_descriptor.backend_options.dx12.shader_compiler = compiler_override.unwrap_or(Dx12Compiler::Fxc);
        log::info!(
            "Windows graphics: backends={:?}, DX12 compiler={:?}",
            create.instance_descriptor.backends,
            create.instance_descriptor.backend_options.dx12.shader_compiler
        );
    }
}

/// One way of starting the window's graphics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// The platform's default (DirectX 12 on Windows, or the `WGPU_BACKEND` asked for).
    Default,
    /// wgpu's OpenGL backend.
    Gl,
    /// A software adapter (the CPU draws: WARP, llvmpipe, lavapipe).
    Software,
}

/// The attempts in order: an explicit `WGPU_BACKEND` is the only one tried; otherwise the
/// default, GL (Windows, where the default leaves it out), then software.
pub fn attempts(backend_override: Option<Backends>) -> Vec<Attempt> {
    if backend_override.is_some() {
        return vec![Attempt::Default];
    }
    if cfg!(target_os = "windows") { vec![Attempt::Default, Attempt::Gl, Attempt::Software] } else { vec![Attempt::Default, Attempt::Software] }
}

/// Set `options` up for `attempt` (after [`configure`]).
pub fn apply(options: &mut NativeOptions, attempt: Attempt) {
    let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup else { return };
    match attempt {
        Attempt::Default => {}
        Attempt::Gl => create.instance_descriptor.backends = Backends::GL,
        Attempt::Software => {
            create.instance_descriptor.backends = Backends::all();
            create.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _surface| {
                adapters
                    .iter()
                    .find(|a| a.get_info().device_type == eframe::wgpu::DeviceType::Cpu)
                    .cloned()
                    .ok_or_else(|| "no software graphics adapter".to_string())
            }));
        }
    }
}

/// What the app tells the user about a fallback in use.
pub fn note(attempt: Attempt) -> Option<&'static str> {
    match attempt {
        Attempt::Default => None,
        Attempt::Gl => Some("Graphics: the default driver couldn't start, so CADaddy is using OpenGL."),
        Attempt::Software => Some("Graphics: no usable GPU, so the CPU is drawing (slower; 3D views are simplified)."),
    }
}

/// What the person is told when no window could be made again for the app (its graphics device
/// was lost and no new one started): why, and that `saved` drawings with unsaved changes were
/// autosaved for Drawing Recovery at the next start.
pub fn gave_up(saved: usize, why: Option<&str>) -> String {
    let why = why.filter(|w| !w.trim().is_empty()).map(|w| format!(" ({})", w.trim())).unwrap_or_default();
    let saved = match saved {
        0 => "No drawing had unsaved changes to autosave.".to_string(),
        1 => "The drawing with unsaved changes was autosaved: it is offered for recovery at the next start.".to_string(),
        n => format!("{n} drawings with unsaved changes were autosaved: they are offered for recovery at the next start."),
    };
    format!(
        "The graphics device stopped working and CADaddy couldn't start drawing again{why}, so it has to close. {saved} Updating the graphics driver, or closing other programs that use a lot of memory, may help."
    )
}

/// Which attempt the window is made with when it has to be made again: its graphics didn't start,
/// or its device was lost (a driver reset, a GPU out of memory) or its drawing panicked while it
/// ran. A lost window is made again on the same kind of device; one lost again soon after is made
/// with the next attempt (OpenGL, then the CPU); when none is left the app gives up.
#[derive(Clone, Debug)]
pub struct Restarts {
    attempts: Vec<Attempt>,
    at: usize,
    /// Windows lost on the current attempt, each within [`Restarts::STABLE`] of being made.
    lost_here: u32,
}

impl Restarts {
    /// A window that ran this long before it was lost was fine: its loss starts the count again.
    pub const STABLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
    /// Windows made again on one attempt, after quick losses, before the next attempt is tried.
    pub const SAME_ATTEMPT: u32 = 2;

    pub fn new(attempts: Vec<Attempt>) -> Self {
        Restarts { attempts, at: 0, lost_here: 0 }
    }

    /// The attempt to make the window with now (none: give up).
    pub fn current(&self) -> Option<Attempt> {
        self.attempts.get(self.at).copied()
    }

    /// The window's graphics didn't start: the next attempt.
    pub fn failed_to_start(&mut self) -> Option<Attempt> {
        self.at += 1;
        self.lost_here = 0;
        self.current()
    }

    /// The window was lost after running for `ran_for`: the attempt to make it again with.
    pub fn lost(&mut self, ran_for: std::time::Duration) -> Option<Attempt> {
        if ran_for >= Self::STABLE {
            self.lost_here = 0;
        }
        self.lost_here += 1;
        if self.lost_here > Self::SAME_ATTEMPT {
            self.at += 1;
            self.lost_here = 0;
        }
        self.current()
    }
}

/// eframe's window run on the app's own event loop, every call into it under `catch_unwind`. A
/// panic in eframe or its renderer ended the process with the drawings open (exit 101): egui's
/// wgpu renderer panics writing a frame to a device that was lost or ran out of memory. Here the
/// window is dropped instead (its device and surfaces, and the app, which hands itself over) and
/// the event loop told to exit; the caller makes the window again.
pub struct Guarded<H> {
    window: Option<H>,
    panicked: Option<String>,
}

impl<H> Guarded<H> {
    pub fn new(window: H) -> Self {
        Guarded { window: Some(window), panicked: None }
    }

    /// The message of the panic that closed the window, if one did.
    pub fn panicked(&self) -> Option<&str> {
        self.panicked.as_deref()
    }

    /// Run `f` on the window, unless a panic has closed it; on a panic drop the window and `exit`.
    pub fn call(&mut self, exit: impl FnOnce(), f: impl FnOnce(&mut H)) {
        let Some(window) = self.window.as_mut() else {
            exit();
            return;
        };
        let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(window))) else { return };
        let message =
            p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
        log::error!("the window panicked: {message}");
        // Dropped while the event loop still runs, as winit wants windows to be; a panic while
        // dropping it doesn't end the app either.
        let window = self.window.take();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(window))).is_err() {
            log::error!("the window panicked again while it was closed");
        }
        self.panicked = Some(message);
        exit();
    }
}

impl<H: winit::application::ApplicationHandler<eframe::UserEvent>> winit::application::ApplicationHandler<eframe::UserEvent> for Guarded<H> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.call(|| event_loop.exit(), |w| w.resumed(event_loop));
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        self.call(|| event_loop.exit(), |w| w.window_event(event_loop, window_id, event));
    }

    fn new_events(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, cause: winit::event::StartCause) {
        self.call(|| event_loop.exit(), |w| w.new_events(event_loop, cause));
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: eframe::UserEvent) {
        self.call(|| event_loop.exit(), |w| w.user_event(event_loop, event));
    }

    fn device_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, device_id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        self.call(|| event_loop.exit(), |w| w.device_event(event_loop, device_id, event));
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.call(|| event_loop.exit(), |w| w.about_to_wait(event_loop));
    }

    fn suspended(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.call(|| event_loop.exit(), |w| w.suspended(event_loop));
    }

    fn exiting(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.call(|| event_loop.exit(), |w| w.exiting(event_loop));
    }

    fn memory_warning(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.call(|| event_loop.exit(), |w| w.memory_warning(event_loop));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_window_is_made_again_then_falls_back_then_gives_up() {
        let quick = std::time::Duration::from_secs(5);
        let mut r = Restarts::new(vec![Attempt::Default, Attempt::Software]);
        assert_eq!(r.current(), Some(Attempt::Default));
        // Lost twice in quick succession: made again on the same kind of device both times.
        assert_eq!(r.lost(quick), Some(Attempt::Default));
        assert_eq!(r.lost(quick), Some(Attempt::Default));
        // A third quick loss moves on to the CPU.
        assert_eq!(r.lost(quick), Some(Attempt::Software));
        // A window that ran a long time before it was lost was fine: same attempt again.
        assert_eq!(r.lost(Restarts::STABLE), Some(Attempt::Software));
        assert_eq!(r.lost(quick), Some(Attempt::Software));
        // Nothing left to try.
        assert_eq!(r.lost(quick), None);
        assert_eq!(r.current(), None);
    }

    #[test]
    fn giving_up_says_why_and_what_was_autosaved() {
        let t = gave_up(2, Some("Out of memory"));
        assert!(t.contains("(Out of memory)") && t.contains("2 drawings") && t.contains("recovery"), "{t}");
        let t = gave_up(0, None);
        assert!(t.contains("No drawing had unsaved changes") && !t.contains("()"), "{t}");
    }

    #[test]
    fn a_window_that_does_not_start_takes_the_next_attempt() {
        let mut r = Restarts::new(vec![Attempt::Default, Attempt::Gl, Attempt::Software]);
        assert_eq!(r.lost(std::time::Duration::ZERO), Some(Attempt::Default));
        assert_eq!(r.failed_to_start(), Some(Attempt::Gl));
        // The count of quick losses starts again on the new attempt.
        assert_eq!(r.lost(std::time::Duration::ZERO), Some(Attempt::Gl));
        assert_eq!(r.lost(std::time::Duration::ZERO), Some(Attempt::Gl));
        assert_eq!(r.failed_to_start(), Some(Attempt::Software));
        assert_eq!(r.failed_to_start(), None);
    }

    /// The window's drawing panics (egui's renderer on a lost device): the panic stops at the
    /// guard, the window is dropped at once (its app handing itself over), the event loop is told
    /// to exit, and nothing more reaches the dropped window.
    #[test]
    fn a_panic_in_the_window_closes_it_instead_of_the_app() {
        struct Window(std::rc::Rc<std::cell::Cell<u32>>);
        impl Drop for Window {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 100);
            }
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut g = Guarded::new(Window(std::rc::Rc::clone(&calls)));
        let exits = std::cell::Cell::new(0);
        g.call(|| exits.set(exits.get() + 1), |w| w.0.set(w.0.get() + 1));
        assert_eq!((calls.get(), exits.get(), g.panicked()), (1, 0, None));
        g.call(|| exits.set(exits.get() + 1), |_| std::panic::panic_any("Failed to create staging buffer for index data".to_string()));
        assert_eq!(g.panicked(), Some("Failed to create staging buffer for index data"));
        assert_eq!((calls.get(), exits.get()), (101, 1), "dropped once, exit asked");
        g.call(|| exits.set(exits.get() + 1), |w| w.0.set(w.0.get() + 1));
        assert_eq!((calls.get(), exits.get()), (101, 2), "nothing reaches the dropped window");
    }

    fn configured_backends(backend_override: Option<Backends>) -> Backends {
        let mut options = NativeOptions::default();
        configure(&mut options, backend_override, None);
        let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        create.instance_descriptor.backends
    }

    #[test]
    fn windows_default_initializes_only_dx12() {
        // eframe's own default (PRIMARY | GL) would initialize OpenGL as well.
        let WgpuSetup::CreateNew(eframe_default) = NativeOptions::default().wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        assert!(eframe_default.instance_descriptor.backends.contains(Backends::GL));
        assert_eq!(configured_backends(None), Backends::DX12);
    }

    #[test]
    fn windows_default_uses_fxc_instead_of_searching_for_dxc() {
        let mut options = NativeOptions::default();
        if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
            // Auto can find an unrelated DXC without its matching DXIL runtime (issue #608).
            create.instance_descriptor.backend_options.dx12.shader_compiler = Dx12Compiler::Auto;
        }
        configure(&mut options, None, None);
        let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        assert!(matches!(create.instance_descriptor.backend_options.dx12.shader_compiler, Dx12Compiler::Fxc));
    }

    #[test]
    fn explicit_compiler_and_backend_overrides_are_preserved() {
        let backends = Backends::VULKAN | Backends::DX12;
        for compiler in
            [Dx12Compiler::Fxc, Dx12Compiler::Auto, Dx12Compiler::StaticDxc, Dx12Compiler::DynamicDxc { dxc_path: "custom/dxcompiler.dll".into() }]
        {
            let mut options = NativeOptions::default();
            configure(&mut options, Some(backends), Some(compiler.clone()));
            let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
                panic!("default native options must create a wgpu instance");
            };
            assert_eq!(create.instance_descriptor.backends, backends);
            match (compiler, create.instance_descriptor.backend_options.dx12.shader_compiler) {
                (Dx12Compiler::Fxc, Dx12Compiler::Fxc)
                | (Dx12Compiler::Auto, Dx12Compiler::Auto)
                | (Dx12Compiler::StaticDxc, Dx12Compiler::StaticDxc) => {}
                (Dx12Compiler::DynamicDxc { dxc_path: expected }, Dx12Compiler::DynamicDxc { dxc_path: actual }) => {
                    assert_eq!(actual, expected);
                }
                (expected, actual) => panic!("compiler override changed from {expected:?} to {actual:?}"),
            }
        }
    }

    #[test]
    fn explicit_backend_override_is_preserved() {
        for backends in [Backends::VULKAN, Backends::GL, Backends::DX12, Backends::VULKAN | Backends::DX12, Backends::empty()] {
            assert_eq!(configured_backends(Some(backends)), backends);
        }
    }

    #[test]
    fn fallbacks_after_the_default() {
        assert_eq!(attempts(Some(Backends::VULKAN)), vec![Attempt::Default]);
        let a = attempts(None);
        assert_eq!(a.first(), Some(&Attempt::Default));
        assert_eq!(a.last(), Some(&Attempt::Software));
        assert_eq!(a.contains(&Attempt::Gl), cfg!(target_os = "windows"));
        let mut options = NativeOptions::default();
        apply(&mut options, Attempt::Gl);
        let WgpuSetup::CreateNew(create) = &options.wgpu_options.wgpu_setup else { panic!("a new setup") };
        assert_eq!(create.instance_descriptor.backends, Backends::GL);
        let mut options = NativeOptions::default();
        apply(&mut options, Attempt::Software);
        let WgpuSetup::CreateNew(create) = &options.wgpu_options.wgpu_setup else { panic!("a new setup") };
        assert!(create.native_adapter_selector.is_some());
        assert!(note(Attempt::Default).is_none() && note(Attempt::Software).is_some());
    }
}
