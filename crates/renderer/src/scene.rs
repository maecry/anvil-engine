//! Plain-data extraction types (ADR-0011 §2). The app rebuilds a [`RenderScene`] every frame;
//! the renderer never depends on scene or ECS types.

use foundation::math::{Mat4, Quat, Vec3};

use crate::{MaterialId, MeshId};

/// Perspective camera. World conventions: right-handed, Y-up, forward is -Z (see
/// [`foundation::math`]).
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// World position.
    pub position: Vec3,
    /// World orientation (identity looks down -Z).
    pub rotation: Quat,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    /// Near plane distance; the far plane is at infinity (reverse-Z).
    pub near: f32,
    /// Linear exposure multiplier applied before tonemapping.
    pub exposure: f32,
}

impl Camera {
    /// World-to-view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.position).inverse()
    }

    /// Reverse-Z infinite-far projection with the Vulkan Y flip folded in.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        foundation::math::camera::rh::proj::vulkan::perspective_infinite_reverse(
            self.fov_y, aspect, self.near,
        )
    }
}

/// One drawn instance of a mesh.
#[derive(Clone, Copy, Debug)]
pub struct MeshInstance {
    /// Mesh to draw.
    pub mesh: MeshId,
    /// Material to draw it with.
    pub material: MaterialId,
    /// Object-to-world transform.
    pub transform: Mat4,
}

/// A punctual light with glTF `KHR_lights_punctual` semantics.
#[derive(Clone, Copy, Debug)]
pub enum Light {
    /// Infinitely distant light.
    Directional {
        /// Direction the light travels (from the light toward the scene).
        direction: Vec3,
        /// Linear RGB color.
        color: Vec3,
        /// Intensity multiplier (lux).
        intensity: f32,
        /// Casts the shadow map. Only the first such light in the scene gets shadows.
        cast_shadows: bool,
    },
    /// Omnidirectional point light with inverse-square falloff.
    Point {
        /// World position.
        position: Vec3,
        /// Linear RGB color.
        color: Vec3,
        /// Intensity multiplier (candela).
        intensity: f32,
        /// Range window: contribution reaches zero at this distance.
        range: f32,
    },
    /// Cone light with inverse-square falloff.
    Spot {
        /// World position.
        position: Vec3,
        /// Direction the cone points.
        direction: Vec3,
        /// Linear RGB color.
        color: Vec3,
        /// Intensity multiplier (candela).
        intensity: f32,
        /// Range window: contribution reaches zero at this distance.
        range: f32,
        /// Half-angle (radians) of the full-intensity cone.
        inner_cone: f32,
        /// Half-angle (radians) where the intensity reaches zero.
        outer_cone: f32,
    },
}

/// Maximum lights shaded per frame; extra lights are ignored (with a warning).
pub const MAX_LIGHTS: usize = 64;

/// Everything the renderer needs for one frame.
#[derive(Clone, Debug)]
pub struct RenderScene {
    /// Viewpoint.
    pub camera: Camera,
    /// Instances to draw.
    pub meshes: Vec<MeshInstance>,
    /// Lights, at most [`MAX_LIGHTS`] are shaded.
    pub lights: Vec<Light>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera {
            position: Vec3::new(0.0, 0.0, 5.0),
            rotation: Quat::IDENTITY,
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        }
    }

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        m.project_point3(p)
    }

    #[test]
    fn reverse_z_and_y_flip() {
        let c = cam();
        let vp = c.projection(1.0) * c.view();
        // Camera at z=5 looks down -Z: near plane at z=4.9 maps to depth 1, far away to ~0.
        assert!((ndc(vp, Vec3::new(0.0, 0.0, 4.9)).z - 1.0).abs() < 1e-4);
        assert!(ndc(vp, Vec3::new(0.0, 0.0, -1.0e6)).z < 1e-5);
        // World +Y is up on screen: negative NDC y in Vulkan.
        assert!(ndc(vp, Vec3::new(0.0, 1.0, 0.0)).y < 0.0);
        // World +X is right.
        assert!(ndc(vp, Vec3::new(1.0, 0.0, 0.0)).x > 0.0);
    }
}
