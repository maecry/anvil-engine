//! Renderer passes as render-graph nodes (ADR-0012 §8). The graph owns barriers, rendering
//! scopes, viewport and scissor; the bodies only bind and draw.

use render_graph::{BufId, ColorTarget, DepthTarget, GraphBuilder, Pass, PassContext, TexId};
use rhi::{
    BufferHandle, BufferState, CommandEncoder, Device, IndexFormat, LoadOp, PipelineHandle,
    RhiError, StoreOp,
};

use crate::gpu::VARIANTS;
use crate::gpu_cull::{COMMAND_BYTES, CullLayout};

/// One indexed draw of level of detail 0 (CPU path and blended objects).
#[derive(Clone, Copy)]
pub(crate) struct Draw {
    /// First index and count of the mesh in the arena's index buffer.
    pub first_index: u32,
    pub index_count: u32,
    /// Base vertex of the mesh in the arena's vertex buffer.
    pub vertex_offset: i32,
    /// Slot in the persistent instance buffer (`gl_InstanceIndex`).
    pub instance: u32,
    /// Pipeline variant: `mask * 2 + double_sided` (blended draws: `double_sided`).
    pub variant: usize,
    /// View depth of the bounding sphere center, the sort key.
    pub depth: f32,
}

/// `ssao_index` push value meaning "no AO texture".
pub(crate) const NO_SSAO: u32 = u32::MAX;

/// One pipeline per [`Draw::variant`].
pub(crate) type Pipes = [PipelineHandle; 4];

/// The shared mesh arena every draw reads: one vertex address, one index buffer.
#[derive(Clone, Copy)]
pub(crate) struct ArenaRef {
    pub vertex_address: u64,
    pub indices: BufferHandle,
}

/// Indirect command lists produced by the culling passes.
#[derive(Clone, Copy)]
pub(crate) struct GpuDraws<'a> {
    pub cmds: BufId,
    pub counts: BufId,
    pub layout: CullLayout,
    /// Culling views whose commands this pass draws (see `cull.glsl`).
    pub views: &'a [usize],
}

/// Where a geometry pass gets its draws.
#[derive(Clone, Copy)]
pub(crate) enum DrawSource<'a> {
    /// One CPU-recorded draw per instance (the M4 path, and blended objects).
    Cpu(&'a [Draw]),
    /// GPU-culled indirect commands, one `draw_indexed_indirect_count` per variant and view.
    Gpu(GpuDraws<'a>),
}

/// Inputs shared by the geometry passes.
pub(crate) struct GeometryInputs<'a> {
    pub pipes: Pipes,
    /// BDA of frame, instance, light and material data.
    pub addresses: [u64; 4],
    pub arena: ArenaRef,
    /// The persistent instance buffer the vertex shaders read.
    pub instances: BufId,
    pub source: DrawSource<'a>,
}

impl GeometryInputs<'_> {
    /// Adds the buffer reads of this geometry to the pass declaration.
    fn declare(&self, pass: Pass) -> Pass {
        let pass = pass.read_buffer(self.instances, BufferState::ShaderRead);
        match self.source {
            DrawSource::Cpu(_) => pass,
            DrawSource::Gpu(g) => pass
                .read_buffer(g.cmds, BufferState::Indirect)
                .read_buffer(g.counts, BufferState::Indirect),
        }
    }
}

/// Inputs of [`shadow_pass`].
pub(crate) struct ShadowInputs<'a> {
    pub name: std::borrow::Cow<'static, str>,
    pub texture: TexId,
    /// 0..3 cascade, 4..7 spot, 8..31 point-light cube faces (`pc.view_index` in `shadow.vert`).
    pub view_index: u32,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`prepass`].
pub(crate) struct PrepassInputs<'a> {
    /// Pass name; `prepass.p2` (GPU culling phase 2) loads what `prepass` rendered.
    pub name: &'static str,
    pub phase2: bool,
    pub depth: TexId,
    pub normal: TexId,
    pub velocity: TexId,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`forward_pass`] and [`transparent_pass`].
pub(crate) struct ShadedInputs<'a> {
    pub hdr: TexId,
    pub depth: TexId,
    /// Textures the shaders sample through bindless indices (shadow maps, IBL).
    pub reads: Vec<TexId>,
    /// Full-resolution ambient occlusion multiplying the ambient term (forward pass only).
    pub ssao: Option<TexId>,
    pub geometry: GeometryInputs<'a>,
}

