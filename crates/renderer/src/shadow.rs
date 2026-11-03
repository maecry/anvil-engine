//! Directional shadow volume fitting (ADR-0011 §7).

use foundation::math::camera::rh::view::look_to_mat4;
use foundation::math::{Mat4, Vec3, Vec4};

use crate::{Camera, Light, MAX_POINT_SHADOWS};

/// Orthographic projection for Vulkan (depth 0..1 from `near` to `far`, Y down) of a
/// right-handed Y-up view space.
// glam 0.34.2's `camera::rh::proj::vulkan::orthographic` flips Y but not the Y translation, which
// mis-centers any off-center volume; this one is exact (checked by `ortho_is_centered`).
pub(crate) fn orthographic(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) -> Mat4 {
    Mat4::from_cols_array(&[
        2.0 / (r - l),
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / (t - b),
        0.0,
        0.0,
        0.0,
        0.0,
        -1.0 / (far - near),
        0.0,
        -(r + l) / (r - l),
        (t + b) / (t - b),
        -near / (far - near),
        1.0,
    ])
}

/// Result of [`fit_directional`].
pub(crate) struct ShadowFit {
    /// World to shadow-map clip space (orthographic, Y flip folded in, depth 0..1).
    pub view_proj: Mat4,
    /// World size of one shadow texel.
    pub texel_world: f32,
}

/// Cascades of the directional light.
pub(crate) const CASCADES: usize = 4;

/// Practical split scheme (Zhang et al.): view depth at which each cascade ends, blending the
/// logarithmic (`lambda = 1`) and uniform (`lambda = 0`) schemes between `near` and `far`.
pub(crate) fn cascade_splits(near: f32, far: f32, lambda: f32) -> [f32; CASCADES] {
    std::array::from_fn(|i| {
        let t = (i + 1) as f32 / CASCADES as f32;
        let log = near * (far / near).powf(t);
        let uniform = near + (far - near) * t;
        lambda * log + (1.0 - lambda) * uniform
    })
}

/// Fits an orthographic light volume around the camera frustum slice between view depths
/// `near_d` and `far_d`, as a bounding sphere (stable under camera rotation) snapped to whole
/// texels (stable under translation), so the shadow edges do not shimmer. `dir` is the
/// direction the light travels.
pub(crate) fn fit_directional(
    camera: &Camera,
    aspect: f32,
    dir: Vec3,
    near_d: f32,
    far_d: f32,
    resolution: u32,
) -> ShadowFit {
    let dir = dir.normalize();
    let tan_y = (camera.fov_y * 0.5).tan();
    let tan_x = tan_y * aspect;
    let to_world = camera.view().inverse();
    let mut corners = [Vec3::ZERO; 8];
    for (i, c) in corners.iter_mut().enumerate() {
        let d = if i < 4 { near_d } else { far_d };
        let (sx, sy) = (
            if i & 1 == 0 { -1.0 } else { 1.0 },
            if i & 2 == 0 { -1.0 } else { 1.0 },
        );
        *c = to_world.transform_point3(Vec3::new(sx * tan_x * d, sy * tan_y * d, -d));
    }
    let center = corners.iter().copied().sum::<Vec3>() / 8.0;
    let radius = corners
        .iter()
        .map(|c| c.distance(center))
        .fold(0.0, f32::max)
        .max(1e-3);

    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = look_to_mat4(Vec3::ZERO, dir, up);
    let c = view.transform_point3(center);
    let texel = 2.0 * radius / resolution as f32;
    let (sx, sy) = ((c.x / texel).round() * texel, (c.y / texel).round() * texel);
    // Casters up to 2 radii toward the light are kept; depth is the distance along `dir`.
    let proj = orthographic(
        sx - radius,
        sx + radius,
        sy - radius,
        sy + radius,
        -c.z - 2.0 * radius,
        -c.z + radius,
    );
    ShadowFit {
        view_proj: proj * view,
        texel_world: texel,
    }
}

