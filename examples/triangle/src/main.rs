//! M1 vertical slice: window, Vulkan device, swapchain, one triangle.
//!
//! Env: `ANVIL_EXIT_AFTER_FRAMES=n` renders n frames, logs diagnostics and exits (code 1 if any
//! validation error was reported). See ADR-0009 §7.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use platform::{App, AppError, Control, Event, KeyCode, Window, WindowConfig};
use rhi::{
    Acquire, Backend, BlendState, ColorAttachment, CommandEncoder, CullMode, Device, DeviceDesc,
    FrontFace, GraphicsPipelineDesc, HasDisplayHandle, Instance, InstanceDesc, LoadOp,
    PipelineHandle, PresentStatus, PrimitiveTopology, Rect2D, RenderingDesc, ShaderModuleDesc,
    ShaderModuleHandle, ShaderStageDesc, StoreOp, SwapchainDesc, SwapchainHandle, TextureState,
    Viewport,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};

static VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/triangle.vert.spv"));
static FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/triangle.frag.spv"));

/// `include_bytes!` data is not guaranteed 4-byte aligned, so copy into words.
fn spirv_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

struct Gfx {
    instance: VulkanInstance,
    surface: VulkanSurface,
    device: VulkanDevice,
    swapchain: SwapchainHandle,
    vs: ShaderModuleHandle,
    fs: ShaderModuleHandle,
    pipeline: PipelineHandle,
}

struct Triangle {
    gfx: Option<Gfx>,
    rendered: u64,
    exit_after: Option<u64>,
    failed: Arc<AtomicBool>,
}

impl Triangle {
    fn init(window: &Window) -> anyhow::Result<Gfx> {
        let display = window.display_handle().context("display handle")?.as_raw();
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: "triangle".to_owned(),
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
            label: Some("triangle.vert"),
        })?;
        let fs = device.create_shader_module(&ShaderModuleDesc {
            spirv: &spirv_words(FRAG_SPV),
            label: Some("triangle.frag"),
        })?;
        let pipeline = device.create_graphics_pipeline(&GraphicsPipelineDesc {
            label: Some("triangle.pipeline"),
            vertex: ShaderStageDesc {
                module: vs,
                entry: "main",
            },
            fragment: ShaderStageDesc {
                module: fs,
                entry: "main",
            },
            color_formats: &[format],
            depth: None,
            topology: PrimitiveTopology::TriangleList,
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            blend: &[BlendState::default()],
        })?;
        Ok(Gfx {
            instance,
            surface,
            device,
            swapchain,
            vs,
            fs,
            pipeline,
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
        cmd.begin_label("triangle", [0.2, 0.6, 1.0, 1.0]);
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
        cmd.bind_pipeline(g.pipeline);
        cmd.draw(3, 1, 0, 0);
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
        let diag = g.device.diagnostics();
        tracing::info!(?diag, "diagnostics at exit");
        g.device.destroy_pipeline(g.pipeline);
        g.device.destroy_shader_module(g.fs);
        g.device.destroy_shader_module(g.vs);
        g.device.destroy_swapchain(g.swapchain);
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

impl App for Triangle {
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
            title: "Anvil triangle".into(),
            ..WindowConfig::default()
        },
        Triangle {
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
