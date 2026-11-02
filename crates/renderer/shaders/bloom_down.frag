#version 460
// Bloom downsample: 13-tap filter (Jimenez); the first level uses a Karis average to tame fireflies.
#extension GL_EXT_nonuniform_qualifier : require

layout(push_constant, std430) uniform Push {
    uint src;
    uint smp;
    vec2 inv_dst;   // 1 / destination size
    vec2 texel;     // 1 / source size
    uint karis;
} pc;

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) out vec4 out_color;

vec3 tap(vec2 uv, vec2 o) {
    return texture(sampler2D(textures[nonuniformEXT(pc.src)], samplers[nonuniformEXT(pc.smp)]), uv + o * pc.texel).rgb;
}

float karis_weight(vec3 c) {
    return 1.0 / (1.0 + dot(c, vec3(0.2126, 0.7152, 0.0722)));
}

void main() {
    vec2 uv = gl_FragCoord.xy * pc.inv_dst;
    vec3 a = tap(uv, vec2(-2, 2)), b = tap(uv, vec2(0, 2)), c = tap(uv, vec2(2, 2));
    vec3 d = tap(uv, vec2(-2, 0)), e = tap(uv, vec2(0, 0)), f = tap(uv, vec2(2, 0));
    vec3 g = tap(uv, vec2(-2, -2)), h = tap(uv, vec2(0, -2)), i = tap(uv, vec2(2, -2));
    vec3 j = tap(uv, vec2(-1, 1)), k = tap(uv, vec2(1, 1)), l = tap(uv, vec2(-1, -1)), m = tap(uv, vec2(1, -1));
    vec3 result;
    if (pc.karis == 1u) {
        vec3 g0 = (a + b + d + e) * 0.25, g1 = (b + c + e + f) * 0.25;
        vec3 g2 = (d + e + g + h) * 0.25, g3 = (e + f + h + i) * 0.25;
        vec3 g4 = (j + k + l + m) * 0.25;
        float w0 = karis_weight(g0) * 0.125, w1 = karis_weight(g1) * 0.125;
        float w2 = karis_weight(g2) * 0.125, w3 = karis_weight(g3) * 0.125;
        float w4 = karis_weight(g4) * 0.5;
        result = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
    } else {
        result = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    }
    out_color = vec4(result, 1.0);
}
