//! Directional shadow volume fitting (ADR-0011 §7).

use foundation::math::camera::rh::{proj::vulkan::orthographic, view::look_to_mat4};
use foundation::math::{Mat4, Vec3};

use crate::Camera;

/// Result of [`fit_directional`].
pub(crate) struct ShadowFit {
    /// World to shadow-map clip space (orthographic, Y flip folded in, depth 0..1).
    pub view_proj: Mat4,
    /// World size of one shadow texel.
    pub texel_world: f32,
}

/// Fits an orthographic light volume around the camera frustum up to `max_distance`, as a
/// bounding sphere (stable under camera rotation) snapped to whole texels (stable under
/// translation), so the shadow edges do not shimmer. `dir` is the direction the light travels.
pub(crate) fn fit_directional(
    camera: &Camera,
    aspect: f32,
    dir: Vec3,
    max_distance: f32,
    resolution: u32,
) -> ShadowFit {
    let dir = dir.normalize();
    let tan_y = (camera.fov_y * 0.5).tan();
    let tan_x = tan_y * aspect;
    let to_world = camera.view().inverse();
    let mut corners = [Vec3::ZERO; 8];
    for (i, c) in corners.iter_mut().enumerate() {
        let d = if i < 4 { camera.near } else { max_distance };
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
        let fit = fit_directional(&c, 1.5, Vec3::new(-0.5, -1.0, -0.4), 50.0, 2048);
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
        let a = fit_directional(&cam(Vec3::new(0.0, 3.0, 9.0)), 1.5, d, 50.0, 2048);
        let b = fit_directional(
            &cam(Vec3::new(0.0, 3.0, 9.0) + Vec3::X * 0.001),
            1.5,
            d,
            50.0,
            2048,
        );
        // A point's shadow-map texel coordinate does not change for sub-texel camera motion.
        let p = Vec3::new(2.0, 0.0, 1.0);
        let t = |f: &ShadowFit| (f.view_proj.project_point3(p).truncate() * 1024.0).round();
        assert_eq!(t(&a), t(&b));
    }
}
