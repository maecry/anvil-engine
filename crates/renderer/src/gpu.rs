//! GPU-side layouts, mirrored by `shaders/common.glsl` (std430).

use bytemuck::{Pod, Zeroable};
use foundation::math::Mat4;

use crate::Light;

/// Per-frame data.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuFrame {
    pub view: Mat4,
    pub proj: Mat4,
    pub view_proj: Mat4,
    pub inv_view_proj: Mat4,
    pub unjit_view_proj: Mat4,
    pub inv_unjit_view_proj: Mat4,
    pub prev_view_proj: Mat4,
    pub cascade_vp: [Mat4; 4],
    pub spot_vp: [Mat4; 4],
    /// Faces of the point-light cube maps: light `i` face `f` at `i * 6 + f`.
    pub point_vp: [Mat4; 24],
    pub camera_pos: [f32; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_texel: [f32; 4],
    pub cascade_tex: [u32; 4],
    pub spot_tex: [u32; 4],
    /// Bindless slots of the point-light shadow cube maps.
    pub point_tex: [u32; 4],
    pub light_count: u32,
    pub dir_count: u32,
    pub sampler_index: u32,
    pub shadow_sampler: u32,
    pub exposure: f32,
    pub debug_mode: u32,
    pub pad0: u32,
    pub pad1: u32,
    pub sky_index: u32,
    pub irradiance_index: u32,
    pub brdf_lut_index: u32,
    pub ibl_sampler: u32,
    pub prefilter: [u32; 6],
    pub env_intensity: f32,
    pub pad2: u32,
    pub cam_right: [f32; 4],
    pub cam_up: [f32; 4],
    pub cam_forward: [f32; 4],
    pub cluster_ranges: [u32; 2],
    pub cluster_indices: [u32; 2],
    pub cluster_params: [f32; 4],
}

/// Persistent per-instance data (`Instance` in `types.glsl`): transforms, world bounds and the
/// ids the GPU culling pass needs. Lives in the persistent instance buffer, indexed by the
/// instance's position in the scene.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    pub model: Mat4,
    pub normal_matrix: Mat4,
    pub prev_model: Mat4,
    /// World-space bounding sphere: center xyz, radius w.
    pub sphere: [f32; 4],
    pub material: u32,
    /// Index of the mesh record in the mesh table.
    pub mesh: u32,
    /// Bits 0-1 pipeline variant, bit 2 blended, bit 3 valid ([`FLAG_VALID`], [`FLAG_BLEND`]).
    pub flags: u32,
    /// Largest axis scale of `model` (object error to world error).
    pub scale: f32,
}

/// Instance has a resolvable mesh and material.
pub(crate) const FLAG_VALID: u32 = 8;
/// Instance is alpha-blended: drawn by the CPU transparent pass, never by GPU culling.
pub(crate) const FLAG_BLEND: u32 = 4;

/// A mesh arena record (`MeshRecord` in `types.glsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuMesh {
    /// Object-space bounding sphere: center xyz, radius w.
    pub sphere: [f32; 4],
    pub first_index: [u32; 4],
    pub index_count: [u32; 4],
    /// Object-space error of each level of detail (level 0 = 0).
    pub lod_error: [f32; 4],
    pub vertex_offset: i32,
    pub lod_count: u32,
    pub pad: [u32; 2],
}

/// Shadow views: 4 cascades, 4 spot lights and 4 point lights with 6 faces each.
pub(crate) const SHADOW_VIEWS: usize = 32;
/// Number of culling views: camera phase 1 and 2, then the shadow views.
pub(crate) const CULL_VIEWS: usize = 2 + SHADOW_VIEWS;
/// Pipeline variants per view (`Draw::variant`).
pub(crate) const VARIANTS: usize = 4;
/// Index of the first statistics counter after the `CULL_VIEWS * VARIANTS` draw counters.
pub(crate) const STATS: usize = CULL_VIEWS * VARIANTS;
/// Statistics words: visible, frustum culled, occlusion culled, triangles, camera draws,
/// phase 2 draws, shadow draws, spare.
pub(crate) const STAT_WORDS: usize = 8;
/// Bytes of the counts buffer (draw counters then statistics).
pub(crate) const COUNTS_BYTES: u64 = ((STATS + STAT_WORDS) * 4) as u64;

