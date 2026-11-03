//! [`Overlay`]: input + egui renderer + panels in one struct, toggled with F1.

use std::time::Instant;

use egui::ClippedPrimitive;
use platform::{RawWindowEvent, Window};
use renderer::Renderer;
use rhi::{CommandEncoder, Device, FrameToken, RhiError, SwapchainImage, TextureState};

use crate::{EguiInput, EguiRenderer, EguiTarget, panels};

/// The on-screen diagnostics overlay (`ANVIL_OVERLAY=1` in the examples). Per frame an app calls
/// [`Overlay::run`] before building its commands, renders the scene with
/// [`Overlay::scene_final_state`] as the target's end state, then [`Overlay::render`].
pub struct Overlay {
    input: EguiInput,
    gpu: EguiRenderer,
    visible: bool,
    history: panels::FrameHistory,
    last: Option<Instant>,
    primitives: Vec<ClippedPrimitive>,
    pixels_per_point: f32,
}

impl Overlay {
    /// True when `ANVIL_OVERLAY=1`: apps create an overlay only then, so the default path runs
    /// no debug code.
    #[must_use]
    pub fn enabled_by_env() -> bool {
        std::env::var("ANVIL_OVERLAY").as_deref() == Ok("1")
    }

    /// Creates the overlay (visible) for color targets of `target_format`.
    ///
    /// # Errors
    /// Backend errors from creating the egui pipeline.
    pub fn new<D: Device>(
        device: &D,
        window: &Window,
        target_format: rhi::Format,
    ) -> Result<Self, RhiError> {
        Ok(Self {
            input: EguiInput::new(window),
            gpu: EguiRenderer::new(device, target_format)?,
            visible: true,
            history: panels::FrameHistory::default(),
            last: None,
            primitives: Vec::new(),
            pixels_per_point: 1.0,
        })
    }

    /// Feeds a raw window event ([`platform::App::on_raw_event`]); F1 toggles the overlay.
    pub fn on_window_event(&mut self, window: &Window, event: &RawWindowEvent) {
        use egui_winit::winit::{
            event::{ElementState, WindowEvent},
            keyboard::{KeyCode, PhysicalKey},
        };
        if let WindowEvent::KeyboardInput { event: key, .. } = event
            && key.state == ElementState::Pressed
            && !key.repeat
            && key.physical_key == PhysicalKey::Code(KeyCode::F1)
        {
            self.visible = !self.visible;
        }
        if self.visible {
            self.input.on_window_event(window, event);
        }
    }

    /// True when the overlay is shown.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// True while the overlay wants the pointer: skip camera look and clicks.
    #[must_use]
    pub fn wants_pointer(&self) -> bool {
        self.visible && self.input.wants_pointer()
    }

    /// True while the overlay wants the keyboard (a text field has focus): skip key bindings.
    #[must_use]
    pub fn wants_keyboard(&self) -> bool {
        self.visible && self.input.wants_keyboard()
    }

    /// The state the scene render must leave the target in: `ColorAttachment` while the
    /// overlay draws on top, otherwise `final_state`.
    #[must_use]
    pub fn scene_final_state(&self, final_state: TextureState) -> TextureState {
        if self.visible {
            TextureState::ColorAttachment
        } else {
            final_state
        }
    }

    /// Builds this frame's UI from the last frame's stats (settings changes apply to the next
    /// render) and uploads font/image changes. Call once per frame before recording commands.
    ///
    /// # Errors
    /// Backend errors from texture uploads.
    pub fn run<D: Device>(
        &mut self,
        device: &D,
        window: &Window,
        renderer: &mut Renderer,
    ) -> Result<(), RhiError> {
        let now = Instant::now();
        if let Some(last) = self.last.replace(now) {
            self.history.push((now - last).as_secs_f32() * 1e3);
        }
        if !self.visible {
            return Ok(());
        }
        let stats = renderer.stats();
        let diagnostics = device.diagnostics();
        let history = &self.history;
        let out = self.input.run(window, |ctx| {
            egui::Window::new("Anvil (F1)")
                .default_pos([8.0, 8.0])
                .default_width(300.0)
                .show(ctx, |ui| {
                    egui::CollapsingHeader::new("Frame")
                        .default_open(true)
                        .show(ui, |ui| panels::frame_stats(ui, history, &stats));
                    egui::CollapsingHeader::new("Renderer")
                        .default_open(true)
                        .show(ui, |ui| panels::renderer_settings(ui, renderer));
                    egui::CollapsingHeader::new("GPU passes")
                        .show(ui, |ui| panels::gpu_timings(ui, &stats.gpu));
                    egui::CollapsingHeader::new("Render graph")
                        .show(ui, |ui| panels::render_graph(ui, &stats.graph, &stats.gpu));
                    egui::CollapsingHeader::new("Memory & device")
                        .show(ui, |ui| panels::memory(ui, &diagnostics, &stats));
                });
        });
        self.gpu.update_textures(device, &out.textures_delta)?;
        self.pixels_per_point = out.pixels_per_point;
        self.primitives = self
            .input
            .context()
            .tessellate(out.shapes, out.pixels_per_point);
        Ok(())
    }

    /// Draws the UI over `image` (left in `ColorAttachment` by the scene render) and moves it to
    /// `final_state`. Does nothing while hidden.
    ///
    /// # Errors
    /// Backend errors from writing the mesh buffers.
    pub fn render<D: Device>(
        &mut self,
        device: &D,
        cmd: &mut D::Commands,
        frame: &FrameToken,
        image: &SwapchainImage,
        final_state: TextureState,
    ) -> Result<(), RhiError> {
        if !self.visible {
            return Ok(());
        }
        self.gpu.render(
            device,
            cmd,
            frame,
            &EguiTarget {
                texture: image.texture,
                extent: image.extent,
                pixels_per_point: self.pixels_per_point,
            },
            &self.primitives,
        )?;
        cmd.texture_barrier(image.texture, TextureState::ColorAttachment, final_state);
        Ok(())
    }

    /// Destroys the GPU resources.
    pub fn destroy<D: Device>(self, device: &D) {
        self.gpu.destroy(device);
    }
}
