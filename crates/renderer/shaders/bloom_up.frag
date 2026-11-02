#version 460
// Bloom upsample: 3x3 tent filter of the coarser level added to the same-size downsample level.
#extension GL_EXT_nonuniform_qualifier : require

layout(push_constant, std430) uniform Push {
    uint coarse;
    uint fine;
    uint smp;
    uint pad;
    vec2 inv_dst;
    vec2 texel_coarse;
} pc;

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) out vec4 out_color;

vec3 coarse_tap(vec2 uv, vec2 o) {
    return texture(sampler2D(textures[nonuniformEXT(pc.coarse)], samplers[nonuniformEXT(pc.smp)]), uv + o * pc.texel_coarse).rgb;
}

void main() {
    vec2 uv = gl_FragCoord.xy * pc.inv_dst;
    vec3 tent = coarse_tap(uv, vec2(-1, 1)) + coarse_tap(uv, vec2(1, 1)) + coarse_tap(uv, vec2(-1, -1)) + coarse_tap(uv, vec2(1, -1));
    tent += 2.0 * (coarse_tap(uv, vec2(0, 1)) + coarse_tap(uv, vec2(0, -1)) + coarse_tap(uv, vec2(-1, 0)) + coarse_tap(uv, vec2(1, 0)));
    tent += 4.0 * coarse_tap(uv, vec2(0, 0));
    vec3 fine = texture(sampler2D(textures[nonuniformEXT(pc.fine)], samplers[nonuniformEXT(pc.smp)]), uv).rgb;
    out_color = vec4(fine + tent / 16.0, 1.0);
}
