#version 460
#extension GL_EXT_nonuniform_qualifier : require

layout(push_constant, std430) uniform Push {
    uint hdr_index;
    uint sampler_index;
    float exposure;
    uint encode_srgb; // 1 when the target is not an sRGB format
} pc;

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

// Narkowicz ACES filmic fit.
vec3 aces(vec3 x) {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

void main() {
    vec3 hdr = texture(sampler2D(textures[nonuniformEXT(pc.hdr_index)],
                                 samplers[nonuniformEXT(pc.sampler_index)]), v_uv).rgb;
    vec3 c = aces(hdr * pc.exposure);
    if (pc.encode_srgb == 1u) c = pow(c, vec3(1.0 / 2.2));
    out_color = vec4(c, 1.0);
}
