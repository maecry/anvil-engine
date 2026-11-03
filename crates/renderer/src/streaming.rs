//! Texture streaming (ADR-0013 §8): CPU mip chains and the residency planner.
//!
//! Every texture keeps its full mip chain on the CPU. Only the mips from a *base level* down are
//! resident on the GPU: a texture is a GPU image of `levels - base` mips whose level 0 is CPU level
//! `base`, so the sampler's LOD clamp to the resident chain is inherent (the image has no finer
//! mip than the resident one). Changing the base level reallocates the image (see LAYER-007 §18 for
//! why this was chosen over a full-chain allocation with partial uploads). The planner here is
//! pure: it decides the new base levels from the wanted ones, a VRAM budget and a per-frame
//! upload budget.

use rayon::prelude::*;

/// Textures whose largest side is at most this are always fully resident (streaming them would
/// save nothing).
pub(crate) const MIN_STREAM_DIM: u32 = 128;
/// A streamed texture starts with the first level whose largest side is at most this.
const INITIAL_DIM: u32 = 64;
/// The coarsest base level a streamed texture may drop to has a largest side of this many texels:
/// the tail is always resident.
const FLOOR_DIM: u32 = 16;
/// A texture may only be demoted (its base level raised) this many frames after its last change...
pub(crate) const DEMOTE_AFTER: u64 = 30;
/// ...and only when it wants at least this many levels coarser than it has (an object near a
/// level boundary keeps the finer level instead of flipping between two).
pub(crate) const DEMOTE_SLACK: u32 = 2;
/// Output pixels above which a level is filtered on the worker pool.
const PAR_PIXELS: usize = 1 << 16;

/// Mip levels of a `w` x `h` texture down to 1x1.
pub(crate) fn level_count(w: u32, h: u32) -> u32 {
    w.max(h).max(1).ilog2() + 1
}

/// Size of level `l` of a `w` x `h` texture.
pub(crate) fn level_size(w: u32, h: u32, l: u32) -> (u32, u32) {
    ((w >> l).max(1), (h >> l).max(1))
}

/// sRGB electro-optical transfer function of a byte.
fn srgb_to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Lookup tables of the sRGB-correct filter: byte to linear, and linear (4096 steps) to byte.
struct SrgbLuts {
    decode: [f32; 256],
    encode: Vec<u8>,
}

impl SrgbLuts {
    fn new() -> Self {
        Self {
            decode: std::array::from_fn(|i| srgb_to_linear(i as u8)),
            encode: (0..=4096)
                .map(|i| (linear_to_srgb(i as f32 / 4096.0) * 255.0 + 0.5) as u8)
                .collect(),
        }
    }

    fn encode(&self, linear: f32) -> u8 {
        self.encode[(linear.clamp(0.0, 1.0) * 4096.0 + 0.5) as usize]
    }
}

/// Box-filters level `src` (`sw` x `sh` RGBA8) to the next level. Each output texel averages the
/// 2x2 source block (clamped at odd edges); sRGB color channels are averaged in linear light,
/// alpha always linearly. shortcut: an odd dimension drops half a texel of weight at the last
/// row or column and normal maps are not renormalized; upgrade to a Kaiser filter when needed.
fn downsample(src: &[u8], sw: u32, sh: u32, srgb: bool, luts: &SrgbLuts) -> Vec<u8> {
    let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
    let mut dst = vec![0u8; dw as usize * dh as usize * 4];
    let row = |y: usize, out: &mut [u8]| {
        let (y0, y1) = (
            (2 * y).min(sh as usize - 1),
            (2 * y + 1).min(sh as usize - 1),
        );
        for x in 0..dw as usize {
            let (x0, x1) = (
                (2 * x).min(sw as usize - 1),
                (2 * x + 1).min(sw as usize - 1),
            );
            let at = |xx: usize, yy: usize| (yy * sw as usize + xx) * 4;
            let taps = [at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1)];
            for c in 0..4 {
                out[x * 4 + c] = if srgb && c < 3 {
                    let sum: f32 = taps.iter().map(|&t| luts.decode[src[t + c] as usize]).sum();
                    luts.encode(sum * 0.25)
                } else {
                    let sum: u32 = taps.iter().map(|&t| u32::from(src[t + c])).sum();
                    ((sum + 2) / 4) as u8
                };
            }
        }
    };
    if dst.len() / 4 >= PAR_PIXELS {
        dst.par_chunks_mut(dw as usize * 4)
            .enumerate()
            .for_each(|(y, out)| row(y, out));
    } else {
        for (y, out) in dst.chunks_mut(dw as usize * 4).enumerate() {
            row(y, out);
        }
    }
    dst
}

