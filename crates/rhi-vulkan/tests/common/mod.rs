//! Helpers shared by the GPU integration tests (gated by `ANVIL_GPU_TESTS=1`).
#![allow(dead_code)] // each test binary uses a subset

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rhi::{
    Backend, BlendState, BufferDesc, BufferHandle, BufferUsage, ColorAttachment, CommandEncoder,
    CullMode, DepthAttachment, Device, DeviceDesc, Format, FrontFace, GraphicsPipelineDesc,
    Instance, InstanceDesc, LoadOp, Memory, PipelineHandle, PrimitiveTopology, Rect2D,
    RenderingDesc, ShaderModuleDesc, ShaderModuleHandle, ShaderStageDesc, StoreOp, TextureDesc,
    TextureHandle, TextureState, TextureUsage, ValidationMode, Viewport,
};
use rhi_vulkan::{Vulkan, VulkanCommands, VulkanDevice, VulkanInstance};

pub static SAMPLE_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sample.vert.spv"));
pub static SAMPLE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sample.frag.spv"));
pub static FLAT_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flat.vert.spv"));
pub static FLAT_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flat.frag.spv"));

pub static EMPTY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/empty.frag.spv"));

pub fn enabled() -> bool {
    let on = std::env::var("ANVIL_GPU_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
    }
    if on {
        let _ = foundation::init_logging();
    }
    on
}

pub fn desc() -> InstanceDesc {
    InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    }
}

