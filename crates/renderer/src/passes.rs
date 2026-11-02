//! Renderer passes as render-graph nodes (ADR-0012 §8). The graph owns barriers, rendering
//! scopes, viewport and scissor; the bodies only bind and draw.

use render_graph::{ColorTarget, DepthTarget, GraphBuilder, Pass, TexId};
use rhi::{BufferHandle, CommandEncoder, Device, IndexFormat, LoadOp, PipelineHandle, StoreOp};

/// One indexed draw.
#[derive(Clone, Copy)]
pub(crate) struct Draw {
    pub vertex_address: u64,
    pub index_buffer: BufferHandle,
    pub index_count: u32,
    /// Index into the instance array.
    pub instance: u32,
    /// Pipeline variant: `mask * 2 + double_sided` (blended draws: `double_sided`).
    pub variant: usize,
    /// View depth of the bounding sphere center, the sort key.
    pub depth: f32,
}

/// One pipeline per [`Draw::variant`].
pub(crate) type Pipes = [PipelineHandle; 4];

/// Inputs shared by the geometry passes.
pub(crate) struct GeometryInputs<'a> {
    pub pipes: Pipes,
    /// BDA of frame, instance, light and material data.
    pub addresses: [u64; 4],
    pub draws: &'a [Draw],
}

/// Inputs of [`shadow_pass`].
pub(crate) struct ShadowInputs<'a> {
    pub name: &'static str,
    pub texture: TexId,
    /// 0..3 cascade, 4..7 spot (`pc.view_index` in `shadow.vert`).
    pub view_index: u32,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`prepass`].
pub(crate) struct PrepassInputs<'a> {
    pub depth: TexId,
    pub normal: TexId,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`forward_pass`] and [`transparent_pass`].
pub(crate) struct ShadedInputs<'a> {
    pub hdr: TexId,
    pub depth: TexId,
    /// Textures the shaders sample through bindless indices (shadow maps, IBL).
    pub reads: Vec<TexId>,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`gbuffer_pass`].
pub(crate) struct GbufferInputs<'a> {
    pub targets: [TexId; 3],
    pub depth: TexId,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`lighting_pass`].
pub(crate) struct LightingInputs {
    pub pipeline: PipelineHandle,
    pub hdr: TexId,
    pub gbuffer: [TexId; 3],
    pub depth: TexId,
    pub reads: Vec<TexId>,
    pub addresses: [u64; 4],
}

/// Inputs of [`skybox_pass`].
pub(crate) struct SkyboxInputs {
    pub pipeline: PipelineHandle,
    pub hdr: TexId,
    pub depth: TexId,
    pub sky: TexId,
    pub addresses: [u64; 4],
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

/// Pushes the four frame-data addresses, the vertex address, the view index and the G-buffer
/// slots (64 bytes, the `Push` block of `common.glsl`).
fn push_frame<C: CommandEncoder>(
    cmd: &mut C,
    addresses: &[u64; 4],
    vertex_address: u64,
    view_index: u32,
    gbuf: [u32; 4],
) {
    let mut pc = [0u8; 64];
    for (chunk, a) in pc
        .chunks_mut(8)
        .zip(addresses.iter().chain([&vertex_address]))
    {
        chunk.copy_from_slice(&a.to_le_bytes());
    }
    pc[40..44].copy_from_slice(&view_index.to_le_bytes());
    for (chunk, g) in pc[48..].chunks_mut(4).zip(gbuf) {
        chunk.copy_from_slice(&g.to_le_bytes());
    }
    cmd.push_constants(0, &pc);
}

/// Draws a list sorted by variant, switching pipelines only when the variant changes. The
/// first instance doubles as the instance array index (`gl_InstanceIndex`).
fn draw_list<C: CommandEncoder>(cmd: &mut C, g: &GeometryInputs, view_index: u32) {
    let mut bound = usize::MAX;
    for d in g.draws {
        if d.variant != bound {
            cmd.bind_pipeline(g.pipes[d.variant]);
            bound = d.variant;
        }
        push_frame(cmd, &g.addresses, d.vertex_address, view_index, [0; 4]);
        cmd.bind_index_buffer(d.index_buffer, 0, IndexFormat::U32);
        cmd.draw_indexed(d.index_count, 1, 0, 0, d.instance);
    }
}

const DEPTH_LOAD: DepthTarget = DepthTarget {
    load: LoadOp::Load,
    store: StoreOp::Store,
    clear: 0.0,
};

/// Depth-only pass into one shadow map (a cascade or a spot light).
pub(crate) fn shadow_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ShadowInputs<'a>) {
    let pass = Pass::new(i.name).label_color([0.4, 0.4, 0.4, 1.0]).depth(
        i.texture,
        DepthTarget {
            load: LoadOp::Clear,
            store: StoreOp::Store,
            clear: 1.0,
        },
    );
    b.pass(pass, move |ctx| {
        draw_list(ctx.cmd, &i.geometry, i.view_index);
        Ok(())
    });
}