/// Perspective shadow matrix of a spot light (conventional depth 0..1, Y flip folded in); the
/// field of view covers the outer cone with a small margin for the PCF kernel.
pub(crate) fn fit_spot(position: Vec3, direction: Vec3, outer_cone: f32, range: f32) -> Mat4 {
    let dir = direction.normalize();
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let fov = (outer_cone * 2.0 * 1.1).clamp(0.1, 3.0);
    let proj = foundation::math::camera::rh::proj::vulkan::perspective(
        fov,
        1.0,
        (range * 0.005).max(0.05),
        range.max(1.0),
    );
    proj * look_to_mat4(position, dir, up)
}

/// Near plane of every point-light shadow face (world units); `POINT_NEAR` in `lighting.glsl`.
pub(crate) const POINT_NEAR: f32 = 0.05;
/// Smallest range a point light may have to cast shadows (the far plane is the range).
pub(crate) const POINT_MIN_RANGE: f32 = 0.5;
/// Resolution of every cube face; mirrored by `POINT_SHADOW_SIZE` in `lighting.glsl`.
pub(crate) const POINT_SHADOW_SIZE: u32 = 512;

/// The Vulkan cube-map face frames `(s axis, t axis, major axis)` in the order +X -X +Y -Y +Z -Z
/// (array layers 0..5): a direction `r` samples `u = (s.r / |m.r| + 1) / 2` and `v = (t.r / |m.r| + 1) / 2`
/// with `v` growing down the image (Vulkan spec, cube map face selection table). The frame is
/// left-handed, so the face images are mirrored against a right-handed camera: the face
/// pipelines flip the culled winding.
const FACES: [(Vec3, Vec3, Vec3); 6] = [
    (Vec3::NEG_Z, Vec3::NEG_Y, Vec3::X),
    (Vec3::Z, Vec3::NEG_Y, Vec3::NEG_X),
    (Vec3::X, Vec3::Z, Vec3::Y),
    (Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y),
    (Vec3::X, Vec3::NEG_Y, Vec3::Z),
    (Vec3::NEG_X, Vec3::NEG_Y, Vec3::NEG_Z),
];

/// World to clip matrix of cube face `face` of a point light at `position`: 90 degrees, square,
/// conventional depth 0..1 between [`POINT_NEAR`] and `far` of the major-axis distance (the
/// depth the fragment shader recomputes from the light-to-fragment direction).
pub(crate) fn point_face_view_proj(position: Vec3, far: f32, face: usize) -> Mat4 {
    let (s, t, m) = FACES[face];
    let view = Mat4::from_cols(
        Vec4::new(s.x, t.x, m.x, 0.0),
        Vec4::new(s.y, t.y, m.y, 0.0),
        Vec4::new(s.z, t.z, m.z, 0.0),
        Vec4::new(-s.dot(position), -t.dot(position), -m.dot(position), 1.0),
    );
    let a = far / (far - POINT_NEAR);
    let proj = Mat4::from_cols(
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, a, 1.0),
        Vec4::new(0.0, 0.0, -a * POINT_NEAR, 0.0),
    );
    proj * view
}

