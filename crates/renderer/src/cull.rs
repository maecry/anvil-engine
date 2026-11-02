//! CPU frustum culling of bounding spheres against a view-projection matrix (ADR-0012 §11).

use foundation::math::{Mat4, Vec3, Vec4};

/// Six planes (normal xyz, offset w, normalized, pointing inward); a point is inside when
/// `n . p + w >= 0` for all of them.
pub(crate) struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Extracts the planes from `m` (clip volume `-w <= x, y <= w`, `0 <= z <= w`, so it works
    /// for orthographic, perspective and reverse-Z projections alike).
    pub fn from_view_proj(m: Mat4) -> Self {
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let raw = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        let planes = raw.map(|p| {
            let len = p.truncate().length();
            // An infinite far plane has a zero normal: it never culls.
            if len < 1e-6 {
                Vec4::new(0.0, 0.0, 0.0, 1.0)
            } else {
                p / len
            }
        });
        Self { planes }
    }

    /// The six inward planes (normal xyz, offset w) as plain arrays, for the GPU.
    pub fn planes(&self) -> [[f32; 4]; 6] {
        self.planes.map(|p| p.to_array())
    }

    /// True unless the sphere is entirely outside one plane (conservative).
    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }
}

/// Bounding sphere (center, radius) of a vertex cloud: the AABB center and the farthest vertex.
pub(crate) fn bounding_sphere(points: impl Iterator<Item = Vec3> + Clone) -> (Vec3, f32) {
    let (min, max) = points.clone().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    );
    let center = (min + max) * 0.5;
    let radius = points.map(|p| p.distance(center)).fold(0.0, f32::max);
    (center, radius)
}

/// Largest axis scale of `model`.
pub(crate) fn max_scale(model: &Mat4) -> f32 {
    model
        .x_axis
        .truncate()
        .length()
        .max(model.y_axis.truncate().length())
        .max(model.z_axis.truncate().length())
}

/// World-space sphere of an object-space sphere under `model` (radius scaled by the largest axis).
pub(crate) fn transform_sphere(model: &Mat4, center: Vec3, radius: f32) -> (Vec3, f32) {
    (model.transform_point3(center), radius * max_scale(model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::math::Quat;

    use crate::Camera;

    fn cam() -> Camera {
        Camera {
            position: Vec3::new(0.0, 0.0, 5.0),
            rotation: Quat::IDENTITY,
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        }
    }

    #[test]
    fn perspective_reverse_z_culls_outside_spheres() {
        let c = cam();
        let f = Frustum::from_view_proj(c.projection(1.5) * c.view());
        // In front, far away (infinite far plane never culls), and straddling the edge.
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, 0.0), 0.5));
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -5000.0), 0.5));
        assert!(f.intersects_sphere(Vec3::new(3.0, 0.0, 0.0), 1.5));
        // Behind the camera, far to the side, far above.
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, 20.0), 1.0));
        assert!(!f.intersects_sphere(Vec3::new(50.0, 0.0, 0.0), 1.0));
        assert!(!f.intersects_sphere(Vec3::new(0.0, 50.0, 0.0), 1.0));
        // Closer than the near plane but with radius reaching past it still counts.
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, 4.95), 0.2));
    }

    #[test]
    fn orthographic_far_plane_culls() {
        let m = foundation::math::camera::rh::proj::vulkan::orthographic(
            -1.0, 1.0, -1.0, 1.0, 0.0, 10.0,
        );
        let f = Frustum::from_view_proj(m);
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -5.0), 0.1));
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, -20.0), 0.1));
        assert!(!f.intersects_sphere(Vec3::new(3.0, 0.0, -5.0), 0.1));
    }

    #[test]
    fn bounds() {
        let pts = [
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ];
        let (c, r) = bounding_sphere(pts.iter().copied());
        assert!((c - Vec3::new(1.0, 0.5, 0.0)).length() < 1e-6);
        assert!(pts.iter().all(|p| p.distance(c) <= r + 1e-6));
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 1.0, 1.0),
            Quat::IDENTITY,
            Vec3::X,
        );
        let (wc, wr) = transform_sphere(&m, c, r);
        assert!((wc - Vec3::new(3.0, 0.5, 0.0)).length() < 1e-6 && (wr - 2.0 * r).abs() < 1e-6);
    }
}
