//! M2 vertical slice: an indexed quad whose vertices are pulled through a buffer device
//! address, textured with a procedural checkerboard through the bindless texture and sampler
//! arrays, with per-draw data in push constants (LAYER-003 §17).
//!
//! Env: `ANVIL_EXIT_AFTER_FRAMES=n` renders n frames, logs diagnostics and exits (code 1 if any
//! validation error was reported). See ADR-0009 §7.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use platform::{App, AppError, Control, Event, KeyCode, Window, WindowConfig};
use rhi::{
    Acquire, AddressMode, Backend, BlendState, BufferDesc, BufferHandle, BufferUsage,
    ColorAttachment, CommandEncoder, CullMode, Device, DeviceDesc, Filter, Format, FrontFace,
    GraphicsPipelineDesc, HasDisplayHandle, IndexFormat, Instance, InstanceDesc, LoadOp, Memory,
    PipelineHandle, PresentStatus, PrimitiveTopology, Rect2D, RenderingDesc, SamplerDesc,
    SamplerHandle, ShaderModuleDesc, ShaderModuleHandle, ShaderStageDesc, StoreOp, SwapchainDesc,
    SwapchainHandle, TextureDesc, TextureHandle, TextureState, TextureUsage, Viewport,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};

static VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.vert.spv"));
static FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.frag.spv"));

/// `include_bytes!` data is not guaranteed 4-byte aligned, so copy into words.
fn spirv_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// Checkerboard texture: `CELLS` x `CELLS` cells of `CELL_PX` texels each, tightly packed RGBA8.
const CELLS: u32 = 8;
const CELL_PX: u32 = 8;
const TEX_SIZE: u32 = CELLS * CELL_PX;

fn checkerboard() -> Vec<u8> {
    let mut data = Vec::with_capacity((TEX_SIZE * TEX_SIZE * 4) as usize);
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let on = ((x / CELL_PX) + (y / CELL_PX)).is_multiple_of(2);
            let c: [u8; 4] = if on {
                [235, 235, 235, 255]
            } else {
                [40, 90, 200, 255]
            };
            data.extend_from_slice(&c);
        }
    }
    data
}

/// Interleaved `(pos.xy, uv.xy)`, Vulkan clip space (y down).
const VERTICES: [f32; 16] = [
    -1.0, -1.0, 0.0, 0.0, //
    1.0, -1.0, 1.0, 0.0, //
    -1.0, 1.0, 0.0, 1.0, //
    1.0, 1.0, 1.0, 1.0,
];
const INDICES: [u32; 6] = [0, 1, 2, 2, 1, 3];

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn u32_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Push constants of `quad.vert` / `quad.frag`: vertex address, bindless indices, scale.
fn push_constants(vertices: u64, texture: u32, sampler: u32, scale: [f32; 2]) -> Vec<u8> {
    let mut pc = vertices.to_le_bytes().to_vec();
    pc.extend_from_slice(&texture.to_le_bytes());
    pc.extend_from_slice(&sampler.to_le_bytes());
    pc.extend_from_slice(&f32_bytes(&scale));
    pc
}

struct Gfx {
    instance: VulkanInstance,
    surface: VulkanSurface,
    device: VulkanDevice,
    swapchain: SwapchainHandle,
    vs: ShaderModuleHandle,
    fs: ShaderModuleHandle,
    pipeline: PipelineHandle,
    vertices: BufferHandle,
    indices: BufferHandle,
    texture: TextureHandle,
    sampler: SamplerHandle,
    vertex_address: u64,
    texture_index: u32,
    sampler_index: u32,
}

struct TexturedQuad {
    gfx: Option<Gfx>,
    rendered: u64,
    exit_after: Option<u64>,
    failed: Arc<AtomicBool>,
}

