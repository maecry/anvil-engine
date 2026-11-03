//! GPU-driven culling (ADR-0013 §6): the compute passes of the two-phase occlusion pipeline, the
//! depth pyramid and the layout of the indirect command buffer. The shaders are
//! `cull_camera.comp`, `cull_shadow.comp` and `hiz.comp`; `gpu.rs` mirrors their structs.
//!
//! Frame order: `cull.reset`, `cull.shadow`, `cull.camera.p1` (last frame's visible set), the
//! prepass for those draws, `hiz.*` (pyramid from that depth), `cull.camera.p2` (everything else
//! against the pyramid), the prepass for the newly visible draws, then the shading passes draw
//! both command lists.

use render_graph::{BufId, GraphBuilder, Pass, RenderGraph, TexId};
use rhi::{
    BufferHandle, BufferState, CommandEncoder, Device, Format, RhiError, TextureDesc,
    TextureHandle, TextureState, TextureUsage, TextureViewDesc,
};

use crate::gpu::{CULL_VIEWS, STAT_WORDS, STATS, VARIANTS};

/// Bytes of one `VkDrawIndexedIndirectCommand`.
pub(crate) const COMMAND_BYTES: u64 = 20;

/// Where each (view, pipeline variant) command region starts. A region can hold as many commands
/// as there are opaque instances of its variant (each instance draws at most once per view).
/// Only the views active this frame get regions (the camera views and the shadow views with a
/// light), so unused point-light faces cost no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CullLayout {
    /// Opaque instances per variant: the capacity of one region.
    pub caps: [u32; VARIANTS],
    prefix: [u32; VARIANTS],
    /// Commands per view.
    pub total: u32,
    /// Bit `v` set: view `v` has regions.
    active: u64,
}

impl CullLayout {
    /// `shadow_mask` has bit `k` set for shadow view `2 + k` (the `view_mask` of the culling
    /// parameters); both camera views are always active.
    pub fn new(caps: [u32; VARIANTS], shadow_mask: u32) -> Self {
        let mut prefix = [0; VARIANTS];
        for v in 1..VARIANTS {
            prefix[v] = prefix[v - 1] + caps[v - 1];
        }
        Self {
            caps,
            prefix,
            total: caps.iter().sum(),
            active: 0b11 | (u64::from(shadow_mask) << 2),
        }
    }

    /// First command of `(view, variant)`; inactive views point at region 0, which nothing
    /// writes for them (their counters stay 0, so drawing them is a no-op).
    pub fn region(&self, view: usize, variant: usize) -> u32 {
        if self.active & (1 << view) == 0 {
            return 0;
        }
        let rank = (self.active & ((1 << view) - 1)).count_ones();
        rank * self.total + self.prefix[variant]
    }

    /// Size of the command buffer, rounded up so small changes do not reallocate it.
    pub fn buffer_bytes(&self) -> u64 {
        (u64::from(self.total) * u64::from(self.active.count_ones()) * COMMAND_BYTES)
            .max(COMMAND_BYTES)
            .next_multiple_of(64 << 10)
    }

    /// The `region` table of `GpuCullParams`.
    pub fn regions(&self) -> [[u32; VARIANTS]; CULL_VIEWS] {
        std::array::from_fn(|view| std::array::from_fn(|v| self.region(view, v)))
    }
}

/// Counters of the culling passes of one frame, read back with the frame slot's delay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullStats {
    /// Frame counter of the measured frame (`0` until the first readback).
    pub frame: u64,
    /// Instances that passed frustum and occlusion tests (the camera's final visible set).
    pub visible: u32,
    /// Instances outside the camera frustum.
    pub frustum_culled: u32,
    /// Instances inside the frustum but hidden behind the depth pyramid.
    pub occlusion_culled: u32,
    /// Triangles of the camera's indirect draws after LOD selection (both phases).
    pub triangles: u32,
    /// Indirect draw commands the camera culling emitted (both phases).
    pub camera_draws: u32,
    /// Of those, the ones phase 2 added (visible now, not drawn from last frame's set).
    pub phase2_draws: u32,
    /// Indirect draw commands emitted for all shadow views.
    pub shadow_draws: u32,
}

