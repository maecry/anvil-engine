//! [`EguiRenderer`]: draws egui's tessellated output through the RHI (ADR-0014 §8).

use std::collections::HashMap;

use egui::epaint::{ImageDelta, Primitive};
use egui::{ClippedPrimitive, ImageData, TextureId, TexturesDelta};
use rhi::{
    AddressMode, BlendFactor, BlendState, BufferDesc, BufferHandle, BufferUsage, ColorAttachment,
    CommandEncoder, CullMode, Device, Filter, Format, FrameToken, FrontFace, GraphicsPipelineDesc,
    IndexFormat, LoadOp, Memory, PipelineHandle, PrimitiveTopology, Rect2D, RhiError, SamplerDesc,
    SamplerHandle, ShaderModuleDesc, ShaderStageDesc, StoreOp, TextureDesc, TextureHandle,
    TextureUsage, Viewport,
};

const VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui.vert.spv"));
const FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui.frag.spv"));
/// Smallest per-frame vertex/index buffer.
const MIN_BUFFER_BYTES: u64 = 256 << 10;

/// A font/image texture egui owns: the GPU texture and a CPU copy for partial updates.
struct Managed {
    handle: TextureHandle,
    size: [usize; 2],
    rgba: Vec<u8>,
}

/// What [`EguiRenderer::render`] draws into.
#[derive(Clone, Copy, Debug)]
pub struct EguiTarget {
    /// Color target, in `ColorAttachment` state.
    pub texture: TextureHandle,
    /// Size in pixels.
    pub extent: (u32, u32),
    /// egui's scale ([`egui::FullOutput::pixels_per_point`]).
    pub pixels_per_point: f32,
}

/// One frame slot's host-visible vertex+index buffer.
struct FrameBuffer {
    buffer: BufferHandle,
    address: u64,
    capacity: u64,
}

/// One scissored draw of the frame.
struct Draw {
    texture: TextureId,
    clip: Rect2D,
    first_index: u32,
    index_count: u32,
    vertex_offset: i32,
}

/// Renders egui meshes into a color target with one pipeline: vertices are pulled through a
/// buffer address, indices are a normal index buffer, textures are bindless. Generic over the
/// RHI device like the renderer.
pub struct EguiRenderer {
    pipeline: PipelineHandle,
    sampler: SamplerHandle,
    srgb_target: bool,
    managed: HashMap<u64, Managed>,
    user: HashMap<u64, TextureHandle>,
    next_user: u64,
    frames: Vec<Option<FrameBuffer>>,
}

impl EguiRenderer {
    /// Creates the pipeline and sampler for color targets of `target_format`.
    ///
    /// # Errors
    /// Backend errors from creating the shaders, pipeline or sampler.
    pub fn new<D: Device>(device: &D, target_format: Format) -> Result<Self, RhiError> {
        let module = |bytes: &[u8], label: &str| {
            let words: Vec<u32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect();
            device.create_shader_module(&ShaderModuleDesc {
                spirv: &words,
                label: Some(label),
            })
        };
        let vs = module(VERT, "egui.vert")?;
        let fs = module(FRAG, "egui.frag")?;
        // Premultiplied alpha: src * 1 + dst * (1 - src.a).
        let blend = BlendState {
            enabled: true,
            src_color: BlendFactor::One,
            dst_color: BlendFactor::OneMinusSrcAlpha,
            src_alpha: BlendFactor::One,
            dst_alpha: BlendFactor::OneMinusSrcAlpha,
            ..BlendState::default()
        };
        let pipeline = device.create_graphics_pipeline(&GraphicsPipelineDesc {
            label: Some("egui"),
            vertex: ShaderStageDesc {
                module: vs,
                entry: "main",
            },
            fragment: ShaderStageDesc {
                module: fs,
                entry: "main",
            },
            color_formats: &[target_format],
            depth: None,
            topology: PrimitiveTopology::TriangleList,
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            blend: &[blend],
        });
        device.destroy_shader_module(vs);
        device.destroy_shader_module(fs);
        // shortcut: one linear/clamp sampler for every texture (egui's per-texture nearest
        // option is ignored), upgrade with a second sampler when pixel-art images need it.
        let sampler = device.create_sampler(&SamplerDesc {
            label: Some("egui"),
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            address_u: AddressMode::ClampToEdge,
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::ClampToEdge,
            compare: None,
        })?;
        Ok(Self {
            pipeline: pipeline?,
            sampler,
            srgb_target: target_format.is_srgb(),
            managed: HashMap::new(),
            user: HashMap::new(),
            next_user: 0,
            frames: Vec::new(),
        })
    }

