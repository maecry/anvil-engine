//! Render passes as free functions over explicit inputs, so M4 can turn them into graph nodes.

use rhi::{
    BufferHandle, ColorAttachment, CommandEncoder, DepthAttachment, IndexFormat, LoadOp,
    PipelineHandle, Rect2D, RenderingDesc, StoreOp, TextureHandle, TextureState, Viewport,
};

/// One indexed draw.
pub(crate) struct Draw {
    pub vertex_address: u64,
    pub index_buffer: BufferHandle,
    pub index_count: u32,
    /// Index into the instance array.
    pub instance: u32,
}

/// Inputs of [`forward_pass`].
pub(crate) struct ForwardInputs<'a> {
    pub pipeline: PipelineHandle,
    pub hdr: TextureHandle,
    pub depth: TextureHandle,
    pub extent: (u32, u32),
    /// False on the first frame after the targets were (re)created.
    pub targets_initialized: bool,
    /// BDA of frame, instance, light and material data.
    pub addresses: [u64; 4],
    pub draws: &'a [Draw],
    /// Drawn after the opaques in the same scope (reverse-Z far plane, no depth write).
    pub skybox: PipelineHandle,
}

/// Inputs of [`shadow_pass`].
pub(crate) struct ShadowInputs<'a> {
    pub pipeline: PipelineHandle,
    pub texture: TextureHandle,
    pub size: u32,
    /// False on the first frame, when the texture is still `Undefined`.
    pub initialized: bool,
    pub addresses: [u64; 4],
    pub draws: &'a [Draw],
}

/// Inputs of [`bake_pass`]: one fullscreen fragment pass into an environment texture.
pub(crate) struct BakeInputs<'a> {
    pub pipeline: PipelineHandle,
    pub target: TextureHandle,
    pub extent: (u32, u32),
    /// False on the first bake of this texture (still `Undefined`).
    pub initialized: bool,
    pub push: &'a [u8],
}

/// Inputs of [`tonemap_pass`].
pub(crate) struct TonemapInputs {
    pub pipeline: PipelineHandle,
    pub hdr_index: u32,
    pub sampler_index: u32,
    pub exposure: f32,
    pub encode_srgb: bool,
    pub target: TextureHandle,
    pub extent: (u32, u32),
}

fn full_viewport<C: CommandEncoder>(cmd: &mut C, (w, h): (u32, u32)) {
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
}

/// Pushes the four frame-data addresses plus the draw's vertex address, binds the index buffer
/// and issues the draw; `first_instance` doubles as the instance array index (`gl_InstanceIndex`).
fn record_draw<C: CommandEncoder>(cmd: &mut C, addresses: &[u64; 4], d: &Draw) {
    let mut pc = [0u8; 40];
    for (chunk, a) in pc
        .chunks_mut(8)
        .zip(addresses.iter().chain([&d.vertex_address]))
    {
        chunk.copy_from_slice(&a.to_le_bytes());
    }
    cmd.push_constants(0, &pc);
    cmd.bind_index_buffer(d.index_buffer, 0, IndexFormat::U32);
    cmd.draw_indexed(d.index_count, 1, 0, 0, d.instance);
}

/// Depth-only pass into the directional shadow map; leaves it in `ShaderRead`.
pub(crate) fn shadow_pass<C: CommandEncoder>(cmd: &mut C, i: &ShadowInputs) {
    cmd.begin_label("shadow", [0.4, 0.4, 0.4, 1.0]);
    let from = if i.initialized {
        TextureState::ShaderRead
    } else {
        TextureState::Undefined
    };
    cmd.texture_barrier(i.texture, from, TextureState::DepthAttachment);
    cmd.begin_rendering(&RenderingDesc {
        color: &[],
        depth: Some(DepthAttachment {
            texture: i.texture,
            load: LoadOp::Clear,
            store: StoreOp::Store,
            clear: 1.0,
        }),
        width: i.size,
        height: i.size,
    });
    full_viewport(cmd, (i.size, i.size));
    cmd.bind_pipeline(i.pipeline);
    for d in i.draws {
        record_draw(cmd, &i.addresses, d);
    }
    cmd.end_rendering();
    cmd.texture_barrier(
        i.texture,
        TextureState::DepthAttachment,
        TextureState::ShaderRead,
    );
    cmd.end_label();
}