/// Instance and headless device, or `None` when GPU tests are disabled.
pub fn setup() -> Option<(VulkanInstance, VulkanDevice)> {
    if !enabled() {
        return None;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    Some((inst, dev))
}

pub fn assert_clean(inst: &VulkanInstance) {
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
}

/// One frame: begin, record with `f`, finish, submit without swapchain. Returns the timeline value.
pub fn frame(dev: &mut VulkanDevice, f: impl FnOnce(&mut VulkanCommands)) -> u64 {
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    f(&mut c);
    let fin = c.finish().unwrap();
    dev.submit_unwindowed(fin).unwrap()
}

/// Runs empty frames until everything deferred is retired.
pub fn retire_all(dev: &mut VulkanDevice) {
    for _ in 0..4 {
        frame(dev, |_| {});
        dev.wait_idle().unwrap();
    }
    dev.begin_frame().unwrap();
    assert_eq!(dev.diagnostics().pending_destroys, 0);
}

pub fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

pub fn module(dev: &VulkanDevice, bytes: &[u8], label: &str) -> ShaderModuleHandle {
    dev.create_shader_module(&ShaderModuleDesc {
        spirv: &words(bytes),
        label: Some(label),
    })
    .unwrap()
}

pub struct Pipe {
    pub vs: ShaderModuleHandle,
    pub fs: ShaderModuleHandle,
    pub pipeline: PipelineHandle,
}

impl Pipe {
    pub fn new(
        dev: &VulkanDevice,
        vert: &[u8],
        frag: &[u8],
        color: Format,
        depth: Option<Format>,
        label: &str,
    ) -> Self {
        let vs = module(dev, vert, &format!("{label}.vert"));
        let fs = module(dev, frag, &format!("{label}.frag"));
        let pipeline = dev
            .create_graphics_pipeline(&GraphicsPipelineDesc {
                label: Some(label),
                vertex: ShaderStageDesc {
                    module: vs,
                    entry: "main",
                },
                fragment: ShaderStageDesc {
                    module: fs,
                    entry: "main",
                },
                color_formats: &[color],
                depth: depth.map(|format| rhi::DepthState {
                    format,
                    compare: rhi::CompareOp::Less,
                    write: true,
                    bias: None,
                }),
                topology: PrimitiveTopology::TriangleList,
                cull: CullMode::None,
                front_face: FrontFace::CounterClockwise,
                blend: &[BlendState::default()],
            })
            .unwrap();
        Self { vs, fs, pipeline }
    }

    pub fn destroy(&self, dev: &VulkanDevice) {
        dev.destroy_pipeline(self.pipeline);
        dev.destroy_shader_module(self.vs);
        dev.destroy_shader_module(self.fs);
    }
}

pub fn bytes_of_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_of_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Full-target quad: `(pos.xy, uv.xy)` per vertex, Vulkan clip space (y down).
pub struct Quad {
    pub vertices: BufferHandle,
    pub indices: BufferHandle,
    pub vertex_address: u64,
}

impl Quad {
    pub fn new(dev: &VulkanDevice) -> Self {
        let verts = bytes_of_f32(&[
            -1.0, -1.0, 0.0, 0.0, //
            1.0, -1.0, 1.0, 0.0, //
            -1.0, 1.0, 0.0, 1.0, //
            1.0, 1.0, 1.0, 1.0,
        ]);
        let idx = bytes_of_u32(&[0, 1, 2, 2, 1, 3]);
        let vertices = dev
            .create_buffer(&BufferDesc {
                label: Some("quad.vertices"),
                size: verts.len() as u64,
                usage: BufferUsage::STORAGE | BufferUsage::TRANSFER_DST,
                memory: Memory::GpuOnly,
            })
            .unwrap();
        let indices = dev
            .create_buffer(&BufferDesc {
                label: Some("quad.indices"),
                size: idx.len() as u64,
                usage: BufferUsage::INDEX | BufferUsage::TRANSFER_DST,
                memory: Memory::GpuOnly,
            })
            .unwrap();
        dev.upload_buffer(vertices, 0, &verts).unwrap();
        dev.upload_buffer(indices, 0, &idx).unwrap();
        let vertex_address = dev.buffer_address(vertices).unwrap();
        Self {
            vertices,
            indices,
            vertex_address,
        }
    }

    pub fn destroy(&self, dev: &VulkanDevice) {
        dev.destroy_buffer(self.vertices);
        dev.destroy_buffer(self.indices);
    }

    /// Binds the index buffer and draws the two triangles (inside rendering).
    pub fn draw(&self, c: &mut VulkanCommands) {
        c.bind_index_buffer(self.indices, 0, rhi::IndexFormat::U32);
        c.draw_indexed(6, 1, 0, 0, 0);
    }
}

pub fn offscreen_target(dev: &VulkanDevice, w: u32, h: u32) -> TextureHandle {
    dev.create_texture(&TextureDesc {
        label: Some("offscreen.color"),
        width: w,
        height: h,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
    })
    .unwrap()
}

pub fn readback_buffer(dev: &VulkanDevice, size: u64) -> BufferHandle {
    dev.create_buffer(&BufferDesc {
        label: Some("readback"),
        size,
        usage: BufferUsage::TRANSFER_DST,
        memory: Memory::Readback,
    })
    .unwrap()
}

/// Renders `draws` into `target` (cleared to opaque black, depth cleared to 1.0), copies the
/// result into a readback buffer and returns the texels.
pub fn render_offscreen(
    dev: &mut VulkanDevice,
    target: TextureHandle,
    depth: Option<TextureHandle>,
    (w, h): (u32, u32),
    draws: impl FnOnce(&mut VulkanCommands),
) -> Vec<u8> {
    let size = u64::from(w) * u64::from(h) * 4;
    let rb = readback_buffer(dev, size);
    frame(dev, |c| {
        c.texture_barrier(
            target,
            TextureState::Undefined,
            TextureState::ColorAttachment,
        );
        if let Some(d) = depth {
            c.texture_barrier(d, TextureState::Undefined, TextureState::DepthAttachment);
        }
        c.begin_rendering(&RenderingDesc {
            color: &[ColorAttachment {
                texture: target,
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: [0.0, 0.0, 0.0, 1.0],
            }],
            depth: depth.map(|texture| DepthAttachment {
                texture,
                load: LoadOp::Clear,
                store: StoreOp::DontCare,
                clear: 1.0,
            }),
            width: w,
            height: h,
        });
        c.set_viewport(Viewport {
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: h as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        });
        c.set_scissor(Rect2D {
            x: 0,
            y: 0,
            width: w,
            height: h,
        });
        draws(c);
        c.end_rendering();
        c.texture_barrier(
            target,
            TextureState::ColorAttachment,
            TextureState::TransferSrc,
        );
        c.copy_texture_to_buffer(target, rb, 0);
    });
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; size as usize];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    dev.destroy_buffer(rb);
    out
}

pub fn pixel(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

// ---------------------------------------------------------------- tracing capture

/// Minimal subscriber recording `(level, message)` of every event on the current thread.
#[derive(Clone, Default)]
pub struct Capture {
    pub events: Arc<Mutex<Vec<(tracing::Level, String)>>>,
    next: Arc<AtomicU64>,
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut v = MessageVisitor(String::new());
        e.record(&mut v);
        self.events
            .lock()
            .unwrap()
            .push((*e.metadata().level(), v.0));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

impl Capture {
    /// Number of captured warn-level events whose message contains `needle`.
    pub fn warnings_containing(&self, needle: &str) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, m)| *l == tracing::Level::WARN && m.contains(needle))
            .count()
    }
}