    /// Registers an existing RHI texture (for example an offscreen renderer target) as an egui
    /// image. It must have [`TextureUsage::SAMPLED`], be in `ShaderRead` state when
    /// [`EguiRenderer::render`] runs, and hold gamma-encoded 8-bit color (a `*Unorm` format the
    /// renderer encoded into, not an `*Srgb` one). The caller still owns the texture.
    pub fn register_texture(&mut self, texture: TextureHandle) -> TextureId {
        self.next_user += 1;
        self.user.insert(self.next_user, texture);
        TextureId::User(self.next_user)
    }

    /// Forgets a texture registered with [`EguiRenderer::register_texture`] (it is not destroyed).
    pub fn unregister_texture(&mut self, id: TextureId) {
        if let TextureId::User(n) = id {
            self.user.remove(&n);
        }
    }

    /// Applies egui's texture changes: new and patched images, then frees. A changed image gets
    /// a fresh GPU texture (the old one is destroyed once the GPU is done with it).
    ///
    /// # Errors
    /// Backend errors from creating or uploading a texture.
    pub fn update_textures<D: Device>(
        &mut self,
        device: &D,
        delta: &TexturesDelta,
    ) -> Result<(), RhiError> {
        for (id, d) in &delta.set {
            let TextureId::Managed(n) = *id else { continue };
            self.set_texture(device, n, d)?;
        }
        for id in &delta.free {
            if let TextureId::Managed(n) = id
                && let Some(t) = self.managed.remove(n)
            {
                device.destroy_texture(t.handle);
            }
        }
        Ok(())
    }

    fn set_texture<D: Device>(
        &mut self,
        device: &D,
        n: u64,
        delta: &ImageDelta,
    ) -> Result<(), RhiError> {
        let ImageData::Color(img) = &delta.image;
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
        let [w, h] = img.size;
        let (size, full) = match (delta.pos, self.managed.get(&n)) {
            (None, _) => ([w, h], rgba),
            (Some([x, y]), Some(old)) => {
                let mut full = old.rgba.clone();
                for row in 0..h {
                    let dst = ((y + row) * old.size[0] + x) * 4;
                    full[dst..dst + w * 4].copy_from_slice(&rgba[row * w * 4..(row + 1) * w * 4]);
                }
                (old.size, full)
            }
            (Some(_), None) => {
                tracing::warn!(id = n, "egui texture patch without a base image");
                return Ok(());
            }
        };
        let handle = device.create_texture(&TextureDesc::new_2d(
            Some("egui.texture"),
            size[0] as u32,
            size[1] as u32,
            Format::Rgba8Unorm,
            TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST,
        ))?;
        device.upload_texture(handle, &full)?;
        let new = Managed {
            handle,
            size,
            rgba: full,
        };
        if let Some(old) = self.managed.insert(n, new) {
            device.destroy_texture(old.handle);
        }
        Ok(())
    }

    fn texture_slot<D: Device>(&self, device: &D, id: TextureId) -> Option<u32> {
        let handle = match id {
            TextureId::Managed(n) => self.managed.get(&n)?.handle,
            TextureId::User(n) => *self.user.get(&n)?,
        };
        device.texture_index(handle).ok()
    }

