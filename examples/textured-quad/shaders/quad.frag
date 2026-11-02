#version 460
#extension GL_EXT_nonuniform_qualifier : require

layout(push_constant, std430) uniform Push {
    uvec2 vertices;
    uint texture_index;
    uint sampler_index;
    vec2 scale;
} pc;

// Global bindless set 0 (ADR-0010): sampled images at binding 0, samplers at binding 1.
layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(sampler2D(textures[nonuniformEXT(pc.texture_index)],
                                  samplers[nonuniformEXT(pc.sampler_index)]), v_uv);
}
