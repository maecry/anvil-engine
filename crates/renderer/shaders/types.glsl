// Shared GPU data layout (mirrors crates/renderer/src/gpu.rs), without a push constant block so
// compute shaders can declare their own. Everything is read through buffer device addresses held
// in push constants (ADR-0010 / ADR-0011 §4).
#extension GL_EXT_buffer_reference : require
#extension GL_EXT_buffer_reference_uvec2 : require
#extension GL_EXT_nonuniform_qualifier : require

struct Vertex { float pos[3]; float normal[3]; float uv[2]; float tangent[4]; };
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer Vertices { Vertex v[]; };

struct FrameData {
    mat4 view;
    mat4 proj;
    mat4 view_proj;
    mat4 inv_view_proj;         // jittered like view_proj (matches the prepass depth)
    mat4 unjit_view_proj;       // without TAA jitter (velocity, camera reprojection)
    mat4 inv_unjit_view_proj;
    mat4 prev_view_proj;        // previous frame, without jitter
    mat4 cascade_vp[4];         // directional cascades (orthographic, Y flip folded in)
    mat4 spot_vp[4];            // spot shadow maps (perspective, depth 0..1, Y flip folded in)
    vec4 camera_pos;
    vec4 cascade_splits;        // view depth at which each cascade ends
    vec4 cascade_texel;         // world size of one texel per cascade (normal offset scale)
    uint cascade_tex[4];        // bindless slots of the cascade depth textures
    uint spot_tex[4];           // bindless slots of the spot depth textures
    uint light_count;           // directional lights first, then local lights
    uint dir_count;
    uint sampler_index;
    uint shadow_sampler;        // bindless slot of the comparison sampler
    float exposure;
    uint debug_mode;            // 0 none, 1 cluster light count, 2 cascade tint, 3 SSAO, 4 LOD, 5 occlusion phase 2
    uint pad0;
    uint pad1;
    uint sky_index;             // equirect environment textures (bindless slots)
    uint irradiance_index;
    uint brdf_lut_index;
    uint ibl_sampler;           // linear, wrap U, clamp V
    uint prefilter[6];          // one equirect per roughness level 0, 0.2, ... 1
    float env_intensity;
    uint pad2;
    vec4 cam_right;             // xyz = camera right, w = tan(fov_x / 2)
    vec4 cam_up;                // xyz = camera up, w = tan(fov_y / 2)
    vec4 cam_forward;           // xyz = camera forward
    uvec2 cluster_ranges;       // device address of the (offset, count) pairs, one per cluster
    uvec2 cluster_indices;      // device address of the light index list
    vec4 cluster_params;        // near, 24 / ln(far / near), viewport width, height
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Frame { FrameData d; };

// Persistent GPU scene entry (mirrors gpu.rs GpuInstance). sphere = world-space bounds;
// flags: bits 0-1 pipeline variant, bit 2 blended (CPU-drawn), bit 3 valid.
struct Instance { mat4 model; mat4 normal_matrix; mat4 prev_model; vec4 sphere; uint material; uint mesh; uint flags; float scale; };
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Instances { Instance i[]; };

// Mesh arena record (mirrors gpu.rs GpuMesh): LOD index ranges in the shared index buffer, the
// object-space bounding sphere, and each LOD's object-space geometric error (LOD 0 = 0).
struct MeshRecord {
    vec4 sphere;
    uint first_index[4];
    uint index_count[4];
    float lod_error[4];
    int vertex_offset;
    uint lod_count;
    uint pad0;
    uint pad1;
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Meshes { MeshRecord m[]; };

// position_type.w: 0 directional, 1 point, 2 spot. direction = direction the light travels.
// color_intensity.rgb is color * intensity. spot.xy = cos(inner), cos(outer); spot.z = shadow
// slot (spot: 0..3, directional: >= 0 means cascaded shadows, < 0 none); spot.w = tan(outer).
struct Light { vec4 position_type; vec4 direction_range; vec4 color_intensity; vec4 spot; };
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Lights { Light l[]; };

struct Material {
    vec4 base_color;
    vec4 emissive;
    float metallic;
    float roughness;
    float normal_scale;
    float occlusion_strength;
    uint base_color_tex;
    uint mr_tex;
    uint normal_tex;
    uint occlusion_tex;
    uint emissive_tex;
    float alpha_cutoff;
    uint pad1;
    uint pad2;
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Materials { Material m[]; };

layout(buffer_reference, std430, buffer_reference_align = 8) readonly buffer ClusterRanges { uvec2 r[]; };
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer LightIndices { uint i[]; };