impl CullStats {
    /// Parses the statistics words of the counts buffer (`bytes` = the whole buffer).
    pub fn parse(frame: u64, bytes: &[u8]) -> Self {
        let w = |i: usize| {
            let o = (STATS + i) * 4;
            u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        };
        const { assert!(STAT_WORDS >= 7) };
        Self {
            frame,
            visible: w(0),
            frustum_culled: w(1),
            occlusion_culled: w(2),
            triangles: w(3),
            camera_draws: w(4),
            phase2_draws: w(5),
            shadow_draws: w(6),
        }
    }
}

/// Compute pipelines of the culling pipeline.
pub(crate) struct CullPipelines {
    pub camera: rhi::PipelineHandle,
    pub shadow: rhi::PipelineHandle,
    pub hiz: rhi::PipelineHandle,
}

/// The depth pyramid: an R32Float mip chain (mip 0 is half the depth resolution) where every
/// texel holds the farthest depth (the minimum, reverse-Z) of its footprint. Each mip has a view
/// of its own that the graph tracks as a texture (this is the subresource state tracking of
/// ADR-0013 §4, opted into for this texture only); the whole chain is sampled by `cull_camera`.
pub(crate) struct Pyramid {
    pub parent: TextureHandle,
    pub mips: Vec<TextureHandle>,
    pub sizes: Vec<(u32, u32)>,
    /// Bindless sampled slot of the whole chain.
    pub slot: u32,
    /// Size of the depth buffer it was built for.
    pub source: (u32, u32),
}

/// Mip sizes of a pyramid over a `w` x `h` depth buffer.
pub(crate) fn pyramid_sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let (w0, h0) = ((w / 2).max(1), (h / 2).max(1));
    let levels = w0.max(h0).ilog2() + 1;
    (0..levels)
        .map(|l| ((w0 >> l).max(1), (h0 >> l).max(1)))
        .collect()
}

impl Pyramid {
    pub fn new<D: Device>(
        device: &D,
        graph: &mut RenderGraph,
        w: u32,
        h: u32,
    ) -> Result<Self, RhiError> {
        let sizes = pyramid_sizes(w, h);
        let usage = TextureUsage::SAMPLED | TextureUsage::STORAGE;
        let parent = device.create_texture(&TextureDesc {
            label: Some("hiz"),
            width: sizes[0].0,
            height: sizes[0].1,
            format: Format::R32Float,
            usage,
            mip_levels: sizes.len() as u32,
            array_layers: 1,
            cube: false,
        })?;
        let mut me = Self {
            parent,
            mips: Vec::new(),
            slot: 0,
            sizes,
            source: (w, h),
        };
        for level in 0..me.sizes.len() as u32 {
            let view = device.create_texture_view(
                parent,
                &TextureViewDesc {
                    label: Some("hiz.mip"),
                    base_mip: level,
                    mip_count: 1,
                    base_layer: 0,
                    layer_count: 1,
                    usage,
                },
            );
            match view {
                Ok(v) => {
                    graph.track(v, TextureState::Undefined);
                    me.mips.push(v);
                }
                Err(e) => {
                    me.destroy(device, graph);
                    return Err(e);
                }
            }
        }
        me.slot = device.texture_index(parent)?;
        Ok(me)
    }

    pub fn destroy<D: Device>(self, device: &D, graph: &mut RenderGraph) {
        for v in self.mips {
            graph.forget(v);
            device.destroy_texture(v);
        }
        device.destroy_texture(self.parent);
    }
}

