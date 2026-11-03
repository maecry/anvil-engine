use assets::AssetId;
use foundation::math::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Entity display name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Name(pub String);

/// Stable identity of an entity across save and load (parent links use it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityUuid(#[serde(with = "assets::u128_str")] pub u128);

/// Local translation / rotation / scale, relative to the parent (or the world).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Translation.
    pub translation: Vec3,
    /// Rotation.
    pub rotation: Quat,
    /// Scale.
    pub scale: Vec3,
}

impl Transform {
    /// No translation, rotation or scale.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// A pure translation.
    pub fn from_translation(t: Vec3) -> Self {
        Self {
            translation: t,
            ..Self::IDENTITY
        }
    }

    /// Local matrix.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// World matrix, written by [`crate::update_transforms`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform {
    /// Object-to-world matrix.
    pub matrix: Mat4,
    /// The matrix of the previous update (equals `matrix` on the first one); feeds TAA velocity.
    pub prev: Mat4,
    /// False until the first update.
    pub valid: bool,
}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self {
            matrix: Mat4::IDENTITY,
            prev: Mat4::IDENTITY,
            valid: false,
        }
    }
}

/// Parent entity. Maintained by [`crate::World::set_parent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub hecs::Entity);

/// Child entities in order. Maintained by [`crate::World::set_parent`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Children(pub Vec<hecs::Entity>);

/// Draws a mesh with a material. `AssetId::NIL` selects the built-in default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshRenderer {
    /// Mesh asset.
    pub mesh: AssetId,
    /// Material asset.
    pub material: AssetId,
}

/// Which punctual light this is. A directional light shines along the entity's -Z; spot cones
/// point along -Z as well.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LightKind {
    /// Infinitely distant.
    Directional,
    /// Omnidirectional.
    Point,
    /// Cone.
    Spot,
}

/// Punctual light; position and direction come from the entity's [`GlobalTransform`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light {
    /// Kind.
    pub kind: LightKind,
    /// Linear RGB color.
    pub color: [f32; 3],
    /// Lux (directional) or candela.
    pub intensity: f32,
    /// Range window (point and spot).
    pub range: f32,
    /// Spot inner half-angle, radians.
    pub inner_cone: f32,
    /// Spot outer half-angle, radians.
    pub outer_cone: f32,
    /// Casts shadows.
    pub cast_shadows: bool,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            color: [1.0; 3],
            intensity: 10.0,
            range: 10.0,
            inner_cone: 0.3,
            outer_cone: 0.5,
            cast_shadows: false,
        }
    }
}

/// Perspective camera looking along the entity's -Z.
// shortcut: the first camera in the world renders, there is no active-camera flag, add one when games need several cameras.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Near plane.
    pub near: f32,
    /// Linear exposure.
    pub exposure: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y: 60f32.to_radians(),
            near: 0.1,
            exposure: 1.0,
        }
    }
}
