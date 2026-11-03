//! The runner's `eframe::App`: the desktop shell's job, reduced to what an iPad runs, plus the
//! safe-area bridge and diagnostics.
//!
//! winit sizes the Metal surface to the safe area while egui lays out the full view; the wrapper
//! feeds UIKit's window insets into every frame and logs both sides whenever they change, so a
//! device run shows whether they agree.

use eframe::egui;
use objc2_foundation::MainThreadMarker;
use solidcraft_ui_egui::SolveApp;

pub struct RunnerApp {
    solve: SolveApp,
    last: Option<String>,
    /// The last text written to the pasteboard, so one copy writes it once.
    last_copied: Option<String>,
    /// Hardware-keyboard state: the keys winit never delivers (Enter, arrows, ⌘ shortcuts).
    keys: crate::hw_keys::HwKeys,
    /// Events produced outside the input hook (releasing keys on focus loss), sent with the next frame.
    pending: Vec<egui::Event>,
}

impl RunnerApp {
    pub fn new(solve: SolveApp) -> Self {
        Self { solve, last: None, last_copied: None, keys: Default::default(), pending: Vec::new() }
    }

    /// Logs what egui lays out with: the persisted insets, not the per-frame raw input, which only
    /// carries them on resize, scale, focus and occlusion events.
    fn report(&mut self, ctx: &egui::Context) {
        let (screen, insets) = ctx.input(|i| (i.viewport_rect(), i.safe_area_insets().0));
        let uikit = MainThreadMarker::new().and_then(crate::scenes::metrics);
        let content = ctx.content_rect();
        let line = match uikit {
            Some([w, h, top, left, bottom, right]) => format!(
                "egui screen {:.0}x{:.0} insets t{:.0} l{:.0} b{:.0} r{:.0} content {:.0}x{:.0}; UIWindow bounds {w:.0}x{h:.0} insets t{top:.0} l{left:.0} b{bottom:.0} r{right:.0}; surface {:.0}x{:.0}",
                screen.width(),
                screen.height(),
                insets.top,
                insets.left,
                insets.bottom,
                insets.right,
                content.width(),
                content.height(),
                w - left - right,
                h - top - bottom,
            ),
            None => format!("egui screen {:.0}x{:.0}; UIKit metrics unavailable", screen.width(), screen.height()),
        };
        if self.last.as_deref() != Some(line.as_str()) {
            log::info!("safe area: {line}");
            self.last = Some(line);
        }
    }

    /// Text egui copied this frame (a text field, a report, a design's clipboard line) belongs on
    /// the system pasteboard too, which is the only clipboard other apps can read.
    fn mirror_clipboard(&mut self, ctx: &egui::Context) {
        let copied = ctx.output(|output| {
            output.commands.iter().rev().find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
        });
        if let Some(text) = copied.filter(|text| !text.is_empty())
            && self.last_copied.as_deref() != Some(text.as_str())
        {
            crate::scenes::set_pasteboard_text(&text);
            self.last_copied = Some(text);
        }
    }
}

/// Lay the app out inside the unobscured rectangle, filling the band above it (the status bar is
/// hidden, but the safe area still reserves it on some scenes).
fn content_ui(app: &mut SolveApp, ui: &mut egui::Ui) {
    let content = ui.ctx().content_rect();
    let above = egui::Rect::from_min_max(ui.max_rect().min, egui::pos2(ui.max_rect().right(), content.top()));
    ui.painter().rect_filled(above, 0.0, solidcraft_ui_egui::theme::Tokens::get().app_bar);
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| app.ui(ui));
}

impl eframe::App for RunnerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.report(ctx);
        if !ctx.input(|i| i.focused) {
            // Key-ups may never arrive once the app is in the background.
            self.pending.extend(self.keys.release_all());
        }
        // The design on screen is the one whose commands a crash report replays.
        self.solve.session.crash_journal = solidcraft_engine::crash::enabled();
        // The system asking the window to close (a scene disconnect, the Simulator being shut
        // down): ask about unsaved designs first, as the desktop shell does.
        if ctx.input(|i| i.viewport().close_requested()) && !self.solve.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            solidcraft_ui_egui::documents::request_quit(&mut self.solve);
        }
        self.solve.logic(ctx);
        // A design the Files app handed over, or a recent one the user clicked.
        crate::documents::open_ready(&mut self.solve);
        if self.solve.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        content_ui(&mut self.solve, ui);
        self.mirror_clipboard(ui.ctx());
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The keys winit's UIKit view never forwards: read the HID transitions and inject them.
        let now = raw_input.time.unwrap_or_default();
        for (code, pressed) in crate::gc_keyboard::drain() {
            raw_input.events.extend(self.keys.handle(code, pressed, now));
        }
        raw_input.events.append(&mut self.pending);
        raw_input.events.extend(self.keys.tick(now));
        if let Some(wait) = self.keys.next_wake(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.001)));
        }
        self.solve.raw_input_hook(raw_input);
        // egui's window layer keeps its own clipboard; a paste the user makes carries what the
        // system pasteboard holds (read only on that gesture: iOS asks before another app's
        // text crosses over).
        if raw_input.events.iter().any(|event| matches!(event, egui::Event::Paste(_)))
            && let Some(text) = crate::scenes::pasteboard_text()
        {
            for event in &mut raw_input.events {
                if let egui::Event::Paste(pasted) = event {
                    *pasted = text.clone();
                }
            }
        }
        // egui-winit reads the insets from a foreground scene's key window and reports zero until
        // that window exists (launch, scene activation). UIKit's window is the source of truth.
        if let Some([_, _, top, left, bottom, right]) = MainThreadMarker::new().and_then(crate::scenes::metrics) {
            raw_input.safe_area_insets =
                Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { top: top as f32, left: left as f32, bottom: bottom as f32, right: right as f32 }));
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(solidcraft_ui_egui::prefs::STORAGE_KEY, self.solve.prefs());
    }

    fn on_exit(&mut self) {
        // The agent chat's agent and everything it started end with the app.
        solidcraft_ui_egui::agent_chat::shutdown(&mut self.solve);
        // A save still being written ends before the app does.
        solidcraft_ui_egui::save_job::tick(&mut self.solve, true);
        // Pictures still being drawn off the window thread can crash the driver if the process
        // exits with them in flight.
        if !solidcraft_engine::capture::close(std::time::Duration::from_secs(5)) {
            log::warn!("a picture was still being drawn as the app closed");
        }
        solidcraft_ui_egui::recovery_ui::close(&mut self.solve);
        solidcraft_ui_egui::open_elsewhere::close(&mut self.solve);
        solidcraft_engine::crash::end_session();
    }
}
