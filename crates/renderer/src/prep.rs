//! Parallel CPU preparation (ADR-0013 §3, §6): the persistent scene cache that finds the
//! instances changed since the previous frame, frustum culling and draw-list sorting on the
//! `rayon` pool. Every function returns exactly what the serial loop would (order-preserving
//! collects, stable sorts); the tests compare against serial references.
//!
//! Change detection compares each [`MeshInstance`] with the one at the same scene index last
//! frame (bitwise-equal fields) instead of adding stable ids or generations to the scene API:
//! the app keeps rebuilding `RenderScene` the way it always did, the comparison is one pass over
//! `n` instances (about 150 bytes each, parallel), and an unchanged scene uploads nothing.
//! shortcut: instance identity is its scene index, so removing an instance from the middle
//! re-uploads everything after it; add stable instance ids when scenes churn.

use foundation::math::{Mat4, Vec3};
use rayon::prelude::*;

use crate::cull::{Frustum, max_scale, transform_sphere};
use crate::gpu::{FLAG_BLEND, FLAG_VALID, GpuInstance};
use crate::passes::Draw;
use crate::scene::MeshInstance;

/// Inputs smaller than this run on the calling thread: waking the pool costs more than the
/// work. shortcut: one fixed threshold measured on the reference machine (BENCH-001), tune
/// per CPU or measure it when the pool is warm.
pub(crate) const PAR_MIN: usize = 2048;

/// A scene instance after resolving its mesh and material, before world bounds.
#[derive(Clone, Copy)]
pub(crate) struct Resolved {
    /// Index of the mesh record in the GPU mesh table.
    pub mesh: u32,
    /// Level 0 of the mesh in the arena.
    pub first_index: u32,
    pub index_count: u32,
    pub vertex_offset: i32,
    /// Object-space bounding sphere.
    pub center: Vec3,
    pub radius: f32,
    /// Slot of the material in the GPU material array.
    pub material: u32,
    /// Pipeline variant (see [`Draw::variant`]).
    pub variant: usize,
    pub blend: bool,
}

/// One drawable instance with its world-space bounds. `draw.depth` is filled when culling.
#[derive(Clone, Copy)]
pub(crate) struct Item {
    pub draw: Draw,
    pub center: Vec3,
    pub radius: f32,
    pub blend: bool,
}

/// The persistent scene: what the GPU instance buffer holds, plus the CPU-side items for the
/// CPU culling path and the transparent pass.
#[derive(Default)]
pub(crate) struct SceneCache {
    /// Last frame's scene instances, by scene index.
    prev: Vec<MeshInstance>,
    /// Drawable items by slot (`None` for unresolvable instances).
    pub items: Vec<Option<Item>>,
    /// Mirror of the GPU instance buffer contents.
    pub gpu: Vec<GpuInstance>,
    /// Slots updated by the last [`SceneCache::update`], ascending.
    pub changed: Vec<u32>,
    /// Opaque (non-blended) drawable instances per pipeline variant.
    pub variant_counts: [u32; 4],
    /// Blended items, rebuilt when something changed.
    pub blended: Vec<Item>,
}

impl SceneCache {
    /// Number of instances in the scene.
    pub fn len(&self) -> usize {
        self.gpu.len()
    }

    /// Forgets last frame's scene: the next update treats every instance as changed (call when
    /// meshes or materials were destroyed, so resolved data cannot go stale).
    pub fn invalidate(&mut self) {
        self.prev.clear();
    }

