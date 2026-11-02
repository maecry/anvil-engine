//! Renderer passes as render-graph nodes (ADR-0012 §8). The graph owns barriers, rendering
//! scopes, viewport and scissor; the bodies only bind and draw.

use render_graph::{ColorTarget, DepthTarget, GraphBuilder, Pass, TexId};
use rhi::{BufferHandle, CommandEncoder, Device, IndexFormat, LoadOp, PipelineHandle, StoreOp};

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
    pub hdr: TexId,
    pub depth: TexId,
    /// Textures the shaders sample through bindless indices (shadow map, IBL).
    pub reads: Vec<TexId>,
    /// BDA of frame, instance, light and material data.
    pub addresses: [u64; 4],
    pub draws: &'a [Draw],
    /// Drawn after the opaques in the same scope (reverse-Z far plane, no depth write).
    pub skybox: PipelineHandle,
}

/// Inputs of [`shadow_pass`].
pub(crate) struct ShadowInputs<'a> {
    pub pipeline: PipelineHandle,
    pub texture: TexId,
    pub addresses: [u64; 4],
    pub draws: &'a [Draw],
}

/// Inputs of [`bake_pass`]: one fullscreen fragment pass into an environment texture.
pub(crate) struct BakeInputs {
    pub name: &'static str,
    pub pipeline: PipelineHandle,
    pub target: TexId,
    /// Texture the shader samples, if any.
    pub read: Option<TexId>,
    pub push: Vec<u8>,
}

/// Inputs of [`tonemap_pass`].
pub(crate) struct TonemapInputs {
    pub pipeline: PipelineHandle,
    pub hdr: TexId,
    pub sampler_index: u32,
    pub exposure: f32,
    pub encode_srgb: bool,
    pub target: TexId,
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

/// Depth-only pass into the directional shadow map.
pub(crate) fn shadow_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ShadowInputs<'a>) {
    let pass = Pass::new("shadow").label_color([0.4, 0.4, 0.4, 1.0]).depth(
        i.texture,
        DepthTarget {
            load: LoadOp::Clear,
            store: StoreOp::Store,
            clear: 1.0,
        },
    );
    b.pass(pass, move |ctx| {
        ctx.cmd.bind_pipeline(i.pipeline);
        for d in i.draws {
            record_draw(ctx.cmd, &i.addresses, d);
        }
        Ok(())
    });
}

/// Renders a fullscreen triangle with `pipeline` into `target` (sky, irradiance, one prefilter
/// level or the BRDF LUT; no depth).
pub(crate) fn bake_pass<D: Device>(b: &mut GraphBuilder<'_, D>, i: BakeInputs) {
    let mut pass = Pass::new(i.name)
        .label_color([0.8, 0.8, 0.3, 1.0])
        .color(i.target, ColorTarget::dont_care());
    if let Some(r) = i.read {
        pass = pass.read(r);
    }
    b.pass(pass, move |ctx| {
        ctx.cmd.bind_pipeline(i.pipeline);
        if !i.push.is_empty() {
            ctx.cmd.push_constants(0, &i.push);
        }
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Forward opaque pass into the HDR target with reverse-Z depth, then the skybox.
pub(crate) fn forward_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ForwardInputs<'a>) {
    let mut pass = Pass::new("forward")
        .label_color([0.3, 0.7, 0.3, 1.0])
        .color(i.hdr, ColorTarget::clear([0.02, 0.03, 0.05, 1.0]))
        .depth(
            i.depth,
            DepthTarget {
                load: LoadOp::Clear,
                store: StoreOp::DontCare,
                clear: 0.0, // reverse-Z: far = 0
            },
        );
    for r in &i.reads {
        pass = pass.read(*r);
    }
    b.pass(pass, move |ctx| {
        ctx.cmd.bind_pipeline(i.pipeline);
        for d in i.draws {
            record_draw(ctx.cmd, &i.addresses, d);
        }
        ctx.cmd.bind_pipeline(i.skybox);
        let mut pc = [0u8; 40];
        for (chunk, a) in pc.chunks_mut(8).zip(&i.addresses) {
            chunk.copy_from_slice(&a.to_le_bytes());
        }
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Tonemaps the HDR target into `target`.
pub(crate) fn tonemap_pass<D: Device>(b: &mut GraphBuilder<'_, D>, t: TonemapInputs) {
    let pass = Pass::new("tonemap")
        .label_color([0.7, 0.5, 0.2, 1.0])
        .read(t.hdr)
        .color(t.target, ColorTarget::dont_care());
    b.pass(pass, move |ctx| {
        let hdr_index = ctx.texture_index(t.hdr)?;
        ctx.cmd.bind_pipeline(t.pipeline);
        let mut pc = [0u8; 16];
        pc[0..4].copy_from_slice(&hdr_index.to_le_bytes());
        pc[4..8].copy_from_slice(&t.sampler_index.to_le_bytes());
        pc[8..12].copy_from_slice(&t.exposure.to_le_bytes());
        pc[12..16].copy_from_slice(&u32::from(t.encode_srgb).to_le_bytes());
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}