/// Depth prepass: opaque and masked geometry, depth (reverse-Z, clear 0) plus view-space
/// normals.
pub(crate) fn prepass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: PrepassInputs<'a>) {
    let pass = Pass::new("prepass")
        .label_color([0.5, 0.5, 0.8, 1.0])
        .color(i.normal, ColorTarget::clear([0.0; 4]))
        .depth(
            i.depth,
            DepthTarget {
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: 0.0, // reverse-Z: far = 0
            },
        );
    b.pass(pass, move |ctx| {
        draw_list(ctx.cmd, &i.geometry, 0);
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

/// Forward opaque + masked lighting into the HDR target; depth comes from the prepass
/// (compare `GreaterOrEqual`, no depth write).
pub(crate) fn forward_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ShadedInputs<'a>) {
    let mut pass = Pass::new("forward")
        .label_color([0.3, 0.7, 0.3, 1.0])
        .color(i.hdr, ColorTarget::clear([0.02, 0.03, 0.05, 1.0]))
        .depth(i.depth, DEPTH_LOAD);
    for r in &i.reads {
        pass = pass.read(*r);
    }
    b.pass(pass, move |ctx| {
        draw_list(ctx.cmd, &i.geometry, 0);
        Ok(())
    });
}

/// Blended geometry after the opaque lighting and the sky: sorted back to front, depth tested,
/// no depth write.
pub(crate) fn transparent_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ShadedInputs<'a>) {
    let mut pass = Pass::new("transparent")
        .label_color([0.3, 0.7, 0.7, 1.0])
        .color(
            i.hdr,
            ColorTarget {
                load: LoadOp::Load,
                ..ColorTarget::dont_care()
            },
        )
        .depth(i.depth, DEPTH_LOAD);
    for r in &i.reads {
        pass = pass.read(*r);
    }
    b.pass(pass, move |ctx| {
        draw_list(ctx.cmd, &i.geometry, 0);
        Ok(())
    });
}

/// Deferred G-buffer fill (albedo + metallic, normal + roughness, emissive + AO).
pub(crate) fn gbuffer_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: GbufferInputs<'a>) {
    let mut pass = Pass::new("gbuffer")
        .label_color([0.6, 0.4, 0.7, 1.0])
        .depth(i.depth, DEPTH_LOAD);
    for t in i.targets {
        pass = pass.color(t, ColorTarget::clear([0.0; 4]));
    }
    b.pass(pass, move |ctx| {
        draw_list(ctx.cmd, &i.geometry, 0);
        Ok(())
    });
}

/// Deferred lighting: fullscreen triangle over the G-buffer into the HDR target.
pub(crate) fn lighting_pass<D: Device>(b: &mut GraphBuilder<'_, D>, i: LightingInputs) {
    let mut pass = Pass::new("lighting")
        .label_color([0.9, 0.8, 0.3, 1.0])
        .color(i.hdr, ColorTarget::clear([0.02, 0.03, 0.05, 1.0]))
        .read(i.depth);
    for t in i.gbuffer {
        pass = pass.read(t);
    }
    for r in &i.reads {
        pass = pass.read(*r);
    }
    b.pass(pass, move |ctx| {
        let gbuf = [
            ctx.texture_index(i.gbuffer[0])?,
            ctx.texture_index(i.gbuffer[1])?,
            ctx.texture_index(i.gbuffer[2])?,
            ctx.texture_index(i.depth)?,
        ];
        ctx.cmd.bind_pipeline(i.pipeline);
        push_frame(ctx.cmd, &i.addresses, 0, 0, gbuf);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Sky into the pixels no geometry covered (reverse-Z far plane, no depth write).
pub(crate) fn skybox_pass<D: Device>(b: &mut GraphBuilder<'_, D>, i: SkyboxInputs) {
    let pass = Pass::new("skybox")
        .label_color([0.4, 0.6, 0.9, 1.0])
        .read(i.sky)
        .color(
            i.hdr,
            ColorTarget {
                load: LoadOp::Load,
                ..ColorTarget::dont_care()
            },
        )
        .depth(i.depth, DEPTH_LOAD);
    b.pass(pass, move |ctx| {
        ctx.cmd.bind_pipeline(i.pipeline);
        push_frame(ctx.cmd, &i.addresses, 0, 0, [0; 4]);
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