/// Picks the point lights that get a cube shadow map this frame: among the lights with
/// `cast_shadows` and a usable range that reach into `max_distance` of the camera, the
/// [`MAX_POINT_SHADOWS`] with the highest `luminous intensity / distance^2` (distance and
/// intensity, nearest bright lights first). Returns indices into `lights`, best first.
pub(crate) fn select_point_shadows(
    lights: &[Light],
    camera: Vec3,
    max_distance: f32,
) -> Vec<usize> {
    let mut scored: Vec<(f32, usize)> = lights
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match *l {
            Light::Point {
                position,
                color,
                intensity,
                range,
                cast_shadows: true,
            } if range >= POINT_MIN_RANGE => {
                let d = position.distance(camera);
                (d - range < max_distance).then(|| {
                    let luma = color.dot(Vec3::new(0.2126, 0.7152, 0.0722)) * intensity;
                    (luma / (d * d).max(1.0), i)
                })
            }
            _ => None,
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(MAX_POINT_SHADOWS);
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::math::Quat;

    fn cam(pos: Vec3) -> Camera {
        Camera {
            position: pos,
            rotation: Quat::from_rotation_x(-0.3),
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        }
    }

    #[test]
    fn frustum_points_land_inside_the_shadow_map() {
        let c = cam(Vec3::new(1.0, 3.0, 9.0));
        let fit = fit_directional(&c, 1.5, Vec3::new(-0.5, -1.0, -0.4), c.near, 50.0, 2048);
        let ahead = c.position + c.rotation * Vec3::new(0.0, 0.0, -20.0);
        for p in [c.position, ahead, ahead + Vec3::new(3.0, 0.0, 0.0)] {
            let n = fit.view_proj.project_point3(p);
            assert!(
                n.x.abs() <= 1.0 && n.y.abs() <= 1.0 && (0.0..=1.0).contains(&n.z),
                "{n:?}"
            );
        }
    }

    #[test]
    fn texel_snapping_removes_subtexel_motion() {
        let d = Vec3::new(-0.5, -1.0, -0.4);
        let a = fit_directional(&cam(Vec3::new(0.0, 3.0, 9.0)), 1.5, d, 0.1, 50.0, 2048);
        let b = fit_directional(
            &cam(Vec3::new(0.0, 3.0, 9.0) + Vec3::X * 0.001),
            1.5,
            d,
            0.1,
            50.0,
            2048,
        );
        // A point's shadow-map texel coordinate does not change for sub-texel camera motion.
        let p = Vec3::new(2.0, 0.0, 1.0);
        let t = |f: &ShadowFit| (f.view_proj.project_point3(p).truncate() * 1024.0).round();
        assert_eq!(t(&a), t(&b));
    }

    #[test]
    fn ortho_is_centered() {
        let m = orthographic(-1.0, 1.0, 2.0, 4.0, 0.0, 10.0);
        let p = |y: f32, z: f32| m.project_point3(Vec3::new(0.0, y, z));
        assert!(p(3.0, -5.0).y.abs() < 1e-6);
        // Y points down: the top edge is -1, the bottom edge +1; depth 0 at near, 1 at far.
        assert!((p(4.0, -5.0).y + 1.0).abs() < 1e-6 && (p(2.0, -5.0).y - 1.0).abs() < 1e-6);
        assert!(p(3.0, 0.0).z.abs() < 1e-6 && (p(3.0, -10.0).z - 1.0).abs() < 1e-6);
    }

    #[test]
    fn splits_are_increasing_blend_of_log_and_uniform() {
        let s = cascade_splits(0.1, 80.0, 0.75);
        assert!(s.windows(2).all(|w| w[0] < w[1]) && s[0] > 0.1);
        assert!((s[3] - 80.0).abs() < 1e-3, "last cascade ends at far");
        // Between the pure schemes.
        let (log, uni) = (
            cascade_splits(0.1, 80.0, 1.0),
            cascade_splits(0.1, 80.0, 0.0),
        );
        for i in 0..CASCADES {
            assert!(s[i] >= log[i].min(uni[i]) - 1e-4 && s[i] <= log[i].max(uni[i]) + 1e-4);
        }
        // lambda 0 is exactly uniform: equal depth steps from near to far.
        assert!((uni[1] - (0.1 + 79.9 * 0.5)).abs() < 1e-3);
    }

    #[test]
    fn every_cascade_contains_its_slice_and_texels_shrink_toward_the_camera() {
        let c = cam(Vec3::new(1.0, 3.0, 9.0));
        let splits = cascade_splits(c.near, 80.0, 0.75);
        let mut prev = c.near;
        let mut texels = Vec::new();
        for s in splits {
            let fit = fit_directional(&c, 1.5, Vec3::new(-0.5, -1.0, -0.4), prev, s, 2048);
            // Points on the view axis at the slice ends project inside the map.
            let fwd = c.rotation * Vec3::new(0.0, 0.0, -1.0);
            for d in [prev + 0.01, s - 0.01] {
                let n = fit.view_proj.project_point3(c.position + fwd * d);
                assert!(
                    n.x.abs() <= 1.0 && n.y.abs() <= 1.0 && (0.0..=1.0).contains(&n.z),
                    "cascade ends {s} d {d} {n:?}"
                );
            }
            texels.push(fit.texel_world);
            prev = s;
        }
        assert!(texels.windows(2).all(|w| w[0] < w[1]), "{texels:?}");
    }

    #[test]
    fn spot_matrix_maps_the_cone_axis_to_the_center() {
        let m = fit_spot(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            0.5,
            10.0,
        );
        let n = m.project_point3(Vec3::new(0.0, 1.0, 0.0));
        assert!(n.x.abs() < 1e-4 && n.y.abs() < 1e-4 && n.z > 0.0 && n.z < 1.0);
        // A point at the outer cone edge stays inside.
        let edge = m.project_point3(Vec3::new(4.0 * 0.5f32.tan(), 1.0, 0.0));
        assert!(edge.x.abs() <= 1.0);
    }

    #[test]
    fn point_faces_project_directions_like_the_cube_sampler() {
        // The Vulkan face selection table, written out per major axis.
        let oracle = |r: Vec3| -> (usize, f32, f32) {
            let a = r.abs();
            let (face, sc, tc, ma) = if a.x >= a.y && a.x >= a.z {
                if r.x > 0.0 {
                    (0, -r.z, -r.y, a.x)
                } else {
                    (1, r.z, -r.y, a.x)
                }
            } else if a.y >= a.z {
                if r.y > 0.0 {
                    (2, r.x, r.z, a.y)
                } else {
                    (3, r.x, -r.z, a.y)
                }
            } else if r.z > 0.0 {
                (4, r.x, -r.y, a.z)
            } else {
                (5, -r.x, -r.y, a.z)
            };
            (face, 0.5 * (sc / ma + 1.0), 0.5 * (tc / ma + 1.0))
        };
        let pos = Vec3::new(1.0, 2.0, -3.0);
        let far = 20.0;
        for r in [
            Vec3::new(0.3, -0.2, 1.0),
            Vec3::new(-0.7, 0.4, -0.5),
            Vec3::new(0.2, 1.0, 0.3),
            Vec3::new(-0.1, -1.0, 0.4),
            Vec3::new(1.0, 0.5, -0.2),
            Vec3::new(-1.0, 0.1, 0.6),
        ] {
            let (face, u, v) = oracle(r);
            let n = point_face_view_proj(pos, far, face).project_point3(pos + r * 3.0);
            assert!((n.x * 0.5 + 0.5 - u).abs() < 1e-5, "{r:?} u");
            assert!((n.y * 0.5 + 0.5 - v).abs() < 1e-5, "{r:?} v");
            // Depth is the one the shader recomputes from the major-axis distance.
            let z = (r * 3.0).abs().max_element();
            let expect = far * (z - POINT_NEAR) / (z * (far - POINT_NEAR));
            assert!((n.z - expect).abs() < 1e-5, "{r:?} depth");
        }
    }

    #[test]
    fn point_shadow_selection_prefers_near_bright_lights() {
        let p = |x: f32, intensity: f32, cast: bool| Light::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            intensity,
            range: 5.0,
            cast_shadows: cast,
        };
        let lights = [
            p(2.0, 10.0, true),   // 2.5
            p(1.0, 1.0, true),    // 1.0
            p(1.0, 100.0, false), // does not cast
            p(4.0, 100.0, true),  // 6.25
            p(100.0, 1e6, true),  // out of reach
            p(3.0, 20.0, true),   // 2.2
            p(5.0, 20.0, true),   // 0.8
        ];
        assert_eq!(
            select_point_shadows(&lights, Vec3::ZERO, 80.0),
            [3, 0, 5, 1]
        );
    }
}
