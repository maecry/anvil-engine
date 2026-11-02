//! Parallel CPU preparation (ADR-0013 §3): instance packing, frustum culling and draw-list
//! sorting on the `rayon` pool. Every function returns exactly what the serial loop would
//! (order-preserving collects, stable sorts); the tests compare against serial references.

use foundation::math::{Mat4, Vec3};
use rayon::prelude::*;

use crate::cull::{Frustum, transform_sphere};
use crate::gpu::GpuInstance;
use crate::passes::Draw;
use crate::scene::MeshInstance;

/// Inputs smaller than this run on the calling thread: waking the pool costs more than the
/// work. shortcut: one fixed threshold measured on the reference machine (BENCH-001), tune
/// per CPU or measure it when the pool is warm.
pub(crate) const PAR_MIN: usize = 2048;

/// A scene instance after resolving its mesh and material, before world bounds.
#[derive(Clone, Copy)]
pub(crate) struct Resolved {
    pub vertex_address: u64,
    pub index_buffer: rhi::BufferHandle,
    pub index_count: u32,
    /// Object-space bounding sphere.
    pub center: Vec3,
    pub radius: f32,
    /// Slot of the material in the GPU material array.
    pub material: u32,
    /// Pipeline variant (see [`Draw::variant`]).
    pub variant: usize,
    pub blend: bool,
}

/// One drawable instance with its world-space bounds.
#[derive(Clone, Copy)]
pub(crate) struct Item {
    pub draw: Draw,
    pub center: Vec3,
    pub radius: f32,
    pub blend: bool,
    /// Index into the scene's instance list.
    src: u32,
    material: u32,
}

/// Buffers reused every frame: fresh multi-megabyte allocations page-fault on every first touch,
/// which costs more than the packing itself and does not scale across threads.
#[derive(Default)]
pub(crate) struct Scratch {
    slots: Vec<Option<Item>>,
    /// Drawable instances in scene order, compacted.
    pub items: Vec<Item>,
    /// GPU instance data, parallel to `items`.
    pub instances: Vec<GpuInstance>,
}

/// Resolves, bounds and packs every instance for which `resolve` returns `Some` into
/// `scratch.items` / `scratch.instances`; `Draw::instance` is the position in those arrays.
/// `view` gives the sort depth.
pub(crate) fn pack(
    scratch: &mut Scratch,
    meshes: &[MeshInstance],
    view: &Mat4,
    resolve: impl Fn(&MeshInstance) -> Option<Resolved> + Sync,
) {
    let Scratch {
        slots,
        items,
        instances,
    } = scratch;
    slots.clear();
    slots.par_extend(
        meshes
            .par_iter()
            .enumerate()
            .with_min_len(PAR_MIN)
            .map(|(src, inst)| {
                let r = resolve(inst)?;
                let (center, radius) = transform_sphere(&inst.transform, r.center, r.radius);
                Some(Item {
                    draw: Draw {
                        vertex_address: r.vertex_address,
                        index_buffer: r.index_buffer,
                        index_count: r.index_count,
                        instance: 0,
                        variant: r.variant,
                        depth: -view.transform_point3(center).z,
                    },
                    center,
                    radius,
                    blend: r.blend,
                    src: src as u32,
                    material: r.material,
                })
            }),
    );
    items.clear();
    items.extend(
        slots
            .iter()
            .flatten()
            .copied()
            .enumerate()
            .map(|(i, mut it)| {
                it.draw.instance = i as u32;
                it
            }),
    );
    instances.clear();
    instances.par_extend(items.par_iter().with_min_len(PAR_MIN).map(|it| {
        let inst = &meshes[it.src as usize];
        GpuInstance {
            model: inst.transform,
            normal_matrix: inst.transform.inverse().transpose(),
            prev_model: inst.prev_transform,
            material: it.material,
            pad: [0; 3],
        }
    }));
}

/// Items whose bounding sphere touches the frustum, in input order.
pub(crate) fn cull<'a>(items: &'a [Item], f: &Frustum) -> Vec<&'a Item> {
    items
        .par_iter()
        .with_min_len(PAR_MIN)
        .filter(|it| f.intersects_sphere(it.center, it.radius))
        .collect()
}

/// Opaque draws sorted by pipeline variant then front to back, blended draws back to front.
pub(crate) fn sort_camera(opaque: &mut [Draw], blended: &mut [Draw]) {
    let by_opaque =
        |a: &Draw, b: &Draw| a.variant.cmp(&b.variant).then(a.depth.total_cmp(&b.depth));
    let by_blended = |a: &Draw, b: &Draw| b.depth.total_cmp(&a.depth);
    // Stable sorts: equal keys keep scene order, in the parallel and the serial branch alike.
    if opaque.len() + blended.len() < PAR_MIN {
        opaque.sort_by(by_opaque);
        blended.sort_by(by_blended);
    } else {
        rayon::join(
            || opaque.par_sort_by(by_opaque),
            || blended.par_sort_by(by_blended),
        );
    }
}

