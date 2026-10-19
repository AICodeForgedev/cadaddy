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

#[cfg(test)]
mod tests {
    use super::*;

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