/// The full mip chain of a tightly packed RGBA8 image, level 0 first (level 0 is `rgba8` itself).
pub(crate) fn mip_chain(rgba8: &[u8], w: u32, h: u32, srgb: bool) -> Vec<Vec<u8>> {
    let luts = srgb.then(SrgbLuts::new);
    let luts = luts.unwrap_or_else(|| SrgbLuts {
        decode: [0.0; 256],
        encode: Vec::new(),
    });
    let mut chain = vec![rgba8.to_vec()];
    for l in 1..level_count(w, h) {
        let (sw, sh) = level_size(w, h, l - 1);
        let next = downsample(&chain[l as usize - 1], sw, sh, srgb, &luts);
        chain.push(next);
    }
    chain
}

/// Residency bounds of a texture: `(floor, initial)` base levels. `floor` is the coarsest base
/// level it may drop to (largest side at most [`FLOOR_DIM`]; 0 for textures at most
/// [`MIN_STREAM_DIM`], which stay fully resident); `initial` is the base level it is created with.
pub(crate) fn residency_bounds(w: u32, h: u32) -> (u32, u32) {
    let top = w.max(h).max(1).ilog2();
    if w.max(h) <= MIN_STREAM_DIM {
        return (0, 0);
    }
    (
        top.saturating_sub(FLOOR_DIM.ilog2()),
        top.saturating_sub(INITIAL_DIM.ilog2()),
    )
}

/// The base level wanted for a texture whose largest side is `size` texels and which an object
/// shows over `pixels` screen pixels: the coarsest level that still has one texel per pixel,
/// clamped to `0..=floor`.
pub(crate) fn wanted_base(size: u32, pixels: f32, floor: u32) -> u32 {
    if pixels.is_nan() || pixels <= 0.0 {
        return floor;
    }
    let ratio = size as f32 / pixels;
    if ratio <= 1.0 {
        return 0;
    }
    (ratio.log2().floor() as u32).min(floor)
}

/// One texture as the planner sees it.
pub(crate) struct PlanItem {
    /// Resident base level now.
    pub base: u32,
    /// Base level wanted (finest wanted level, `0` is full resolution).
    pub want: u32,
    /// Coarsest allowed base level.
    pub floor: u32,
    /// Frames since the last residency change.
    pub stable: u64,
    /// `suffix[b]` = bytes resident with base level `b` (`suffix[levels] == 0`).
    pub suffix: Vec<u64>,
}

/// The planner's result for one frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// `(item, new base level)` for every texture whose residency changes.
    pub changes: Vec<(usize, u32)>,
    /// Textures that want a finer level than they get this frame (upload budget).
    pub pending: u32,
    /// Bytes still to upload for them.
    pub pending_bytes: u64,
    /// Textures dropped to a coarser level.
    pub evictions: u32,
    /// Bytes of the uploads this plan makes.
    pub upload_bytes: u64,
    /// Resident bytes after the changes.
    pub resident_bytes: u64,
}