/// Parameters of the culling compute passes (`CullParams` in `cull.glsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuCullParams {
    /// Inward frustum planes of the camera and then every shadow view.
    pub planes: [[[f32; 4]; 6]; 1 + SHADOW_VIEWS],
    pub view: Mat4,
    /// P00, P11, near, LOD error threshold in pixels.
    pub proj: [f32; 4],
    /// Camera position, pixels per world unit at distance 1.
    pub cam: [f32; 4],
    /// Depth pyramid: bindless slot, mip 0 width and height, mip count.
    pub pyramid: [u32; 4],
    pub instances: [u32; 2],
    pub meshes: [u32; 2],
    pub visible: [u32; 2],
    pub instance_count: u32,
    pub view_mask: u32,
    pub pad: [u32; 2],
    /// First command of each (view, variant) region.
    pub region: [[u32; VARIANTS]; CULL_VIEWS],
    pub pad2: [u32; 2],
}

/// One light; see `common.glsl` for the field meaning.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuLight {
    pub position_type: [f32; 4],
    pub direction_range: [f32; 4],
    pub color_intensity: [f32; 4],
    pub spot: [f32; 4],
}

impl GpuLight {
    pub fn new(l: &Light) -> Self {
        match *l {
            Light::Directional {
                direction,
                color,
                intensity,
                ..
            } => Self {
                position_type: [0.0; 4],
                direction_range: [direction.x, direction.y, direction.z, 0.0],
                color_intensity: [
                    color.x * intensity,
                    color.y * intensity,
                    color.z * intensity,
                    0.0,
                ],
                // Cascaded shadows are flagged by the renderer on the shadow caster.
                spot: [0.0, 0.0, -1.0, 0.0],
            },
            Light::Point {
                position,
                color,
                intensity,
                range,
                ..
            } => Self {
                position_type: [position.x, position.y, position.z, 1.0],
                direction_range: [0.0, 0.0, 0.0, range],
                color_intensity: [
                    color.x * intensity,
                    color.y * intensity,
                    color.z * intensity,
                    0.0,
                ],
                spot: [0.0, 0.0, -1.0, 0.0],
            },
            Light::Spot {
                position,
                direction,
                color,
                intensity,
                range,
                inner_cone,
                outer_cone,
                ..
            } => Self {
                position_type: [position.x, position.y, position.z, 2.0],
                direction_range: [direction.x, direction.y, direction.z, range],
                color_intensity: [
                    color.x * intensity,
                    color.y * intensity,
                    color.z * intensity,
                    0.0,
                ],
                spot: [inner_cone.cos(), outer_cone.cos(), -1.0, outer_cone.tan()],
            },
        }
    }
}

/// One material (glTF metallic-roughness); texture fields are bindless image indices.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuMaterial {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub base_color_tex: u32,
    pub mr_tex: u32,
    pub normal_tex: u32,
    pub occlusion_tex: u32,
    pub emissive_tex: u32,
    pub alpha_cutoff: f32,
    /// Resident base mip level of the base color texture (debug view `mips`).
    pub base_mip: u32,
    pub pad: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_std430() {
        assert_eq!(size_of::<GpuFrame>(), 2752);
        assert_eq!(size_of::<GpuInstance>(), 224);
        assert_eq!(size_of::<GpuMesh>(), 80);
        assert_eq!(size_of::<GpuCullParams>(), 3872);
        assert_eq!(size_of::<GpuLight>(), 64);
        assert_eq!(size_of::<GpuMaterial>(), 80);
    }
}