    /// Diffs `meshes` against last frame's, re-resolves and re-packs the changed instances and
    /// fills `changed`. `resolve` maps an instance to its mesh and material data.
    pub fn update(
        &mut self,
        meshes: &[MeshInstance],
        resolve: impl Fn(&MeshInstance) -> Option<Resolved> + Sync,
    ) {
        let n = meshes.len();
        let prev = &self.prev;
        let differs = |i: &usize| prev.get(*i) != Some(&meshes[*i]);
        self.changed.clear();
        if n >= PAR_MIN {
            self.changed.par_extend(
                (0..n)
                    .into_par_iter()
                    .with_min_len(PAR_MIN)
                    .filter(differs)
                    .map(|i| i as u32),
            );
        } else {
            self.changed
                .extend((0..n).filter(differs).map(|i| i as u32));
        }
        let pack = |&slot: &u32| {
            let inst = &meshes[slot as usize];
            let item = resolve(inst).map(|r| {
                let (center, radius) = transform_sphere(&inst.transform, r.center, r.radius);
                let gpu = GpuInstance {
                    model: inst.transform,
                    normal_matrix: inst.transform.inverse().transpose(),
                    prev_model: inst.prev_transform,
                    sphere: [center.x, center.y, center.z, radius],
                    material: r.material,
                    mesh: r.mesh,
                    flags: r.variant as u32 | FLAG_VALID | if r.blend { FLAG_BLEND } else { 0 },
                    scale: max_scale(&inst.transform),
                };
                let item = Item {
                    draw: Draw {
                        first_index: r.first_index,
                        index_count: r.index_count,
                        vertex_offset: r.vertex_offset,
                        instance: slot,
                        variant: r.variant,
                        depth: 0.0,
                    },
                    center,
                    radius,
                    blend: r.blend,
                };
                (gpu, item)
            });
            item.map_or((bytemuck::Zeroable::zeroed(), None), |(g, i)| (g, Some(i)))
        };
        let packed: Vec<(GpuInstance, Option<Item>)> = if self.changed.len() >= PAR_MIN {
            self.changed
                .par_iter()
                .with_min_len(PAR_MIN)
                .map(pack)
                .collect()
        } else {
            self.changed.iter().map(pack).collect()
        };
        self.gpu.resize(n, bytemuck::Zeroable::zeroed());
        self.items.resize(n, None);
        self.prev.truncate(n);
        for (&slot, (gpu, item)) in self.changed.iter().zip(packed) {
            let s = slot as usize;
            self.gpu[s] = gpu;
            self.items[s] = item;
            if s < self.prev.len() {
                self.prev[s] = meshes[s];
            } else {
                self.prev.push(meshes[s]);
            }
        }
        debug_assert_eq!(self.prev.len(), n);
        if !self.changed.is_empty() {
            self.variant_counts = [0; 4];
            self.blended.clear();
            for it in self.items.iter().flatten() {
                if it.blend {
                    self.blended.push(*it);
                } else {
                    self.variant_counts[it.draw.variant] += 1;
                }
            }
        }
    }
}

/// Items whose bounding sphere touches the frustum, in input order, with the view depth (the
/// sort key) filled in.
pub(crate) fn cull(items: &[Option<Item>], f: &Frustum, view: &Mat4) -> Vec<Item> {
    let keep = |it: &Item| {
        f.intersects_sphere(it.center, it.radius).then(|| {
            let mut it = *it;
            it.draw.depth = -view.transform_point3(it.center).z;
            it
        })
    };
    if items.len() >= PAR_MIN {
        items
            .par_iter()
            .with_min_len(PAR_MIN)
            .filter_map(|o| o.as_ref().and_then(keep))
            .collect()
    } else {
        items
            .iter()
            .filter_map(|o| o.as_ref().and_then(keep))
            .collect()
    }
}