/// Decides this frame's residency changes.
///
/// 1. Each texture's target is its wanted level; a demotion (coarser) is held until the texture
///    has been stable for [`DEMOTE_AFTER`] frames and wants [`DEMOTE_SLACK`] levels less, so a
///    camera hovering at a boundary does not thrash.
/// 2. Over `budget`, the textures with the largest top level are demoted one level at a time until
///    the targets fit (or every texture sits at its floor); these demotions apply at once.
/// 3. Promotions are applied by benefit (levels gained) while their uploads fit `upload_budget`;
///    a texture that does not fit is promoted as far as the budget allows, and the first
///    promotion of a frame is always allowed in full (so a texture larger than the budget still
///    arrives).
pub(crate) fn plan(items: &[PlanItem], budget: u64, upload_budget: u64) -> Plan {
    let bytes_at = |it: &PlanItem, b: u32| it.suffix[b as usize];
    let mut target: Vec<u32> = items
        .iter()
        .map(|it| {
            let want = it.want.min(it.floor);
            // Demotions need a stable texture and at least two levels of slack.
            if want > it.base && (it.stable < DEMOTE_AFTER || want < it.base + DEMOTE_SLACK) {
                it.base
            } else {
                want
            }
        })
        .collect();
    let mut total: u64 = items
        .iter()
        .zip(&target)
        .map(|(it, &t)| bytes_at(it, t))
        .sum();
    while total > budget {
        let Some(i) = (0..items.len())
            .filter(|&i| target[i] < items[i].floor)
            .max_by_key(|&i| {
                (
                    bytes_at(&items[i], target[i]) - bytes_at(&items[i], target[i] + 1),
                    std::cmp::Reverse(i),
                )
            })
        else {
            break;
        };
        total -= bytes_at(&items[i], target[i]) - bytes_at(&items[i], target[i] + 1);
        target[i] += 1;
    }
    let mut plan = Plan::default();
    let mut promote: Vec<usize> = Vec::new();
    let mut final_base: Vec<u32> = items.iter().map(|it| it.base).collect();
    for (i, it) in items.iter().enumerate() {
        match target[i].cmp(&it.base) {
            std::cmp::Ordering::Greater => {
                final_base[i] = target[i];
                plan.evictions += 1;
                plan.upload_bytes += bytes_at(it, target[i]);
            }
            std::cmp::Ordering::Less => promote.push(i),
            std::cmp::Ordering::Equal => {}
        }
    }
    promote.sort_by_key(|&i| (std::cmp::Reverse(items[i].base - target[i]), i));
    let mut first = true;
    for i in promote {
        let it = &items[i];
        let remaining = upload_budget.saturating_sub(plan.upload_bytes);
        // The finest level between the target and the current one whose upload still fits.
        let fit = (target[i]..it.base).find(|&b| bytes_at(it, b) <= remaining);
        let chosen = match fit {
            Some(b) => Some(b),
            None if first => Some(target[i]),
            None => None,
        };
        if let Some(b) = chosen {
            final_base[i] = b;
            plan.upload_bytes += bytes_at(it, b);
            first = false;
        }
        if final_base[i] > target[i] {
            plan.pending += 1;
            plan.pending_bytes += bytes_at(it, target[i]);
        }
    }
    plan.changes = (0..items.len())
        .filter(|&i| final_base[i] != items[i].base)
        .map(|i| (i, final_base[i]))
        .collect();
    plan.resident_bytes = items
        .iter()
        .zip(&final_base)
        .map(|(it, &b)| bytes_at(it, b))
        .sum();
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(levels: u32, base: u32, want: u32, floor: u32, stable: u64) -> PlanItem {
        // 4 bytes per texel, square, top level 2^(levels-1).
        let mut suffix = vec![0u64; levels as usize + 1];
        for l in (0..levels).rev() {
            let side = 1u64 << (levels - 1 - l);
            suffix[l as usize] = suffix[l as usize + 1] + side * side * 4;
        }
        PlanItem {
            base,
            want,
            floor,
            stable,
            suffix,
        }
    }

    #[test]
    fn chain_has_every_level_and_averages_in_linear_light() {
        let (w, h) = (4u32, 2u32);
        let mut img = Vec::new();
        for i in 0..w * h {
            let v = if i % 2 == 0 { 255 } else { 0 };
            img.extend_from_slice(&[v, v, v, 200]);
        }
        let lin = mip_chain(&img, w, h, false);
        assert_eq!(lin.len(), 3);
        assert_eq!(lin[1].len(), 2 * 4);
        assert_eq!(lin[2].len(), 4);
        // Byte average of black and white: 128 (rounded), alpha stays 200.
        assert_eq!(&lin[1][..4], &[128, 128, 128, 200]);
        // Averaging 0 and 1 in linear light gives 0.5, which is 188 as an sRGB byte.
        let srgb = mip_chain(&img, w, h, true);
        assert!((i32::from(srgb[1][0]) - 188).abs() <= 1, "{}", srgb[1][0]);
        assert_eq!(srgb[1][3], 200);
        // Level 0 is the source; a 1x1 texture has one level.
        assert_eq!(srgb[0], img);
        assert_eq!(mip_chain(&[1, 2, 3, 4], 1, 1, true), [vec![1, 2, 3, 4]]);
    }

    #[test]
    fn bounds_and_wanted_levels() {
        assert_eq!(residency_bounds(64, 64), (0, 0));
        assert_eq!(residency_bounds(128, 128), (0, 0));
        // 2048: floor is 16 px (level 7), initial 64 px (level 5).
        assert_eq!(residency_bounds(2048, 1024), (7, 5));
        assert_eq!(level_count(2048, 1024), 12);
        assert_eq!(wanted_base(1024, 1024.0, 6), 0);
        assert_eq!(wanted_base(1024, 5000.0, 6), 0);
        assert_eq!(wanted_base(1024, 300.0, 6), 1);
        assert_eq!(wanted_base(1024, 100.0, 6), 3);
        assert_eq!(wanted_base(1024, 0.5, 6), 6);
        assert_eq!(wanted_base(1024, 0.0, 6), 6);
    }

    #[test]
    fn promotions_respect_the_upload_budget_but_never_starve() {
        // Two 1024 textures (level 0 = 4 MiB) at base 5, both want full resolution.
        let two = || [item(11, 5, 0, 6, 100), item(11, 5, 0, 6, 100)];
        let top = two()[0].suffix[0];
        // A budget nothing fits in: the first promotion still happens in full, the second waits.
        let p = plan(&two(), u64::MAX, 1);
        assert_eq!(p.changes, [(0, 0)]);
        assert_eq!((p.pending, p.pending_bytes, p.upload_bytes), (1, top, top));
        // A generous budget does both.
        let p = plan(&two(), u64::MAX, u64::MAX);
        assert_eq!((p.changes.len(), p.pending), (2, 0));
        // A mid budget promotes the first texture as far as it fits (level 2) and defers the rest.
        let mid = two()[0].suffix[2] + 1;
        let p = plan(&two(), u64::MAX, mid);
        assert_eq!(p.changes, [(0, 2)]);
        assert_eq!(p.pending, 2);
    }

    #[test]
    fn budget_forces_demotions_largest_first_and_demotion_is_delayed() {
        let big = item(11, 0, 0, 6, 100);
        let small = item(9, 0, 0, 4, 100);
        let total = big.suffix[0] + small.suffix[0];
        let p = plan(&[big, small], total - 1, u64::MAX);
        // Dropping the big texture one level saves the most.
        assert_eq!(p.changes, [(0, 1)]);
        assert_eq!(p.evictions, 1);
        assert!(p.resident_bytes < total);
        // Without pressure, a wanted demotion waits for the stable period.
        let young = item(11, 0, 3, 6, DEMOTE_AFTER - 1);
        assert!(plan(&[young], u64::MAX, u64::MAX).changes.is_empty());
        let old = item(11, 0, 3, 6, DEMOTE_AFTER);
        assert_eq!(plan(&[old], u64::MAX, u64::MAX).changes, [(0, 3)]);
        // One level of slack is not enough.
        let near = item(11, 2, 3, 6, 1000);
        assert!(plan(&[near], u64::MAX, u64::MAX).changes.is_empty());
        // An impossible budget stops at the floors.
        let p = plan(&[item(11, 0, 0, 6, 100)], 1, u64::MAX);
        assert_eq!(p.changes, [(0, 6)]);
    }
}
