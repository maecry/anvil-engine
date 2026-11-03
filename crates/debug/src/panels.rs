//! Diagnostics panels: plain functions over an `&mut egui::Ui` and plain data.

use std::collections::VecDeque;

use render_graph::{GpuTimings, GraphStats};
use renderer::{DebugDraw, DebugView, PostSettings, RenderPath, Renderer, RendererStats};
use rhi::DeviceDiagnostics;

/// Frame times kept for the history graph.
const HISTORY: usize = 240;

/// Rolling frame times in milliseconds.
#[derive(Default)]
pub struct FrameHistory {
    ms: VecDeque<f32>,
}

impl FrameHistory {
    /// Records one frame's wall time.
    pub fn push(&mut self, ms: f32) {
        if self.ms.len() == HISTORY {
            self.ms.pop_front();
        }
        self.ms.push_back(ms);
    }

    /// Average of the recorded frames in milliseconds.
    #[must_use]
    pub fn average(&self) -> f32 {
        self.ms.iter().sum::<f32>() / self.ms.len().max(1) as f32
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
}

/// FPS, frame time, renderer CPU and GPU time, and the frame time history graph.
pub fn frame_stats(ui: &mut egui::Ui, history: &FrameHistory, stats: &RendererStats) {
    let avg = history.average();
    let s = &stats.stages;
    let cpu = s.clusters
        + s.pack
        + s.cull
        + s.sort
        + s.shadow_lists
        + s.upload
        + s.streaming
        + s.graph_build
        + s.compile
        + s.execute;
    ui.label(format!(
        "{:.0} FPS   frame {avg:.2} ms   renderer CPU {:.2} ms   GPU {:.2} ms",
        1000.0 / avg.max(1e-3),
        cpu.as_secs_f64() * 1e3,
        stats.gpu.total_ms
    ));
    ui.label(format!(
        "draws {}  triangles {}  lights {}  culled {}  instances {}",
        stats.draws, stats.triangles, stats.lights, stats.culled, stats.instances
    ));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 48.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(100));
    // Scale to the slowest frame (at least 16.7 ms) so spikes stay visible.
    let top = history.ms.iter().copied().fold(16.7f32, f32::max);
    let n = history.ms.len().max(2) - 1;
    let points: Vec<egui::Pos2> = history
        .ms
        .iter()
        .enumerate()
        .map(|(i, ms)| {
            egui::pos2(
                rect.left() + rect.width() * i as f32 / n as f32,
                rect.bottom() - rect.height() * (ms / top).min(1.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.0_f32, egui::Color32::LIGHT_GREEN),
    ));
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        format!("{top:.1} ms"),
        egui::FontId::monospace(9.0),
        egui::Color32::GRAY,
    );
}

/// GPU time per render graph pass (an earlier frame, see [`GpuTimings`]).
pub fn gpu_timings(ui: &mut egui::Ui, gpu: &GpuTimings) {
    if gpu.passes.is_empty() {
        ui.label("no GPU timings yet (or no timestamp support)");
        return;
    }
    egui::Grid::new("gpu_timings").striped(true).show(ui, |ui| {
        for (name, ms) in &gpu.passes {
            ui.label(name.as_ref());
            ui.monospace(format!("{ms:.3} ms"));
            ui.end_row();
        }
        ui.strong("total");
        ui.monospace(format!("{:.3} ms", gpu.total_ms));
        ui.end_row();
    });
}

/// Device memory, live objects, bindless usage, validation counts and texture streaming.
pub fn memory(ui: &mut egui::Ui, device: &DeviceDiagnostics, stats: &RendererStats) {
    let (m, o, b) = (&device.memory, &device.live, &device.bindless);
    egui::Grid::new("memory").striped(true).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(k);
            ui.monospace(v);
            ui.end_row();
        };
        row("gpu", device.info.name.clone());
        row("driver", device.info.driver.clone());
        row(
            "memory allocated / reserved",
            format!("{} / {}", mib(m.allocated_bytes), mib(m.reserved_bytes)),
        );
        row(
            "objects",
            format!(
                "{} tex, {} buf, {} pipe, {} smp",
                o.textures, o.buffers, o.pipelines, o.samplers
            ),
        );
        row(
            "bindless images / samplers",
            format!(
                "{}/{} , {}/{}",
                b.image_high_water, b.image_capacity, b.sampler_high_water, b.sampler_capacity
            ),
        );
        row("pending destroys", device.pending_destroys.to_string());
        row(
            "validation e/w/i",
            format!(
                "{}/{}/{}",
                device.validation.errors, device.validation.warnings, device.validation.infos
            ),
        );
        let t = &stats.textures;
        row(
            "texture streaming",
            format!(
                "{} resident of {} (budget {}), {} pending",
                mib(t.resident_bytes),
                mib(t.full_bytes),
                mib(t.budget_bytes),
                t.pending_uploads
            ),
        );
        row("upload ring", mib(stats.ring_bytes_used));
    });
}

