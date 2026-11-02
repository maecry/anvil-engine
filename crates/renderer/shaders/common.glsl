// Shared GPU data layout (mirrors crates/renderer/src/gpu.rs). Everything is read through
// buffer device addresses held in push constants (ADR-0010 / ADR-0011 §4).
#extension GL_EXT_buffer_reference : require
#extension GL_EXT_nonuniform_qualifier : require

struct Vertex { float pos[3]; float normal[3]; float uv[2]; float tangent[4]; };
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer Vertices { Vertex v[]; };

struct FrameData {
    mat4 view;
    mat4 proj;
    mat4 view_proj;
    mat4 light_view_proj;       // directional shadow map (orthographic, Y flip folded in)
    vec4 camera_pos;
    uint light_count;
    uint sampler_index;
    uint shadow_index;          // bindless slot of the shadow depth texture
    uint shadow_sampler;        // bindless slot of the comparison sampler
    float exposure;
    uint shadow_light;          // index of the shadow-casting light, ~0u = none
    float shadow_texel_world;   // world size of one shadow texel (normal offset scale)
    float pad;
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Frame { FrameData d; };

struct Instance { mat4 model; mat4 normal_matrix; uint material; uint pad0; uint pad1; uint pad2; };
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Instances { Instance i[]; };

// position_type.w: 0 directional, 1 point, 2 spot. direction = direction the light travels.
// color_intensity.rgb is color * intensity. spot.xy = cos(inner), cos(outer).
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
    uint pad0;
    uint pad1;
    uint pad2;
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Materials { Material m[]; };

// 40 of the 128 push constant bytes.
layout(push_constant, std430) uniform Push {
    Frame frame;
    Instances instances;
    Lights lights;
    Materials materials;
    Vertices vertices;
} pc;
