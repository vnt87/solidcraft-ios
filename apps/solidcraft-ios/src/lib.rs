//! Native SolidCraft iPadOS bootstrap: Metal window, scene ownership, the shared egui shell, and
//! the document services (managed copies of opened files, saves through a reserved path, one
//! export sheet per destination, designs the Files app opens, recent designs, and the system
//! pasteboard).
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod apple_entry;
mod documents;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod gc_keyboard;
#[cfg(any(target_os = "ios", test))]
mod hw_keys;
#[cfg(any(target_os = "ios", test))]
mod logging;
#[cfg(target_os = "ios")]
mod safe_area;
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod scenes;


/// Window depth and stencil buffers the viewport was designed for (`apps/solidcraft/src/main.rs`).
#[cfg(target_os = "ios")]
const DEPTH_BITS: u8 = 24;
#[cfg(target_os = "ios")]
const STENCIL_BITS: u8 = 8;
/// Simulator Metal adapters sometimes report fewer samples than a device; the app falls back to 1x.
#[cfg(target_os = "ios")]
const MSAA: u32 = 4;

/// Configure Metal explicitly; UIKit/winit supplies the current window dimensions.
pub fn native_options() -> eframe::NativeOptions {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::METAL;
    setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let available = adapter.limits();
        // The same downlevel baseline the engine's own device opens with, raised to what this
        // adapter reports, so the shared egui shader and the viewport's pipelines agree.
        let requested = eframe::wgpu::Limits::downlevel_defaults().using_resolution(available.clone());
        eframe::wgpu::DeviceDescriptor {
            label: Some("solidcraft iOS Metal device"),
            required_limits: compatible_limits(requested, &available),
            ..Default::default()
        }
    });
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_app_id("io.tangencylabs.solidcraft").with_title("SolidCraft"),
        // The viewport draws its scene offscreen with its own depth and MSAA, then blits it.
        depth_buffer: 0,
        stencil_buffer: 0,
        multisampling: 1,
        renderer: eframe::Renderer::Wgpu,
        run_and_return: false,
        // The OS owns the window on iPadOS: there is no geometry to restore.
        persist_window: false,
        persistence_path: persistence_path(),
        wgpu_options: eframe::WgpuConfiguration { wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() },
        ..Default::default()
    }
}

/// Preferences file inside the app container (`Application Support`), which eframe cannot derive
/// for iOS on its own.
fn persistence_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "ios")]
    {
        documents::support_directory().ok().map(|folder| folder.join("app.ron"))
    }
    #[cfg(not(target_os = "ios"))]
    {
        None
    }
}

/// egui uses two inter-stage locations; Metal simulators may expose 15 instead of
/// wgpu's default 16. Keep every other editor limit and the default feature set.
fn compatible_limits(mut requested: eframe::wgpu::Limits, available: &eframe::wgpu::Limits) -> eframe::wgpu::Limits {
    // Below two, device creation must fail rather than pretend the egui shader is supported.
    requested.max_inter_stage_shader_variables = requested.max_inter_stage_shader_variables.min(available.max_inter_stage_shader_variables).max(2);
    requested
}

/// The app with the shell's startup work the desktop host also does, plus the iPadOS document
/// services: managed copies of opened files, a reserved path for every save, and one publish step.
#[cfg(target_os = "ios")]
fn app(cc: &eframe::CreationContext<'_>) -> solidcraft_ui_egui::SolveApp {
    solidcraft_ui_egui::fonts::setup_bundled_fonts(&cc.egui_ctx);
    solidcraft_ui_egui::fonts::load_system_fonts_in_background(&cc.egui_ctx);
    let mut services = solidcraft_ui_egui::Services::default();
    if let Err(error) = documents::install(&mut services, cc.egui_ctx.clone()) {
        log::error!("Document pickers unavailable: {error}");
    }
    let mut app = solidcraft_ui_egui::SolveApp::new(solidcraft_engine::Session::default(), services);
    if let Some(prefs) = cc.storage.and_then(|storage| solidcraft_ui_egui::prefs::stored(|key| storage.get_string(key))) {
        app.load_prefs(&prefs);
    }
    // A tablet's first run starts on the touch mapping (fingers navigate, the Pencil edits); any
    // preset the user picks afterwards, including from Preferences, is kept.
    if !app.preferences.touch_nav_seen {
        let preset = solidcraft_ui_egui::prefs::nav_preset(solidcraft_ui_egui::prefs::TOUCH_NAV);
        solidcraft_ui_egui::prefs::choose_nav_preset(&mut app.preferences, preset);
        app.preferences.touch_nav_seen = true;
    }
    // Long features compute off the window thread on a desktop; an iPad has fewer threads to
    // spare, so they run at once unless the user says otherwise in Preferences.
    app.apply_off_thread = None;
    if let Some(render_state) = cc.wgpu_render_state.clone() {
        let samples = app.set_wgpu_split(&render_state, DEPTH_BITS, STENCIL_BITS, MSAA);
        app.set_status(format!("Renderer: Metal, {}x anti-aliasing", samples.max(1)), false);
    }
    app
}