/// The frame's render graph: pass order with GPU time, and the graph counters.
pub fn render_graph(ui: &mut egui::Ui, graph: &GraphStats, gpu: &GpuTimings) {
    ui.label(format!(
        "{} passes ({} culled), {} barriers, {} transients in {} slots, compile {:.2} ms",
        graph.passes,
        graph.culled,
        graph.barriers,
        graph.transients,
        graph.slots,
        graph.compile_time.as_secs_f64() * 1e3
    ));
    for (i, (name, ms)) in gpu.passes.iter().enumerate() {
        ui.monospace(format!("{i:>2} {name:<24} {ms:.3} ms"));
    }
}

/// Combo box over `(label, value)` options; true when the selection changed.
fn combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    options: &[(&str, T)],
) -> bool {
    let shown = options.iter().find(|o| o.1 == *value).map_or("?", |o| o.0);
    let mut changed = false;
    egui::ComboBox::from_label(label)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (name, v) in options {
                changed |= ui.selectable_value(value, *v, *name).changed();
            }
        });
    changed
}

/// Renderer settings through the renderer's own setters: render path, post effects, debug
/// view and geometry, GPU culling and the LOD threshold.
pub fn renderer_settings(ui: &mut egui::Ui, renderer: &mut Renderer) {
    let mut path = renderer.render_path();
    if combo(
        ui,
        "render path",
        &mut path,
        &[
            ("forward", RenderPath::Forward),
            ("deferred", RenderPath::Deferred),
        ],
    ) {
        renderer.set_render_path(path);
    }
    let mut post: PostSettings = renderer.post();
    let changed = ui.checkbox(&mut post.ssao, "SSAO").changed()
        | ui.checkbox(&mut post.taa, "TAA").changed()
        | ui.checkbox(&mut post.bloom, "bloom").changed();
    if changed {
        renderer.set_post(post);
    }
    let mut view = renderer.debug_view();
    if combo(
        ui,
        "debug view",
        &mut view,
        &[
            ("none", DebugView::None),
            ("clusters", DebugView::Clusters),
            ("cascades", DebugView::Cascades),
            ("ssao", DebugView::Ssao),
            ("lod", DebugView::Lod),
            ("occlusion", DebugView::Occlusion),
            ("mips", DebugView::Mips),
        ],
    ) {
        renderer.set_debug_view(view);
    }
    let mut draw = renderer.debug_draw();
    if combo(
        ui,
        "debug draw",
        &mut draw,
        &[
            ("none", DebugDraw::None),
            ("bounds", DebugDraw::Bounds),
            ("cascades", DebugDraw::Cascades),
            ("lights", DebugDraw::Lights),
        ],
    ) {
        renderer.set_debug_draw(draw);
    }
    let mut culling = renderer.gpu_culling();
    if ui.checkbox(&mut culling, "GPU culling").changed() {
        renderer.set_gpu_culling(culling);
    }
    let mut lod = renderer.lod_pixels();
    if ui
        .add(egui::Slider::new(&mut lod, 0.0..=8.0).text("LOD pixels"))
        .changed()
    {
        renderer.set_lod_pixels(lod);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_and_averages() {
        let mut h = FrameHistory::default();
        assert_eq!(h.average(), 0.0);
        for i in 0..HISTORY + 10 {
            h.push(if i < 10 { 1000.0 } else { 2.0 });
        }
        assert_eq!(h.ms.len(), HISTORY);
        assert_eq!(h.average(), 2.0);
    }
}