    /// Draws `primitives` into `target` (it stays in `ColorAttachment` state), keeping its
    /// contents (`LoadOp::Load`). Meshes whose texture is unknown are skipped.
    ///
    /// # Errors
    /// Backend errors from growing or writing the vertex/index buffer.
    pub fn render<D: Device>(
        &mut self,
        device: &D,
        cmd: &mut D::Commands,
        frame: &FrameToken,
        target: &EguiTarget,
        primitives: &[ClippedPrimitive],
    ) -> Result<(), RhiError> {
        let EguiTarget {
            texture: target,
            extent,
            pixels_per_point,
        } = *target;
        let (mut vertex_bytes, mut index_bytes) = (0u64, 0u64);
        for p in primitives {
            if let Primitive::Mesh(m) = &p.primitive {
                vertex_bytes += size_of_val(m.vertices.as_slice()) as u64;
                index_bytes += size_of_val(m.indices.as_slice()) as u64;
            }
        }
        if index_bytes == 0 {
            return Ok(());
        }
        let index_base = vertex_bytes.next_multiple_of(16);
        let (buffer, address) =
            self.frame_buffer(device, frame.slot as usize, index_base + index_bytes)?;

        // Fill the buffer and collect the draws.
        let (mut voff, mut ioff) = (0u64, index_base);
        let (mut vertex_count, mut index_count) = (0i32, 0u32);
        let mut draws = Vec::new();
        for p in primitives {
            let Primitive::Mesh(m) = &p.primitive else {
                continue;
            };
            device.write_buffer(buffer, voff, bytemuck::cast_slice(&m.vertices))?;
            device.write_buffer(buffer, ioff, bytemuck::cast_slice(&m.indices))?;
            voff += size_of_val(m.vertices.as_slice()) as u64;
            ioff += size_of_val(m.indices.as_slice()) as u64;
            let ppp = pixels_per_point;
            let r = p.clip_rect;
            let min_x = ((r.min.x * ppp).round().max(0.0) as u32).min(extent.0);
            let min_y = ((r.min.y * ppp).round().max(0.0) as u32).min(extent.1);
            let max_x = ((r.max.x * ppp).round().max(0.0) as u32).clamp(min_x, extent.0);
            let max_y = ((r.max.y * ppp).round().max(0.0) as u32).clamp(min_y, extent.1);
            if max_x > min_x && max_y > min_y {
                draws.push(Draw {
                    texture: m.texture_id,
                    clip: Rect2D {
                        x: min_x as i32,
                        y: min_y as i32,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    },
                    first_index: index_count,
                    index_count: m.indices.len() as u32,
                    vertex_offset: vertex_count,
                });
            }
            vertex_count += m.vertices.len() as i32;
            index_count += m.indices.len() as u32;
        }

        cmd.begin_label("egui", [0.4, 0.7, 0.9, 1.0]);
        cmd.begin_rendering(&rhi::RenderingDesc {
            color: &[ColorAttachment {
                texture: target,
                load: LoadOp::Load,
                store: StoreOp::Store,
                clear: [0.0; 4],
            }],
            depth: None,
            width: extent.0,
            height: extent.1,
        });
        cmd.bind_pipeline(self.pipeline);
        cmd.set_viewport(Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.0 as f32,
            height: extent.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        });
        cmd.bind_index_buffer(buffer, index_base, IndexFormat::U32);
        let sampler = device.sampler_index(self.sampler)?;
        for d in &draws {
            let Some(slot) = self.texture_slot(device, d.texture) else {
                continue;
            };
            let mut pc = [0u8; 32];
            pc[0..8].copy_from_slice(&address.to_le_bytes());
            pc[8..12].copy_from_slice(&(extent.0 as f32 / pixels_per_point).to_le_bytes());
            pc[12..16].copy_from_slice(&(extent.1 as f32 / pixels_per_point).to_le_bytes());
            pc[16..20].copy_from_slice(&slot.to_le_bytes());
            pc[20..24].copy_from_slice(&sampler.to_le_bytes());
            pc[24..28].copy_from_slice(&u32::from(self.srgb_target).to_le_bytes());
            cmd.push_constants(0, &pc);
            cmd.set_scissor(d.clip);
            cmd.draw_indexed(d.index_count, 1, d.first_index, d.vertex_offset, 0);
        }
        cmd.end_rendering();
        cmd.end_label();
        Ok(())
    }

    /// The frame slot's buffer, grown to `bytes` when needed.
    fn frame_buffer<D: Device>(
        &mut self,
        device: &D,
        slot: usize,
        bytes: u64,
    ) -> Result<(BufferHandle, u64), RhiError> {
        if self.frames.len() <= slot {
            self.frames.resize_with(slot + 1, || None);
        }
        if let Some(f) = &self.frames[slot]
            && f.capacity >= bytes
        {
            return Ok((f.buffer, f.address));
        }
        if let Some(old) = self.frames[slot].take() {
            device.destroy_buffer(old.buffer);
        }
        let capacity = bytes.next_power_of_two().max(MIN_BUFFER_BYTES);
        let buffer = device.create_buffer(&BufferDesc {
            label: Some("egui.mesh"),
            size: capacity,
            usage: BufferUsage::STORAGE | BufferUsage::INDEX,
            memory: Memory::Upload,
        })?;
        let address = device.buffer_address(buffer)?;
        self.frames[slot] = Some(FrameBuffer {
            buffer,
            address,
            capacity,
        });
        Ok((buffer, address))
    }

    /// Destroys the pipeline, sampler, egui-owned textures and buffers (user textures stay).
    pub fn destroy<D: Device>(self, device: &D) {
        device.destroy_pipeline(self.pipeline);
        device.destroy_sampler(self.sampler);
        for t in self.managed.into_values() {
            device.destroy_texture(t.handle);
        }
        for f in self.frames.into_iter().flatten() {
            device.destroy_buffer(f.buffer);
        }
    }
}
