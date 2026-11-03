//! [`EguiInput`]: winit events into egui.

use platform::{RawWindowEvent, Window};

/// Owns the egui [`egui::Context`] and the egui-winit state that feeds it.
pub struct EguiInput {
    ctx: egui::Context,
    state: egui_winit::State,
}

impl EguiInput {
    /// Input state for `window`, with a fresh egui context.
    #[must_use]
    pub fn new(window: &Window) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window.winit(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        Self { ctx, state }
    }

    /// The egui context (cheap to clone).
    #[must_use]
    pub fn context(&self) -> &egui::Context {
        &self.ctx
    }

    /// Feeds a raw window event ([`platform::App::on_raw_event`]); true when egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &RawWindowEvent) -> bool {
        self.state.on_window_event(window.winit(), event).consumed
    }

    /// True while egui wants the pointer (over a panel, dragging a widget): apps skip camera look.
    #[must_use]
    pub fn wants_pointer(&self) -> bool {
        self.ctx.wants_pointer_input() || self.ctx.is_pointer_over_area()
    }

    /// True while egui wants the keyboard (a text field has focus): apps skip key bindings.
    #[must_use]
    pub fn wants_keyboard(&self) -> bool {
        self.ctx.wants_keyboard_input()
    }

    /// Runs one egui frame with `ui`, handling cursor and clipboard output.
    pub fn run(&mut self, window: &Window, ui: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let raw = self.state.take_egui_input(window.winit());
        let out = self.ctx.run(raw, ui);
        self.state
            .handle_platform_output(window.winit(), out.platform_output.clone());
        out
    }
}