/// [`cull`] over a plain item list (the blended items).
pub(crate) fn cull_list(items: &[Item], f: &Frustum, view: &Mat4) -> Vec<Item> {
    items
        .iter()
        .filter(|it| f.intersects_sphere(it.center, it.radius))
        .map(|it| {
            let mut it = *it;
            it.draw.depth = -view.transform_point3(it.center).z;
            it
        })
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
pub(crate) fn shadow_draws(items: &[Option<Item>], f: &Frustum) -> Vec<Draw> {
    let keep =
        |it: &Item| (!it.blend && f.intersects_sphere(it.center, it.radius)).then_some(it.draw);
    let mut v: Vec<Draw> = if items.len() >= PAR_MIN {
        items
            .par_iter()
            .with_min_len(PAR_MIN)
            .filter_map(|o| o.as_ref().and_then(keep))
            .collect()
    } else {
        items
            .iter()
            .filter_map(|o| o.as_ref().and_then(keep))
            .collect()
    };
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
            mesh: 3,
            first_index: 12,
            index_count: 36,
            vertex_offset: 5,
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
        let mut cache = SceneCache::default();
        cache.update(&meshes, resolve);
        let items = &cache.items;
        assert_eq!((cache.len(), items.len()), (meshes.len(), meshes.len()));
        assert_eq!(
            cache.changed.len(),
            meshes.len(),
            "first frame: all changed"
        );

        // Serial pack against the cache, slot by slot (unresolvable slots stay empty).
        let mut drawable = 0;
        for (slot, inst) in meshes.iter().enumerate() {
            let (g, it) = (&cache.gpu[slot], &items[slot]);
            match resolve(inst) {
                None => assert!(it.is_none() && g.flags & FLAG_VALID == 0),
                Some(r) => {
                    drawable += 1;
                    let it = it.as_ref().expect("resolved");
                    let (c, rad) = transform_sphere(&inst.transform, r.center, r.radius);
                    assert_eq!(
                        (it.center, it.radius, it.draw.variant, it.blend),
                        (c, rad, r.variant, r.blend)
                    );
                    assert_eq!(it.draw.instance, slot as u32);
                    assert_eq!(g.sphere, [c.x, c.y, c.z, rad]);
                    assert_eq!(
                        (g.material, g.mesh, g.flags & 3),
                        (r.material, 3, r.variant as u32)
                    );
                    assert_eq!(g.flags & FLAG_BLEND != 0, r.blend);
                    assert_eq!(g.model, inst.transform);
                }
            }
        }
        assert!(drawable > 0 && drawable < meshes.len());
        let opaque_total: u32 = cache.variant_counts.iter().sum();
        assert_eq!(opaque_total as usize + cache.blended.len(), drawable);

        // Cull, partition and sort against plain iterators.
        let kept = cull(items, &frustum, &view);
        let serial_kept: Vec<Item> = items
            .iter()
            .flatten()
            .filter(|it| frustum.intersects_sphere(it.center, it.radius))
            .copied()
            .collect();
        assert!(!kept.is_empty() && kept.len() < drawable);
        assert_eq!(kept.len(), serial_kept.len());
        assert!(
            kept.iter()
                .zip(&serial_kept)
                .all(|(a, b)| a.draw.instance == b.draw.instance)
        );
        assert!(
            kept.iter()
                .all(|it| it.draw.depth == -view.transform_point3(it.center).z)
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
        // The blended list culls like the full one.
        let from_list = cull_list(&cache.blended, &frustum, &view);
        assert_eq!(from_list.len(), blended.len());

        // Shadow lists.
        let sh = shadow_draws(items, &frustum);
        let mut ssh: Vec<Draw> = items
            .iter()
            .flatten()
            .filter(|it| !it.blend && frustum.intersects_sphere(it.center, it.radius))
            .map(|it| it.draw)
            .collect();
        ssh.sort_by_key(|d| d.variant);
        assert_eq!(sh.len(), ssh.len());
        assert!(sh.iter().zip(&ssh).all(|(a, b)| same(a, b)));
    }

    #[test]
    fn only_changed_instances_are_repacked() {
        let mut meshes = scene(5000);
        let mut cache = SceneCache::default();
        let resolved = std::sync::atomic::AtomicUsize::new(0);
        let counting = |i: &MeshInstance| {
            resolved.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            resolve(i)
        };
        cache.update(&meshes, counting);
        assert_eq!(resolved.swap(0, std::sync::atomic::Ordering::Relaxed), 5000);
        // Same scene: nothing to do.
        cache.update(&meshes, counting);
        assert!(cache.changed.is_empty());
        assert_eq!(resolved.load(std::sync::atomic::Ordering::Relaxed), 0);
        // Move two instances, one of them also gets a new previous transform.
        let moved = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)) * meshes[10].transform;
        meshes[10] = MeshInstance {
            transform: moved,
            ..meshes[10]
        };
        meshes[4000].prev_transform = moved;
        cache.update(&meshes, counting);
        assert_eq!(cache.changed, [10, 4000]);
        assert_eq!(resolved.swap(0, std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(cache.gpu[10].model, moved);
        assert_eq!(cache.gpu[4000].prev_model, moved);
        // Growing appends the new slots, shrinking drops them without touching the rest.
        meshes.extend(scene(3));
        cache.update(&meshes, counting);
        assert_eq!(cache.changed, [5000, 5001, 5002]);
        assert_eq!(cache.len(), 5003);
        meshes.truncate(4990);
        cache.update(&meshes, counting);
        assert!(cache.changed.is_empty());
        assert_eq!((cache.len(), cache.items.len()), (4990, 4990));
        // Invalidation (a mesh was destroyed) re-resolves everything.
        cache.invalidate();
        cache.update(&meshes, counting);
        assert_eq!(cache.changed.len(), 4990);
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
                    let mut cache = SceneCache::default();
                    cache.update(&meshes, resolve);
                    let mut v = shadow_draws(&cache.items, &frustum);
                    let (mut o, mut b) = (v.split_off(v.len() / 2), v.clone());
                    sort_camera(&mut o, &mut b);
                    let kept = cull(&cache.items, &frustum, &view);
                    (
                        o.iter()
                            .chain(&b)
                            .map(|d| (d.instance, d.depth.to_bits()))
                            .collect::<Vec<_>>(),
                        cache.changed.clone(),
                        kept.iter().map(|k| k.draw.instance).collect::<Vec<_>>(),
                    )
                })
        };
        assert_eq!(run(1), run(8));
    }
}