fn address_bytes(pc: &mut [u8], at: usize, v: u64) {
    pc[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Buffers and addresses shared by the culling passes.
#[derive(Clone, Copy)]
pub(crate) struct CullBuffers {
    pub instances: BufId,
    pub visible: BufId,
    pub cmds: BufId,
    pub counts: BufId,
    /// Ring address of the frame's `GpuCullParams`.
    pub params: u64,
    pub instance_count: u32,
}

/// Zeroes the draw counters and statistics.
pub(crate) fn reset_pass<D: Device>(b: &mut GraphBuilder<'_, D>, counts: BufId) {
    let pass = Pass::new("cull.reset")
        .label_color([0.6, 0.3, 0.3, 1.0])
        .write_buffer(counts, BufferState::TransferDst);
    b.pass(pass, move |ctx| {
        ctx.cmd
            .fill_buffer(ctx.buffer(counts), 0, crate::gpu::COUNTS_BYTES, 0);
        Ok(())
    });
}

fn groups(n: u32, size: u32) -> u32 {
    n.div_ceil(size).max(1)
}

/// Frustum-only culling and LOD selection for every shadow view (command regions 2..33).
pub(crate) fn shadow_cull_pass<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    pipeline: rhi::PipelineHandle,
    c: CullBuffers,
) {
    let pass = Pass::new("cull.shadow")
        .label_color([0.6, 0.3, 0.3, 1.0])
        .read_buffer(c.instances, BufferState::ShaderRead)
        .modify_buffer(c.counts, BufferState::ShaderWrite)
        .write_buffer(c.cmds, BufferState::ShaderWrite);
    b.pass(pass, move |ctx| {
        let mut pc = [0u8; 24];
        address_bytes(&mut pc, 0, c.params);
        address_bytes(&mut pc, 8, ctx.buffer_address(c.cmds)?);
        address_bytes(&mut pc, 16, ctx.buffer_address(c.counts)?);
        ctx.cmd.bind_pipeline(pipeline);
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.dispatch(groups(c.instance_count, 64), 1, 1);
        Ok(())
    });
}

/// One phase of the camera culling. Phase 0 needs last frame's visible set only; phase 1 also
/// samples every mip of the pyramid (`mips`) and updates the visible set.
pub(crate) fn camera_cull_pass<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    pipeline: rhi::PipelineHandle,
    c: CullBuffers,
    phase: u32,
    mips: &[TexId],
) {
    let mut pass = Pass::new(if phase == 0 {
        "cull.camera.p1"
    } else {
        "cull.camera.p2"
    })
    .label_color([0.6, 0.3, 0.3, 1.0])
    .read_buffer(c.instances, BufferState::ShaderRead)
    .modify_buffer(c.counts, BufferState::ShaderWrite)
    .write_buffer(c.cmds, BufferState::ShaderWrite);
    pass = if phase == 0 {
        pass.read_buffer(c.visible, BufferState::ShaderRead)
    } else {
        // Reads the previous set and writes the new one.
        pass.modify_buffer(c.visible, BufferState::ShaderWrite)
    };
    for m in mips {
        pass = pass.read(*m);
    }
    b.pass(pass, move |ctx| {
        let mut pc = [0u8; 28];
        address_bytes(&mut pc, 0, c.params);
        address_bytes(&mut pc, 8, ctx.buffer_address(c.cmds)?);
        address_bytes(&mut pc, 16, ctx.buffer_address(c.counts)?);
        pc[24..28].copy_from_slice(&phase.to_le_bytes());
        ctx.cmd.bind_pipeline(pipeline);
        ctx.cmd.push_constants(0, &pc);
        ctx.cmd.dispatch(groups(c.instance_count, 64), 1, 1);
        Ok(())
    });
}

