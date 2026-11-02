//! CPU clustered light assignment (ADR-0012 §11): a 16x9x24 froxel grid with exponential depth
//! slices; light bounding spheres are tested against each froxel's view-space AABB.

use foundation::math::Vec3;
use rayon::prelude::*;

/// Froxels along x, y and z (depth slices).
pub(crate) const GRID: (u32, u32, u32) = (16, 9, 24);
/// Total clusters.
pub(crate) const CLUSTER_COUNT: usize = (GRID.0 * GRID.1 * GRID.2) as usize;
/// Distance of the last slice boundary; the last slice extends beyond it.
pub(crate) const CLUSTER_FAR: f32 = 100.0;
/// Stand-in for infinity as the outer edge of the last slice.
const BEYOND: f32 = 1.0e5;

/// Light count from which [`ClusterGrid::assign`] uses the worker pool.
const PAR_LIGHTS: usize = 16;

/// A light's bounding sphere in view space (camera looks down -Z).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LightSphere {
    pub center: Vec3,
    pub radius: f32,
    /// Index into the frame's light array.
    pub index: u32,
}

/// Result of [`ClusterGrid::assign`]: `(offset, count)` per cluster into `indices`.
pub(crate) struct Assignment {
    pub ranges: Vec<[u32; 2]>,
    pub indices: Vec<u32>,
}

/// View-space AABBs of all froxels for one camera shape.
pub(crate) struct ClusterGrid {
    near: f32,
    pub tan: (f32, f32),
    aabbs: Vec<(Vec3, Vec3)>,
}

/// Cluster index layout: `(slice * GRID.1 + y) * GRID.0 + x`, mirrored by `cluster_index` in GLSL.
pub(crate) fn cluster_id(x: u32, y: u32, slice: u32) -> usize {
    ((slice * GRID.1 + y) * GRID.0 + x) as usize
}