/// Shadow casters (everything but blended objects) inside `f`, grouped by variant.
pub(crate) fn shadow_draws(items: &[Item], f: &Frustum) -> Vec<Draw> {
    let mut v: Vec<Draw> = items
        .par_iter()
        .with_min_len(PAR_MIN)
        .filter(|it| !it.blend && f.intersects_sphere(it.center, it.radius))
        .map(|it| it.draw)
        .collect();
    if v.len() < PAR_MIN {
        v.sort_by_key(|d| d.variant);
    } else {
        v.par_sort_by_key(|d| d.variant);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::math::Quat;

    use crate::Camera;

    fn rnd(n: u32) -> f32 {
        let mut x = n.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
        x ^= x >> 15;
        x = x.wrapping_mul(0x2C1B_3C6D);
        x ^= x >> 13;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn scene(n: u32) -> Vec<MeshInstance> {
        (0..n)
            .map(|i| {
                let t = Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.5 + rnd(i * 5 + 3)),
                    Quat::from_rotation_y(rnd(i * 5)),
                    Vec3::new(
                        rnd(i * 5 + 1) * 200.0 - 100.0,
                        0.5,
                        rnd(i * 5 + 2) * 200.0 - 100.0,
                    ),
                );
                MeshInstance::new(crate::MeshId::NULL, crate::MaterialId::NULL, t)
            })
            .collect()
    }

    /// Every 7th instance is unresolvable (destroyed mesh); depth ties are common (quantized).
    fn resolve(inst: &MeshInstance) -> Option<Resolved> {
        let k = (inst.transform.w_axis.x * 3.0) as i32;
        (k % 7 != 0).then_some(Resolved {
            vertex_address: 1,
            index_buffer: rhi::BufferHandle::NULL,
            index_count: 36,
            center: Vec3::ZERO,
            radius: 0.8,
            material: k.unsigned_abs(),
            variant: (k.unsigned_abs() % 4) as usize,
            blend: k.unsigned_abs().is_multiple_of(5),
        })
    }

    fn setup() -> (Vec<MeshInstance>, Mat4, Frustum) {
        let cam = Camera {
            position: Vec3::new(0.0, 3.0, 60.0),
            rotation: Quat::IDENTITY,
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        };
        let view = cam.view();
        (
            scene(40000),
            view,
            Frustum::from_view_proj(cam.projection(1.6) * view),
        )
    }

    fn same(a: &Draw, b: &Draw) -> bool {
        (a.instance, a.variant, a.depth.to_bits(), a.index_count)
            == (b.instance, b.variant, b.depth.to_bits(), b.index_count)
    }

    #[test]
    fn parallel_matches_serial() {
        let (meshes, view, frustum) = setup();
        let mut scratch = Scratch::default();
        pack(&mut scratch, &meshes, &view, resolve);
        let (items, instances) = (&scratch.items, &scratch.instances);

        // Serial pack.
        let mut serial_items = Vec::new();
        for inst in &meshes {
            if let Some(r) = resolve(inst) {
                let (c, rad) = transform_sphere(&inst.transform, r.center, r.radius);
                serial_items.push((
                    c,
                    rad,
                    -view.transform_point3(c).z,
                    r.variant,
                    r.blend,
                    r.material,
                ));
            }
        }
        assert_eq!(items.len(), serial_items.len());
        assert!(items.len() < meshes.len());
        for (i, (it, s)) in items.iter().zip(&serial_items).enumerate() {
            assert_eq!(it.draw.instance, i as u32);
            assert_eq!(
                (
                    it.center,
                    it.radius,
                    it.draw.depth,
                    it.draw.variant,
                    it.blend
                ),
                (s.0, s.1, s.2, s.3, s.4)
            );
            assert_eq!(instances[i].material, s.5);
        }

        // Cull, partition and sort against plain iterators.
        let kept = cull(items, &frustum);
        let serial_kept: Vec<&Item> = items
            .iter()
            .filter(|it| frustum.intersects_sphere(it.center, it.radius))
            .collect();
        assert!(!kept.is_empty() && kept.len() < items.len());
        assert!(
            kept.iter()
                .zip(&serial_kept)
                .all(|(a, b)| std::ptr::eq(*a, *b))
                && kept.len() == serial_kept.len()
        );

        let (mut opaque, mut blended): (Vec<Draw>, Vec<Draw>) = (Vec::new(), Vec::new());
        for it in &kept {
            if it.blend { &mut blended } else { &mut opaque }.push(it.draw);
        }
        assert!(
            opaque.len() + blended.len() >= PAR_MIN,
            "exercise the parallel sort"
        );
        let (mut so, mut sb) = (opaque.clone(), blended.clone());
        sort_camera(&mut opaque, &mut blended);
        so.sort_by(|a, b| a.variant.cmp(&b.variant).then(a.depth.total_cmp(&b.depth)));
        sb.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        assert!(opaque.iter().zip(&so).all(|(a, b)| same(a, b)));
        assert!(blended.iter().zip(&sb).all(|(a, b)| same(a, b)));

        // Shadow lists.
        let sh = shadow_draws(items, &frustum);
        let mut ssh: Vec<Draw> = items
            .iter()
            .filter(|it| !it.blend && frustum.intersects_sphere(it.center, it.radius))
            .map(|it| it.draw)
            .collect();
        ssh.sort_by_key(|d| d.variant);
        assert_eq!(sh.len(), ssh.len());
        assert!(sh.iter().zip(&ssh).all(|(a, b)| same(a, b)));
    }

    #[test]
    fn one_thread_equals_many() {
        let (meshes, view, frustum) = setup();
        let run = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| {
                    let mut scratch = Scratch::default();
                    pack(&mut scratch, &meshes, &view, resolve);
                    let mut v = shadow_draws(&scratch.items, &frustum);
                    let (mut o, mut b) = (v.split_off(v.len() / 2), v.clone());
                    sort_camera(&mut o, &mut b);
                    o.iter()
                        .chain(&b)
                        .map(|d| (d.instance, d.depth.to_bits()))
                        .collect::<Vec<_>>()
                })
        };
        assert_eq!(run(1), run(8));
    }
}