/// Builds the pyramid from `depth`: one dispatch per mip, mip 0 from the depth buffer, mip k
/// from mip k-1. Each pass writes one mip view and samples the previous one.
pub(crate) fn hiz_passes<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    pipeline: rhi::PipelineHandle,
    depth: TexId,
    depth_extent: (u32, u32),
    mips: &[TexId],
    sizes: &[(u32, u32)],
) {
    for level in 0..mips.len() {
        let src = if level == 0 { depth } else { mips[level - 1] };
        let src_size = if level == 0 {
            depth_extent
        } else {
            sizes[level - 1]
        };
        let (dst, dst_size) = (mips[level], sizes[level]);
        let pass = Pass::new(format!("hiz.{level}"))
            .label_color([0.5, 0.3, 0.6, 1.0])
            .read(src)
            .write_storage(dst);
        b.pass(pass, move |ctx| {
            let mut pc = [0u8; 24];
            pc[0..4].copy_from_slice(&ctx.texture_index(src)?.to_le_bytes());
            pc[4..8].copy_from_slice(&ctx.device.storage_index(ctx.texture(dst))?.to_le_bytes());
            for (i, v) in [src_size.0, src_size.1, dst_size.0, dst_size.1]
                .into_iter()
                .enumerate()
            {
                pc[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            ctx.cmd.bind_pipeline(pipeline);
            ctx.cmd.push_constants(0, &pc);
            ctx.cmd
                .dispatch(groups(dst_size.0, 8), groups(dst_size.1, 8), 1);
            Ok(())
        });
    }
}

/// Copies the counters into the frame slot's readback buffer (read `frames_in_flight` later).
pub(crate) fn stats_pass<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    counts: BufId,
    readback: BufferHandle,
) {
    let pass = Pass::new("cull.stats")
        .label_color([0.6, 0.3, 0.3, 1.0])
        .read_buffer(counts, BufferState::TransferSrc)
        .side_effect();
    b.pass(pass, move |ctx| {
        ctx.cmd
            .copy_buffer_to_buffer(ctx.buffer(counts), 0, readback, 0, crate::gpu::COUNTS_BYTES);
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_regions_do_not_overlap() {
        // Camera views, cascade 1, one spot light and the last point face are active.
        let mask = 0b10 | (1 << 4) | (1 << 31);
        let l = CullLayout::new([5, 0, 3, 2], mask);
        assert_eq!(l.total, 10);
        let active = [0, 1, 3, 6, CULL_VIEWS - 1];
        let mut spans = Vec::new();
        for view in active {
            for v in 0..VARIANTS {
                let s = l.region(view, v);
                spans.push((s, s + l.caps[v]));
            }
        }
        spans.sort_unstable();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "{w:?}");
        }
        assert_eq!(spans.last().unwrap().1, l.total * active.len() as u32);
        assert_eq!(l.regions()[1][2], 10 + 5);
        // An inactive view has no region of its own.
        assert_eq!(l.region(2, 3), 0);
        assert!(CullLayout::new([0; 4], 0).buffer_bytes() >= COMMAND_BYTES);
        assert_eq!(l.buffer_bytes() % (64 << 10), 0);
    }

    #[test]
    fn pyramid_levels_cover_down_to_one_texel() {
        let s = pyramid_sizes(1920, 1080);
        assert_eq!((s[0], s.len()), ((960, 540), 10));
        assert_eq!(*s.last().unwrap(), (1, 1));
        // Every level is the floor-halved previous one, the shape the shader's odd-size rule expects.
        assert!(
            s.windows(2)
                .all(|w| w[1] == ((w[0].0 / 2).max(1), (w[0].1 / 2).max(1)))
        );
        assert_eq!(pyramid_sizes(1, 1), [(1, 1)]);
        assert_eq!(pyramid_sizes(1280, 721)[0], (640, 360));
    }

    #[test]
    fn stats_parse_reads_the_tail_words() {
        let mut bytes = vec![0u8; crate::gpu::COUNTS_BYTES as usize];
        for i in 0..STAT_WORDS {
            let o = (STATS + i) * 4;
            bytes[o..o + 4].copy_from_slice(&(100 + i as u32).to_le_bytes());
        }
        let s = CullStats::parse(7, &bytes);
        assert_eq!(
            (
                s.frame,
                s.visible,
                s.frustum_culled,
                s.occlusion_culled,
                s.triangles
            ),
            (7, 100, 101, 102, 103)
        );
        assert_eq!(
            (s.camera_draws, s.phase2_draws, s.shadow_draws),
            (104, 105, 106)
        );
    }
}