/// Inputs of [`debug_pass`].
pub(crate) struct DebugInputs {
    pub pipeline: PipelineHandle,
    pub hdr: TexId,
    pub depth: TexId,
    /// BDA of the frame data and of the debug vertices (both in the upload ring).
    pub frame_address: u64,
    pub vertex_address: u64,
    pub vertex_count: u32,
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
    pub ssao: Option<TexId>,
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
    /// Bloom texture (half resolution) added to `hdr` before tonemapping.
    pub bloom: Option<TexId>,
    pub bloom_strength: f32,
}

/// Pushes the four frame-data addresses, the vertex address, the view index and the G-buffer
/// slots (64 bytes, the `Push` block of `common.glsl`).
pub(crate) fn push_frame<C: CommandEncoder>(
    cmd: &mut C,
    addresses: &[u64; 4],
    vertex_address: u64,
    view_index: u32,
    ssao_index: u32,
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
    pc[44..48].copy_from_slice(&ssao_index.to_le_bytes());
    for (chunk, g) in pc[48..].chunks_mut(4).zip(gbuf) {
        chunk.copy_from_slice(&g.to_le_bytes());
    }
    cmd.push_constants(0, &pc);
}

/// Records the geometry of a pass: CPU draws sorted by variant (switching pipelines only when
/// the variant changes), or GPU-culled indirect lists. The first instance of a draw doubles as the
/// instance slot (`gl_InstanceIndex`); the arena's index buffer is bound once.
fn draw_geometry<D: Device>(
    ctx: &mut PassContext<'_, D>,
    g: &GeometryInputs,
    view_index: u32,
    ssao_index: u32,
) -> Result<(), RhiError> {
    push_frame(
        ctx.cmd,
        &g.addresses,
        g.arena.vertex_address,
        view_index,
        ssao_index,
        [0; 4],
    );
    match g.source {
        DrawSource::Cpu(draws) => {
            if draws.is_empty() {
                return Ok(());
            }
            ctx.cmd
                .bind_index_buffer(g.arena.indices, 0, IndexFormat::U32);
            let mut bound = usize::MAX;
            for d in draws {
                if d.variant != bound {
                    ctx.cmd.bind_pipeline(g.pipes[d.variant]);
                    bound = d.variant;
                }
                ctx.cmd
                    .draw_indexed(d.index_count, 1, d.first_index, d.vertex_offset, d.instance);
            }
        }
        DrawSource::Gpu(gd) => {
            let (cmds, counts) = (ctx.buffer(gd.cmds), ctx.buffer(gd.counts));
            ctx.cmd
                .bind_index_buffer(g.arena.indices, 0, IndexFormat::U32);
            for v in (0..VARIANTS).filter(|&v| gd.layout.caps[v] > 0) {
                ctx.cmd.bind_pipeline(g.pipes[v]);
                for &view in gd.views {
                    ctx.cmd.draw_indexed_indirect_count(
                        cmds,
                        u64::from(gd.layout.region(view, v)) * COMMAND_BYTES,
                        counts,
                        ((view * VARIANTS + v) * 4) as u64,
                        gd.layout.caps[v],
                        COMMAND_BYTES as u32,
                    );
                }
            }
        }
    }
    Ok(())
}

const DEPTH_LOAD: DepthTarget = DepthTarget {
    load: LoadOp::Load,
    store: StoreOp::Store,
    clear: 0.0,
};

/// Depth-only pass into one shadow map (a cascade or a spot light).
pub(crate) fn shadow_pass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: ShadowInputs<'a>) {
    let pass = Pass::new(i.name.clone())
        .label_color([0.4, 0.4, 0.4, 1.0])
        .depth(
            i.texture,
            DepthTarget {
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: 1.0,
            },
        );
    b.pass(i.geometry.declare(pass), move |ctx| {
        draw_geometry(ctx, &i.geometry, i.view_index, NO_SSAO)
    });
}