impl TexturedQuad {
    fn init(window: &Window) -> anyhow::Result<Gfx> {
        let display = window.display_handle().context("display handle")?.as_raw();
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: "textured-quad".to_owned(),
            display: Some(display),
            ..InstanceDesc::default()
        })?;
        let surface = instance.create_surface(Arc::new(window.clone()))?;
        let device = instance.create_device(&DeviceDesc::default(), Some(&surface))?;
        let (w, h) = window.inner_size();
        let swapchain = device.create_swapchain(
            &surface,
            &SwapchainDesc {
                width: w.max(1),
                height: h.max(1),
                vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0"),
                preferred_format: rhi::Format::Bgra8Srgb,
            },
        )?;
        let format = device.swapchain_format(swapchain)?;
        let vs = device.create_shader_module(&ShaderModuleDesc {
            spirv: &spirv_words(VERT_SPV),
            label: Some("quad.vert"),
        })?;
        let fs = device.create_shader_module(&ShaderModuleDesc {
            spirv: &spirv_words(FRAG_SPV),
            label: Some("quad.frag"),
        })?;
        let pipeline = device.create_graphics_pipeline(&GraphicsPipelineDesc {
            label: Some("quad.pipeline"),
            vertex: ShaderStageDesc {
                module: vs,
                entry: "main",
            },
            fragment: ShaderStageDesc {
                module: fs,
                entry: "main",
            },
            color_formats: &[format],
            depth_format: None,
            topology: PrimitiveTopology::TriangleList,
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            blend: &[BlendState::default()],
        })?;
        // Static geometry lives in GPU-only memory and is filled through the upload path; the
        // copies run ahead of the first frame's commands in the same submission.
        let vertices = device.create_buffer(&BufferDesc {
            label: Some("quad.vertices"),
            size: (VERTICES.len() * 4) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::TRANSFER_DST,
            memory: Memory::GpuOnly,
        })?;
        device.upload_buffer(vertices, 0, &f32_bytes(&VERTICES))?;
        let indices = device.create_buffer(&BufferDesc {
            label: Some("quad.indices"),
            size: (INDICES.len() * 4) as u64,
            usage: BufferUsage::INDEX | BufferUsage::TRANSFER_DST,
            memory: Memory::GpuOnly,
        })?;
        device.upload_buffer(indices, 0, &u32_bytes(&INDICES))?;
        let texture = device.create_texture(&TextureDesc {
            label: Some("quad.checkerboard"),
            width: TEX_SIZE,
            height: TEX_SIZE,
            format: Format::Rgba8Srgb,
            usage: TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST,
        })?;
        device.upload_texture(texture, &checkerboard())?;
        let sampler = device.create_sampler(&SamplerDesc {
            label: Some("quad.sampler"),
            mag_filter: Filter::Nearest,
            min_filter: Filter::Linear,
            address_u: AddressMode::ClampToEdge,
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::ClampToEdge,
        })?;
        let vertex_address = device.buffer_address(vertices)?;
        let texture_index = device.texture_index(texture)?;
        let sampler_index = device.sampler_index(sampler)?;
        Ok(Gfx {
            instance,
            surface,
            device,
            swapchain,
            vs,
            fs,
            pipeline,
            vertices,
            indices,
            texture,
            sampler,
            vertex_address,
            texture_index,
            sampler_index,
        })
    }

    fn render(&mut self, window: &Window) -> anyhow::Result<bool> {
        let Some(g) = self.gfx.as_mut() else {
            return Ok(false);
        };
        if window.is_minimized() {
            return Ok(false);
        }
        let _span = tracing::info_span!("app_frame", frame = self.rendered).entered();
        let frame = g.device.begin_frame()?;
        let image = match g.device.acquire(g.swapchain)? {
            Acquire::Image(i) => i,
            Acquire::OutOfDate => {
                let (w, h) = window.inner_size();
                g.device.resize_swapchain(g.swapchain, w, h)?;
                return Ok(false);
            }
            Acquire::Minimized => return Ok(false),
        };
        let (w, h) = image.extent;
        let mut cmd = g.device.begin_commands(&frame)?;
        cmd.begin_label("textured quad", [0.2, 0.6, 1.0, 1.0]);
        cmd.texture_barrier(
            image.texture,
            TextureState::Undefined,
            TextureState::ColorAttachment,
        );
        cmd.begin_rendering(&RenderingDesc {
            color: &[ColorAttachment {
                texture: image.texture,
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: [0.02, 0.02, 0.05, 1.0],
            }],
            depth: None,
            width: w,
            height: h,
        });
        cmd.set_viewport(Viewport {
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: h as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        });
        cmd.set_scissor(Rect2D {
            x: 0,
            y: 0,
            width: w,
            height: h,
        });
        // Keep the quad square and 80% of the shorter side whatever the window shape.
        let aspect = w as f32 / h as f32;
        let scale = if aspect >= 1.0 {
            [0.8 / aspect, 0.8]
        } else {
            [0.8, 0.8 * aspect]
        };
        cmd.bind_pipeline(g.pipeline);
        cmd.push_constants(
            0,
            &push_constants(g.vertex_address, g.texture_index, g.sampler_index, scale),
        );
        cmd.bind_index_buffer(g.indices, 0, IndexFormat::U32);
        cmd.draw_indexed(INDICES.len() as u32, 1, 0, 0, 0);
        cmd.end_rendering();
        cmd.texture_barrier(
            image.texture,
            TextureState::ColorAttachment,
            TextureState::Present,
        );
        cmd.end_label();
        let finished = cmd.finish()?;
        g.device.submit(finished, &image)?;
        if g.device.present(g.swapchain, image)? == PresentStatus::Recreate {
            let (w, h) = window.inner_size();
            g.device.resize_swapchain(g.swapchain, w, h)?;
        }
        Ok(true)
    }

    /// Destroys everything in reverse creation order and records the validation verdict.
    fn teardown(&mut self) {
        let Some(g) = self.gfx.take() else { return };
        if let Err(e) = g.device.wait_idle() {
            tracing::error!(error = %e, "wait_idle failed");
        }
        g.device.destroy_pipeline(g.pipeline);
        g.device.destroy_sampler(g.sampler);
        g.device.destroy_texture(g.texture);
        g.device.destroy_buffer(g.indices);
        g.device.destroy_buffer(g.vertices);
        g.device.destroy_shader_module(g.fs);
        g.device.destroy_shader_module(g.vs);
        g.device.destroy_swapchain(g.swapchain);
        // After the destroys: live counts must be zero (leak check); destruction is deferred
        // until the device drops, so `pending_destroys` is nonzero here.
        let diag = g.device.diagnostics();
        tracing::info!(?diag, "diagnostics at exit");
        let Gfx {
            instance,
            surface,
            device,
            ..
        } = g;
        drop(device);
        drop(surface);
        let counts = instance.validation_counts();
        tracing::info!(?counts, "final validation counts");
        if counts.errors > 0 {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}

impl App for TexturedQuad {
    fn on_start(&mut self, window: &Window) -> Result<(), AppError> {
        self.gfx = Some(Self::init(window)?);
        Ok(())
    }

    fn on_event(&mut self, _: &Window, event: &Event) -> Control {
        match event {
            Event::Resized { width, height } if *width > 0 && *height > 0 => {
                if let Some(g) = &self.gfx
                    && let Err(e) = g.device.resize_swapchain(g.swapchain, *width, *height)
                {
                    tracing::error!(error = %e, "resize failed");
                    return Control::Exit;
                }
            }
            Event::CloseRequested
            | Event::Key {
                code: KeyCode::Escape,
                pressed: true,
            } => {
                return Control::Exit;
            }
            _ => {}
        }
        Control::Continue
    }

    fn on_frame(&mut self, window: &Window) -> Result<Control, AppError> {
        if self.render(window)? {
            self.rendered += 1;
        }
        if self.exit_after.is_some_and(|n| self.rendered >= n) {
            return Ok(Control::Exit);
        }
        Ok(Control::Continue)
    }

    fn on_exit(&mut self) {
        self.teardown();
    }
}

fn main() -> anyhow::Result<()> {
    foundation::init_logging()?;
    let failed = Arc::new(AtomicBool::new(false));
    let exit_after = std::env::var("ANVIL_EXIT_AFTER_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    platform::run(
        WindowConfig {
            title: "Anvil textured quad".into(),
            ..WindowConfig::default()
        },
        TexturedQuad {
            gfx: None,
            rendered: 0,
            exit_after,
            failed: failed.clone(),
        },
    )?;
    if failed.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
    Ok(())
}
