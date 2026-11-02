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
    pub cascade_vp: [Mat4; 4],
    pub spot_vp: [Mat4; 4],
    pub camera_pos: [f32; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_texel: [f32; 4],
    pub cascade_tex: [u32; 4],
    pub spot_tex: [u32; 4],
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

/// Per-instance data.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    pub model: Mat4,
    pub normal_matrix: Mat4,
    pub material: u32,
    pub pad: [u32; 3],
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
    pub pad: [u32; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_std430() {
        assert_eq!(size_of::<GpuFrame>(), 1008);
        assert_eq!(size_of::<GpuInstance>(), 144);
        assert_eq!(size_of::<GpuLight>(), 64);
        assert_eq!(size_of::<GpuMaterial>(), 80);
    }
}