/// Depth prepass: opaque and masked geometry, depth (reverse-Z, clear 0), view-space normals
/// and screen-space velocity.
pub(crate) fn prepass<'a, D: Device>(b: &mut GraphBuilder<'a, D>, i: PrepassInputs<'a>) {
    let color = |clear| {
        if i.phase2 {
            ColorTarget {
                load: LoadOp::Load,
                ..ColorTarget::dont_care()
            }
        } else {
            ColorTarget::clear(clear)
        }
    };
    let pass = Pass::new(i.name)
        .label_color([0.5, 0.5, 0.8, 1.0])
        .color(i.normal, color([0.0; 4]))
        .color(i.velocity, color([0.0; 4]))
        .depth(
            i.depth,
            if i.phase2 {
                DEPTH_LOAD
            } else {
                DepthTarget {
                    load: LoadOp::Clear,
                    store: StoreOp::Store,
                    clear: 0.0, // reverse-Z: far = 0
                }
            },
        );
    b.pass(i.geometry.declare(pass), move |ctx| {
        draw_geometry(ctx, &i.geometry, 0, NO_SSAO)
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
    for r in i.reads.iter().chain(&i.ssao) {
        pass = pass.read(*r);
    }
    b.pass(i.geometry.declare(pass), move |ctx| {
        let ssao = i.ssao.map_or(Ok(NO_SSAO), |t| ctx.texture_index(t))?;
        draw_geometry(ctx, &i.geometry, 0, ssao)
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
    b.pass(i.geometry.declare(pass), move |ctx| {
        draw_geometry(ctx, &i.geometry, 0, NO_SSAO)
    });
}

/// Debug lines over the lit image: depth tested against the opaque depth, no depth write.
pub(crate) fn debug_pass<D: Device>(b: &mut GraphBuilder<'_, D>, i: DebugInputs) {
    let pass = Pass::new("debug.lines")
        .label_color([0.9, 0.2, 0.9, 1.0])
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
        let mut pc = [0u8; 16];
        pc[0..8].copy_from_slice(&i.frame_address.to_le_bytes());
        pc[8..16].copy_from_slice(&i.vertex_address.to_le_bytes());
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.draw(i.vertex_count, 1, 0, 0);
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
    b.pass(i.geometry.declare(pass), move |ctx| {
        draw_geometry(ctx, &i.geometry, 0, NO_SSAO)
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
    for r in i.reads.iter().chain(&i.ssao) {
        pass = pass.read(*r);
    }
    b.pass(pass, move |ctx| {
        let ssao = i.ssao.map_or(Ok(NO_SSAO), |t| ctx.texture_index(t))?;
        let gbuf = [
            ctx.texture_index(i.gbuffer[0])?,
            ctx.texture_index(i.gbuffer[1])?,
            ctx.texture_index(i.gbuffer[2])?,
            ctx.texture_index(i.depth)?,
        ];
        ctx.cmd.bind_pipeline(i.pipeline);
        push_frame(ctx.cmd, &i.addresses, 0, 0, ssao, gbuf);
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
        push_frame(ctx.cmd, &i.addresses, 0, 0, NO_SSAO, [0; 4]);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Tonemaps `hdr` (plus `bloom` scaled by `bloom_strength`) into `target`.
pub(crate) fn tonemap_pass<D: Device>(b: &mut GraphBuilder<'_, D>, t: TonemapInputs) {
    let mut pass = Pass::new("tonemap")
        .label_color([0.7, 0.5, 0.2, 1.0])
        .read(t.hdr)
        .color(t.target, ColorTarget::dont_care());
    if let Some(bloom) = t.bloom {
        pass = pass.read(bloom);
    }
    b.pass(pass, move |ctx| {
        let hdr_index = ctx.texture_index(t.hdr)?;
        // Without bloom the slot is any valid texture and the strength is 0.
        let (bloom_index, strength) = match t.bloom {
            Some(bloom) => (ctx.texture_index(bloom)?, t.bloom_strength),
            None => (hdr_index, 0.0),
        };
        ctx.cmd.bind_pipeline(t.pipeline);
        let mut pc = [0u8; 24];
        pc[0..4].copy_from_slice(&hdr_index.to_le_bytes());
        pc[4..8].copy_from_slice(&t.sampler_index.to_le_bytes());
        pc[8..12].copy_from_slice(&t.exposure.to_le_bytes());
        pc[12..16].copy_from_slice(&u32::from(t.encode_srgb).to_le_bytes());
        pc[16..20].copy_from_slice(&bloom_index.to_le_bytes());
        pc[20..24].copy_from_slice(&strength.to_le_bytes());
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}