/// Renders a fullscreen triangle with `pipeline` into `target` (sky, irradiance, one prefilter
/// level or the BRDF LUT; no depth) and leaves it in `ShaderRead`.
pub(crate) fn bake_pass<C: CommandEncoder>(cmd: &mut C, b: &BakeInputs) {
    cmd.begin_label("bake", [0.8, 0.8, 0.3, 1.0]);
    let from = if b.initialized {
        TextureState::ShaderRead
    } else {
        TextureState::Undefined
    };
    cmd.texture_barrier(b.target, from, TextureState::ColorAttachment);
    cmd.begin_rendering(&RenderingDesc {
        color: &[ColorAttachment {
            texture: b.target,
            load: LoadOp::DontCare,
            store: StoreOp::Store,
            clear: [0.0; 4],
        }],
        depth: None,
        width: b.extent.0,
        height: b.extent.1,
    });
    full_viewport(cmd, b.extent);
    cmd.bind_pipeline(b.pipeline);
    if !b.push.is_empty() {
        cmd.push_constants(0, b.push);
    }
    cmd.draw(3, 1, 0, 0);
    cmd.end_rendering();
    cmd.texture_barrier(
        b.target,
        TextureState::ColorAttachment,
        TextureState::ShaderRead,
    );
    cmd.end_label();
}

/// Forward opaque pass into the HDR target with reverse-Z depth; leaves HDR in `ShaderRead`.
pub(crate) fn forward_pass<C: CommandEncoder>(cmd: &mut C, i: &ForwardInputs) {
    cmd.begin_label("forward", [0.3, 0.7, 0.3, 1.0]);
    let (hdr_from, depth_from) = if i.targets_initialized {
        (TextureState::ShaderRead, TextureState::DepthAttachment)
    } else {
        (TextureState::Undefined, TextureState::Undefined)
    };
    cmd.texture_barrier(i.hdr, hdr_from, TextureState::ColorAttachment);
    cmd.texture_barrier(i.depth, depth_from, TextureState::DepthAttachment);
    cmd.begin_rendering(&RenderingDesc {
        color: &[ColorAttachment {
            texture: i.hdr,
            load: LoadOp::Clear,
            store: StoreOp::Store,
            clear: [0.02, 0.03, 0.05, 1.0],
        }],
        depth: Some(DepthAttachment {
            texture: i.depth,
            load: LoadOp::Clear,
            store: StoreOp::DontCare,
            clear: 0.0, // reverse-Z: far = 0
        }),
        width: i.extent.0,
        height: i.extent.1,
    });
    full_viewport(cmd, i.extent);
    cmd.bind_pipeline(i.pipeline);
    for d in i.draws {
        record_draw(cmd, &i.addresses, d);
    }
    cmd.bind_pipeline(i.skybox);
    let mut pc = [0u8; 40];
    for (chunk, a) in pc.chunks_mut(8).zip(&i.addresses) {
        chunk.copy_from_slice(&a.to_le_bytes());
    }
    cmd.push_constants(0, &pc);
    cmd.draw(3, 1, 0, 0);
    cmd.end_rendering();
    cmd.texture_barrier(
        i.hdr,
        TextureState::ColorAttachment,
        TextureState::ShaderRead,
    );
    cmd.end_label();
}

/// Tonemaps the HDR target into `target`; leaves `target` in `ColorAttachment`.
pub(crate) fn tonemap_pass<C: CommandEncoder>(cmd: &mut C, t: &TonemapInputs) {
    cmd.begin_label("tonemap", [0.7, 0.5, 0.2, 1.0]);
    cmd.texture_barrier(
        t.target,
        TextureState::Undefined,
        TextureState::ColorAttachment,
    );
    cmd.begin_rendering(&RenderingDesc {
        color: &[ColorAttachment {
            texture: t.target,
            load: LoadOp::DontCare,
            store: StoreOp::Store,
            clear: [0.0; 4],
        }],
        depth: None,
        width: t.extent.0,
        height: t.extent.1,
    });
    full_viewport(cmd, t.extent);
    cmd.bind_pipeline(t.pipeline);
    let mut pc = [0u8; 16];
    pc[0..4].copy_from_slice(&t.hdr_index.to_le_bytes());
    pc[4..8].copy_from_slice(&t.sampler_index.to_le_bytes());
    pc[8..12].copy_from_slice(&t.exposure.to_le_bytes());
    pc[12..16].copy_from_slice(&u32::from(t.encode_srgb).to_le_bytes());
    cmd.push_constants(0, &pc);
    cmd.draw(3, 1, 0, 0);
    cmd.end_rendering();
    cmd.end_label();
}