/// Launch on iOS's main thread; other platforms return an error without a window.
pub fn run() -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        if let Err(error) = logging::install() {
            eprintln!("SolidCraft logging unavailable: {error}");
        }
        log::info!("Starting SolidCraft iPadOS with Metal");
        scenes::register();
        eframe::run_native(
            "SolidCraft",
            native_options(),
            Box::new(|cc| {
                scenes::attach(cc)?;
                // Enter, arrows, Escape, F-keys and every ⌘ chord: winit's UIKit view forwards only
                // text and Backspace, so the hardware keyboard is read separately and injected.
                gc_keyboard::install(cc.egui_ctx.clone());
                // Hide the status bar through the safe platform API so it cannot overlay the
                // application bar; scene-based safe-area handling still feeds egui its insets.
                use winit::platform::ios::WindowExtIOS;
                if let Some(window) = cc.winit_window() {
                    window.set_prefers_status_bar_hidden(true);
                }
                Ok(Box::new(safe_area::RunnerApp::new(app(cc))))
            }),
        )
        .map_err(|error| format!("SolidCraft iOS startup failed: {error}"))
    }
    #[cfg(not(target_os = "ios"))]
    {
        Err("SolidCraft iOS runner requires an iOS device or simulator".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn simulator_varying_limit_does_not_reduce_other_editor_limits() {
        let requested = eframe::wgpu::Limits { max_texture_dimension_2d: 16384, max_buffer_size: 512 * 1024 * 1024, ..Default::default() };
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested.clone() };
        let actual = super::compatible_limits(requested.clone(), &available);
        assert_eq!(actual, eframe::wgpu::Limits { max_inter_stage_shader_variables: 15, ..requested });
    }

    #[test]
    fn adapters_below_egui_minimum_cannot_silently_lower_shader_requirements() {
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 1, ..Default::default() };
        assert_eq!(super::compatible_limits(eframe::wgpu::Limits::default(), &available).max_inter_stage_shader_variables, 2);
    }

    #[test]
    fn capable_adapters_keep_the_egui_varying_requirement() {
        let requested = eframe::wgpu::Limits::default();
        let available = eframe::wgpu::Limits { max_inter_stage_shader_variables: 32, ..requested.clone() };
        assert_eq!(super::compatible_limits(requested.clone(), &available), requested);
    }

    #[test]
    fn runner_rejects_non_ios_hosts() {
        if !cfg!(target_os = "ios") {
            let error = super::run().err().unwrap_or_default();
            assert!(error.contains("iOS"), "unexpected error: {error}");
        }
    }

    #[test]
    fn viewport_uses_os_size_and_only_metal() {
        let options = super::native_options();
        assert_eq!(options.renderer, eframe::Renderer::Wgpu);
        assert_eq!(options.viewport.app_id.as_deref(), Some("io.tangencylabs.solidcraft"));
        assert!(options.viewport.inner_size.is_none());
        assert!(!options.run_and_return);
        match options.wgpu_options.wgpu_setup {
            eframe::egui_wgpu::WgpuSetup::CreateNew(setup) => {
                assert_eq!(setup.instance_descriptor.backends, eframe::wgpu::Backends::METAL);
            }
            _ => panic!("runner must create its own Metal device"),
        }
    }
}