impl ClusterGrid {
    /// `tan` = (tan of half fov x, tan of half fov y).
    pub fn new(near: f32, tan: (f32, f32)) -> Self {
        let mut aabbs = Vec::with_capacity(CLUSTER_COUNT);
        for slice in 0..GRID.2 {
            let z0 = slice_depth(near, slice);
            let z1 = if slice + 1 == GRID.2 {
                BEYOND
            } else {
                slice_depth(near, slice + 1)
            };
            for y in 0..GRID.1 {
                for x in 0..GRID.0 {
                    let mut lo = Vec3::splat(f32::MAX);
                    let mut hi = Vec3::splat(f32::MIN);
                    for (nx, ny, z) in [
                        (x, y, z0),
                        (x + 1, y, z0),
                        (x, y + 1, z0),
                        (x + 1, y + 1, z0),
                        (x, y, z1),
                        (x + 1, y, z1),
                        (x, y + 1, z1),
                        (x + 1, y + 1, z1),
                    ] {
                        // NDC y = -1 is the top of the screen (projection has the Y flip).
                        let ndc_x = nx as f32 / GRID.0 as f32 * 2.0 - 1.0;
                        let ndc_y = ny as f32 / GRID.1 as f32 * 2.0 - 1.0;
                        let p = Vec3::new(ndc_x * tan.0 * z, -ndc_y * tan.1 * z, -z);
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                    aabbs.push((lo, hi));
                }
            }
        }
        Self { near, tan, aabbs }
    }

    /// True when the grid was built for this camera shape.
    pub fn matches(&self, near: f32, tan: (f32, f32)) -> bool {
        self.near == near && self.tan == tan
    }

    /// Depth slice of a view depth, as the shader computes it.
    pub fn slice_of(&self, view_z: f32) -> u32 {
        let s = (view_z.max(self.near) / self.near).ln() * GRID.2 as f32
            / (CLUSTER_FAR / self.near).ln();
        (s.max(0.0) as u32).min(GRID.2 - 1)
    }

    /// Calls `f` with every cluster (ascending slice, y, x) whose AABB the light's sphere touches.
    fn for_each_cluster(&self, l: LightSphere, mut f: impl FnMut(usize)) {
        let d = -l.center.z;
        if d + l.radius < self.near {
            return; // entirely in front of the near plane's far side: behind the camera
        }
        let (s0, s1) = if l.radius.is_finite() {
            // Padded a little so a sphere touching a slice boundary still reaches the slice.
            (
                self.slice_of((d - l.radius) * 0.999),
                self.slice_of((d + l.radius) * 1.001),
            )
        } else {
            (0, GRID.2 - 1)
        };
        let r2 = l.radius * l.radius;
        for slice in s0..=s1 {
            for y in 0..GRID.1 {
                for x in 0..GRID.0 {
                    let c = cluster_id(x, y, slice);
                    let (lo, hi) = self.aabbs[c];
                    if l.center.clamp(lo, hi).distance_squared(l.center) <= r2 {
                        f(c);
                    }
                }
            }
        }
    }

    /// Assigns every light to the clusters its sphere touches.
    pub fn assign(&self, lights: &[LightSphere]) -> Assignment {
        // (cluster, light) pairs, lights in order; on the worker pool when there are enough
        // lights (each one tests hundreds of froxels).
        let pairs: Vec<(u32, u32)> = if lights.len() < PAR_LIGHTS {
            let mut v = Vec::new();
            for l in lights {
                self.for_each_cluster(*l, |c| v.push((c as u32, l.index)));
            }
            v
        } else {
            // `fold` makes one vector per chunk of lights; the chunks come back in order.
            lights
                .par_iter()
                .fold(Vec::new, |mut v, l| {
                    self.for_each_cluster(*l, |c| v.push((c as u32, l.index)));
                    v
                })
                .collect::<Vec<_>>()
                .concat()
        };
        let mut ranges = vec![[0u32; 2]; CLUSTER_COUNT];
        for &(c, _) in &pairs {
            ranges[c as usize][1] += 1;
        }
        let mut offset = 0;
        for r in &mut ranges {
            r[0] = offset;
            offset += r[1];
        }
        let mut cursor: Vec<u32> = ranges.iter().map(|r| r[0]).collect();
        let mut indices = vec![0u32; pairs.len()];
        for &(c, i) in &pairs {
            indices[cursor[c as usize] as usize] = i;
            cursor[c as usize] += 1;
        }
        Assignment { ranges, indices }
    }
}

/// Near edge of depth slice `i` (`near * (far / near)^(i / 24)`).
pub(crate) fn slice_depth(near: f32, i: u32) -> f32 {
    near * (CLUSTER_FAR / near).powf(i as f32 / GRID.2 as f32)
}

/// Bounding sphere of a spot cone (apex, unit axis, range, half-angle): the smallest sphere
/// containing the cone's apex and base circle.
pub(crate) fn spot_bounds(apex: Vec3, axis: Vec3, range: f32, half_angle: f32) -> (Vec3, f32) {
    let (s, c) = half_angle.sin_cos();
    if half_angle > std::f32::consts::FRAC_PI_4 {
        // Wide cone: the base circle decides.
        (apex + axis * range * c, range * s)
    } else {
        let r = range / (2.0 * c);
        (apex + axis * r, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAN: (f32, f32) = (0.8, 0.5);

    fn sphere(center: Vec3, radius: f32, index: u32) -> LightSphere {
        LightSphere {
            center,
            radius,
            index,
        }
    }

    /// Cluster containing a view-space point, the way the shader finds it.
    fn cluster_of(g: &ClusterGrid, p: Vec3) -> usize {
        let z = -p.z;
        let nx = p.x / (TAN.0 * z) * 0.5 + 0.5;
        let ny = -p.y / (TAN.1 * z) * 0.5 + 0.5;
        let x = ((nx * GRID.0 as f32) as u32).min(GRID.0 - 1);
        let y = ((ny * GRID.1 as f32) as u32).min(GRID.1 - 1);
        cluster_id(x, y, g.slice_of(z))
    }

    fn lights_of(a: &Assignment, c: usize) -> &[u32] {
        let [o, n] = a.ranges[c];
        &a.indices[o as usize..(o + n) as usize]
    }

    #[test]
    fn slices_are_exponential_and_cover_near_to_far() {
        assert!((slice_depth(0.1, 0) - 0.1).abs() < 1e-6);
        assert!((slice_depth(0.1, GRID.2) - CLUSTER_FAR).abs() < 1e-2);
        let r = slice_depth(0.1, 2) / slice_depth(0.1, 1);
        assert!((r - slice_depth(0.1, 11) / slice_depth(0.1, 10)).abs() < 1e-4);
        let g = ClusterGrid::new(0.1, TAN);
        for s in 0..GRID.2 {
            // The middle of each slice maps back to it; beyond far clamps to the last.
            assert_eq!(g.slice_of(slice_depth(0.1, s) * 1.001), s);
        }
        assert_eq!(g.slice_of(5000.0), GRID.2 - 1);
    }

    #[test]
    fn light_lands_in_its_cluster_and_not_in_distant_ones() {
        let g = ClusterGrid::new(0.1, TAN);
        let p = Vec3::new(1.0, -0.5, -8.0);
        let a = g.assign(&[sphere(p, 0.3, 7)]);
        assert!(lights_of(&a, cluster_of(&g, p)).contains(&7));
        // The opposite corner of the screen at the same depth sees nothing.
        let far_corner = cluster_of(&g, Vec3::new(-3.0, 2.0, -8.0));
        assert!(lights_of(&a, far_corner).is_empty());
        // Totals agree with the ranges.
        let total: u32 = a.ranges.iter().map(|r| r[1]).sum();
        assert_eq!(total as usize, a.indices.len());
        assert!(
            a.ranges
                .iter()
                .all(|r| (r[0] + r[1]) as usize <= a.indices.len())
        );
    }

    #[test]
    fn assignment_matches_brute_force_sphere_aabb_tests() {
        let g = ClusterGrid::new(0.1, TAN);
        let lights = [
            sphere(Vec3::new(0.0, 0.0, -3.0), 2.0, 0),
            sphere(Vec3::new(2.0, 1.0, -20.0), 6.0, 1),
            sphere(Vec3::new(-4.0, -2.0, -50.0), 15.0, 2),
            sphere(Vec3::new(0.0, 0.0, 3.0), 1.0, 3), // behind the camera
            sphere(Vec3::ZERO, f32::INFINITY, 4),
        ];
        let a = g.assign(&lights);
        for c in 0..CLUSTER_COUNT {
            let (lo, hi) = g.aabbs[c];
            for l in &lights {
                let hit = l.center.clamp(lo, hi).distance(l.center) <= l.radius
                    && -l.center.z + l.radius >= 0.1;
                let got = lights_of(&a, c).contains(&l.index);
                // Infinite-radius lights are in every cluster; the rest must agree exactly.
                assert_eq!(
                    got,
                    hit || !l.radius.is_finite(),
                    "cluster {c} light {}",
                    l.index
                );
            }
        }
        assert!(
            a.ranges.iter().all(|r| r[1] >= 1),
            "the infinite light is everywhere"
        );
    }

    #[test]
    fn spot_bounds_contain_the_cone() {
        let (apex, axis) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, -1.0, 0.0));
        for half in [0.2f32, 0.6, 0.785, 1.0, 1.4] {
            let (c, r) = spot_bounds(apex, axis, 10.0, half);
            // Apex and rim of the base circle.
            assert!(apex.distance(c) <= r + 1e-4, "apex, half {half}");
            assert!(
                (apex + axis * 10.0).distance(c) <= r + 1e-3,
                "tip, half {half}"
            );
            let base = apex + axis * 10.0 * half.cos();
            for dir in [Vec3::X, Vec3::Z, -Vec3::X, -Vec3::Z] {
                let rim = base + dir * 10.0 * half.sin();
                assert!(rim.distance(c) <= r + 1e-3, "rim, half {half}");
            }
        }
    }

    #[test]
    fn parallel_assignment_matches_serial() {
        let g = ClusterGrid::new(0.1, TAN);
        let r = |n: u32| (n.wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 24) as f32;
        let mut lights: Vec<LightSphere> = (0..300)
            .map(|i| {
                sphere(
                    Vec3::new(
                        r(i * 3) * 40.0 - 20.0,
                        r(i * 3 + 1) * 10.0 - 5.0,
                        -r(i * 3 + 2) * 60.0 + 5.0,
                    ),
                    0.5 + r(i * 7) * 6.0,
                    i,
                )
            })
            .collect();
        lights.push(sphere(Vec3::new(0.0, 0.0, -5.0), f32::INFINITY, 300));
        let run = |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| g.assign(&lights))
        };
        let (a, b) = (run(8), run(1));
        assert_eq!((&a.ranges, &a.indices), (&b.ranges, &b.indices));
        // Serial reference: per cluster, the lights touching it in light order.
        let mut expect = vec![Vec::new(); CLUSTER_COUNT];
        for l in &lights {
            g.for_each_cluster(*l, |c| expect[c].push(l.index));
        }
        assert!(expect.iter().any(|e| e.len() > 1));
        for (c, e) in expect.iter().enumerate() {
            assert_eq!(lights_of(&a, c), &e[..], "cluster {c}");
        }
    }
}
