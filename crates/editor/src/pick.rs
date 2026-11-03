//! Viewport math: camera rays, ray/box tests, projection to the viewport and the translate
//! gizmo's axis drag. Pure functions over [`renderer::Camera`] so they are unit-tested without a GPU.

use foundation::math::{Mat4, Vec3};
use renderer::Camera;

/// A ray with a unit direction.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    /// Start.
    pub origin: Vec3,
    /// Unit direction.
    pub dir: Vec3,
}

/// The ray through the viewport position `uv` (0..1, origin top-left) of a view with `aspect`.
pub fn camera_ray(cam: &Camera, uv: (f32, f32), aspect: f32) -> Ray {
    let t = (cam.fov_y * 0.5).tan();
    let (nx, ny) = (uv.0 * 2.0 - 1.0, 1.0 - uv.1 * 2.0);
    let local = Vec3::new(nx * t * aspect, ny * t, -1.0);
    Ray {
        origin: cam.position,
        dir: (cam.rotation * local).normalize(),
    }
}

/// Viewport position (0..1, origin top-left) of a world point, `None` behind the camera.
pub fn project(cam: &Camera, p: Vec3, aspect: f32) -> Option<(f32, f32)> {
    let local = cam.rotation.inverse() * (p - cam.position);
    if local.z >= -1e-4 {
        return None;
    }
    let t = (cam.fov_y * 0.5).tan();
    let (nx, ny) = (local.x / -local.z / (t * aspect), local.y / -local.z / t);
    Some((nx * 0.5 + 0.5, 0.5 - ny * 0.5))
}

/// Distance along the ray to the box `min..max`, `None` on a miss.
pub fn ray_aabb(ray: &Ray, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = ray.dir.recip();
    let (a, b) = ((min - ray.origin) * inv, (max - ray.origin) * inv);
    let near = a.min(b).max_element();
    let far = a.max(b).min_element();
    (far >= near.max(0.0)).then_some(near.max(0.0))
}

/// World-space box of the local box `min..max` under `m`.
pub fn transform_aabb(m: &Mat4, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    let mut out = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        );
        let w = m.transform_point3(c);
        out = (out.0.min(w), out.1.max(w));
    }
    out
}

/// Parameter `t` of the point `line_origin + t * axis` closest to the ray (both directions unit
/// length), `None` when they are nearly parallel.
pub fn axis_param(ray: &Ray, line_origin: Vec3, axis: Vec3) -> Option<f32> {
    let w = ray.origin - line_origin;
    let (b, d, e) = (ray.dir.dot(axis), ray.dir.dot(w), axis.dot(w));
    let denom = 1.0 - b * b;
    if denom < 1e-4 {
        return None;
    }
    let s = (b * e - d) / denom;
    Some(e + s * b)
}

/// Distance from `p` to the segment `a..b` (2D).
pub fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ab, ap) = ((b.0 - a.0, b.1 - a.1), (p.0 - a.0, p.1 - a.1));
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if len2 < 1e-9 {
        0.0
    } else {
        ((ap.0 * ab.0 + ap.1 * ab.1) / len2).clamp(0.0, 1.0)
    };
    let (dx, dy) = (ap.0 - ab.0 * t, ap.1 - ab.1 * t);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::math::Quat;

    fn cam() -> Camera {
        Camera {
            position: Vec3::new(1.0, 2.0, 5.0),
            rotation: Quat::from_rotation_y(0.2) * Quat::from_rotation_x(-0.1),
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        }
    }

    #[test]
    fn project_and_ray_agree() {
        let c = cam();
        let p = Vec3::new(0.5, 1.0, -1.0);
        let uv = project(&c, p, 1.6).unwrap();
        let ray = camera_ray(&c, uv, 1.6);
        let along = (p - ray.origin).dot(ray.dir);
        assert!((ray.origin + ray.dir * along - p).length() < 1e-3);
        assert!(project(&c, c.position + c.rotation * Vec3::Z * 3.0, 1.0).is_none());
    }

    #[test]
    fn ray_hits_and_misses_boxes() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            dir: Vec3::NEG_Z,
        };
        let t = ray_aabb(&ray, Vec3::splat(-1.0), Vec3::splat(1.0)).unwrap();
        assert!((t - 4.0).abs() < 1e-5);
        assert!(ray_aabb(&ray, Vec3::new(2.0, 2.0, -1.0), Vec3::new(3.0, 3.0, 1.0)).is_none());
        // Box behind the origin is not hit.
        assert!(ray_aabb(&ray, Vec3::new(-1.0, -1.0, 6.0), Vec3::new(1.0, 1.0, 7.0)).is_none());
        let (lo, hi) = transform_aabb(
            &Mat4::from_translation(Vec3::X * 10.0),
            Vec3::splat(-1.0),
            Vec3::splat(1.0),
        );
        assert_eq!(
            (lo, hi),
            (Vec3::new(9.0, -1.0, -1.0), Vec3::new(11.0, 1.0, 1.0))
        );
    }

    #[test]
    fn axis_drag_recovers_the_cursor_position_along_the_axis() {
        let c = cam();
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let target = origin + Vec3::X * 1.7;
        let uv = project(&c, target, 1.0).unwrap();
        let t = axis_param(&camera_ray(&c, uv, 1.0), origin, Vec3::X).unwrap();
        assert!((t - 1.7).abs() < 1e-3, "{t}");
        let parallel = Ray {
            origin: Vec3::new(0.0, 1.0, 0.0),
            dir: Vec3::X,
        };
        assert!(axis_param(&parallel, origin, Vec3::X).is_none());
    }

    #[test]
    fn segment_distance_clamps_to_the_ends() {
        assert!((segment_distance((0.5, 1.0), (0.0, 0.0), (1.0, 0.0)) - 1.0).abs() < 1e-6);
        assert!((segment_distance((3.0, 0.0), (0.0, 0.0), (1.0, 0.0)) - 2.0).abs() < 1e-6);
    }
}
