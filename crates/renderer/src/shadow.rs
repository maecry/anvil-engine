//! Directional shadow volume fitting (ADR-0011 §7).

use foundation::math::camera::rh::view::look_to_mat4;
use foundation::math::{Mat4, Vec3};

use crate::Camera;

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
}
