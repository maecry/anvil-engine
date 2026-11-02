//! Post-processing passes (ADR-0012 §11): SSAO, TAA resolve and bloom. Each is a pass function,
//! so a disabled feature simply adds no passes and the graph stays valid.

use render_graph::{ColorTarget, GraphBuilder, Pass, TexId};
use rhi::{CommandEncoder, Device, PipelineHandle};

use crate::passes::{NO_SSAO, push_frame};

/// Bloom pyramid levels (each half the size of the previous, starting at half resolution).
pub(crate) const BLOOM_LEVELS: usize = 6;

/// Half-resolution hemisphere SSAO from the prepass depth and view normals.
pub(crate) fn ssao_pass<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    pipeline: PipelineHandle,
    depth: TexId,
    normal: TexId,
    ao_half: TexId,
    addresses: [u64; 4],
) {
    let pass = Pass::new("ssao")
        .label_color([0.5, 0.5, 0.5, 1.0])
        .read(depth)
        .read(normal)
        .color(ao_half, ColorTarget::dont_care());
    b.pass(pass, move |ctx| {
        let gbuf = [ctx.texture_index(depth)?, ctx.texture_index(normal)?, 0, 0];
        ctx.cmd.bind_pipeline(pipeline);
        push_frame(ctx.cmd, &addresses, 0, 0, NO_SSAO, gbuf);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Depth-aware blur fused with the upsample to full resolution.
pub(crate) fn ssao_blur_pass<D: Device>(
    b: &mut GraphBuilder<'_, D>,
    pipeline: PipelineHandle,
    ao_half: TexId,
    depth: TexId,
    ao: TexId,
    addresses: [u64; 4],
) {
    let pass = Pass::new("ssao.blur")
        .label_color([0.5, 0.5, 0.5, 1.0])
        .read(ao_half)
        .read(depth)
        .color(ao, ColorTarget::dont_care());
    b.pass(pass, move |ctx| {
        let gbuf = [ctx.texture_index(ao_half)?, ctx.texture_index(depth)?, 0, 0];
        ctx.cmd.bind_pipeline(pipeline);
        push_frame(ctx.cmd, &addresses, 0, 0, NO_SSAO, gbuf);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Inputs of [`taa_pass`].
pub(crate) struct TaaInputs {
    pub pipeline: PipelineHandle,
    pub hdr: TexId,
    pub depth: TexId,
    pub velocity: TexId,
    pub history_prev: TexId,
    pub history_curr: TexId,
    /// No usable history (first frame, resize, TAA was off): the history is cleared first and
    /// the resolve outputs the current frame.
    pub reset: bool,
    pub addresses: [u64; 4],
}

/// Temporal resolve into `history_curr`: YCoCg neighborhood clamp, reprojection with the
/// velocity target (camera reprojection for the sky), depth-based disocclusion rejection.
pub(crate) fn taa_pass<D: Device>(b: &mut GraphBuilder<'_, D>, i: TaaInputs) {
    if i.reset {
        // Gives the unused history a defined state so the resolve may sample it.
        b.pass(
            Pass::new("taa.reset").color(i.history_prev, ColorTarget::clear([0.0; 4])),
            |_| Ok(()),
        );
    }
    let pass = Pass::new("taa")
        .label_color([0.9, 0.5, 0.5, 1.0])
        .read(i.hdr)
        .read(i.depth)
        .read(i.velocity)
        .read(i.history_prev)
        .color(i.history_curr, ColorTarget::dont_care());
    b.pass(pass, move |ctx| {
        let gbuf = [
            ctx.texture_index(i.hdr)?,
            ctx.texture_index(i.depth)?,
            ctx.texture_index(i.velocity)?,
            ctx.texture_index(i.history_prev)?,
        ];
        ctx.cmd.bind_pipeline(i.pipeline);
        push_frame(ctx.cmd, &i.addresses, 0, u32::from(i.reset), NO_SSAO, gbuf);
        ctx.cmd.draw(3, 1, 0, 0);
        Ok(())
    });
}

/// Bloom pyramid textures: `down[i]` is level `i` (size halves each level), `up[i]` the
/// upsampled result at level `i` for `i < BLOOM_LEVELS - 1`.
pub(crate) struct BloomTargets {
    pub down: [(TexId, (u32, u32)); BLOOM_LEVELS],
    pub up: [(TexId, (u32, u32)); BLOOM_LEVELS - 1],
}

/// Inputs of [`bloom_passes`].
pub(crate) struct BloomInputs {
    pub down_pipeline: PipelineHandle,
    pub up_pipeline: PipelineHandle,
    pub source: TexId,
    pub source_extent: (u32, u32),
    pub sampler_index: u32,
    pub targets: BloomTargets,
}

pub(crate) const DOWN_NAMES: [&str; BLOOM_LEVELS] = [
    "bloom.down0",
    "bloom.down1",
    "bloom.down2",
    "bloom.down3",
    "bloom.down4",
    "bloom.down5",
];
pub(crate) const UP_NAMES: [&str; BLOOM_LEVELS - 1] = [
    "bloom.up0",
    "bloom.up1",
    "bloom.up2",
    "bloom.up3",
    "bloom.up4",
];

fn push_words<C: CommandEncoder>(cmd: &mut C, words: &[u32]) {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    cmd.push_constants(0, &bytes);
}

/// Downsample chain (13-tap, Karis average on the first level) and tent upsample back to
/// level 0, all in separate textures. The result is `targets.up[0]`.
pub(crate) fn bloom_passes<D: Device>(b: &mut GraphBuilder<'_, D>, i: BloomInputs) {
    let t = i.targets;
    let sampler = i.sampler_index;
    let f = f32::to_bits;
    for (level, name) in DOWN_NAMES.iter().enumerate() {
        let (src, src_extent) = if level == 0 {
            (i.source, i.source_extent)
        } else {
            t.down[level - 1]
        };
        let (dst, dst_extent) = t.down[level];
        let pass = Pass::new(*name)
            .label_color([0.9, 0.9, 0.5, 1.0])
            .read(src)
            .color(dst, ColorTarget::dont_care());
        let pipeline = i.down_pipeline;
        b.pass(pass, move |ctx| {
            let src_index = ctx.texture_index(src)?;
            ctx.cmd.bind_pipeline(pipeline);
            push_words(
                ctx.cmd,
                &[
                    src_index,
                    sampler,
                    f(1.0 / dst_extent.0 as f32),
                    f(1.0 / dst_extent.1 as f32),
                    f(1.0 / src_extent.0 as f32),
                    f(1.0 / src_extent.1 as f32),
                    u32::from(level == 0),
                ],
            );
            ctx.cmd.draw(3, 1, 0, 0);
            Ok(())
        });
    }
    for level in (0..BLOOM_LEVELS - 1).rev() {
        let (coarse, coarse_extent) = if level == BLOOM_LEVELS - 2 {
            t.down[level + 1]
        } else {
            t.up[level + 1]
        };
        let (fine, _) = t.down[level];
        let (dst, dst_extent) = t.up[level];
        let pass = Pass::new(UP_NAMES[level])
            .label_color([0.9, 0.9, 0.5, 1.0])
            .read(coarse)
            .read(fine)
            .color(dst, ColorTarget::dont_care());
        let pipeline = i.up_pipeline;
        b.pass(pass, move |ctx| {
            let (c, fi) = (ctx.texture_index(coarse)?, ctx.texture_index(fine)?);
            ctx.cmd.bind_pipeline(pipeline);
            push_words(
                ctx.cmd,
                &[
                    c,
                    fi,
                    sampler,
                    0,
                    f(1.0 / dst_extent.0 as f32),
                    f(1.0 / dst_extent.1 as f32),
                    f(1.0 / coarse_extent.0 as f32),
                    f(1.0 / coarse_extent.1 as f32),
                ],
            );
            ctx.cmd.draw(3, 1, 0, 0);
            Ok(())
        });
    }
}

/// Frames in the jitter cycle.
pub(crate) const JITTER_PERIOD: u32 = 8;

/// Radical inverse of `index` in `base` (the Halton sequence), in `[0, 1)`.
pub(crate) fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// Sub-pixel jitter in pixels, each in `(-0.5, 0.5)`, Halton(2, 3) over [`JITTER_PERIOD`] frames.
pub(crate) fn jitter_pixels(frame: u32) -> (f32, f32) {
    let i = frame % JITTER_PERIOD + 1;
    (halton(i, 2) - 0.5, halton(i, 3) - 0.5)
}

/// Shifts the projected image by `jitter` pixels of a `size` viewport (Y down, as projected).
pub(crate) fn jitter_projection(
    mut proj: foundation::math::Mat4,
    jitter: (f32, f32),
    size: (u32, u32),
) -> foundation::math::Mat4 {
    // ndc += -z_axis.xy for a perspective matrix (clip.w = -view z), see the unit test.
    proj.z_axis.x -= jitter.0 * 2.0 / size.0 as f32;
    proj.z_axis.y -= jitter.1 * 2.0 / size.1 as f32;
    proj
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::math::Vec3;

    #[test]
    fn halton_sequence() {
        let b2: Vec<f32> = (1..=4).map(|i| halton(i, 2)).collect();
        assert_eq!(b2, [0.5, 0.25, 0.75, 0.125]);
        let b3: Vec<f32> = (1..=3).map(|i| halton(i, 3)).collect();
        assert!((b3[0] - 1.0 / 3.0).abs() < 1e-6 && (b3[1] - 2.0 / 3.0).abs() < 1e-6);
        assert!((b3[2] - 1.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn jitter_stays_within_a_pixel_and_cycles() {
        for f in 0..JITTER_PERIOD * 2 {
            let (x, y) = jitter_pixels(f);
            assert!(x.abs() < 0.5 && y.abs() < 0.5);
            assert_eq!(jitter_pixels(f), jitter_pixels(f + JITTER_PERIOD));
        }
        // All distinct within a period.
        let pts: Vec<_> = (0..JITTER_PERIOD).map(jitter_pixels).collect();
        for (i, a) in pts.iter().enumerate() {
            assert!(pts[i + 1..].iter().all(|b| a != b));
        }
    }

    #[test]
    fn projection_jitter_moves_the_image_by_the_requested_pixels() {
        let proj = foundation::math::camera::rh::proj::vulkan::perspective(1.0, 1.5, 0.1, 100.0);
        let size = (1000u32, 500u32);
        let p = Vec3::new(0.3, -0.2, -5.0);
        let a = proj.project_point3(p);
        let b = jitter_projection(proj, (0.25, -0.5), size).project_point3(p);
        // One pixel is 2 / size in NDC.
        assert!((b.x - a.x - 0.25 * 2.0 / 1000.0).abs() < 1e-6);
        assert!((b.y - a.y + 0.5 * 2.0 / 500.0).abs() < 1e-6);
        // Independent of depth (a pure image shift).
        let far = Vec3::new(0.3, -0.2, -50.0);
        let d = jitter_projection(proj, (0.25, -0.5), size)
            .project_point3(far)
            .x
            - proj.project_point3(far).x;
        assert!((d - 0.25 * 2.0 / 1000.0).abs() < 1e-6);
    }
}
