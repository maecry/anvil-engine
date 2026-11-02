#version 460
#extension GL_EXT_nonuniform_qualifier : require
#include "ibl.glsl"

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(push_constant, std430) uniform Push { uint sky_index; uint sampler_index; float roughness; uint pad; } pc;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

vec3 sky(vec3 d) {
    return min(textureLod(sampler2D(textures[nonuniformEXT(pc.sky_index)],
                                    samplers[nonuniformEXT(pc.sampler_index)]), dir_to_uv(d), 0.0).rgb,
               vec3(4.0));
}

float radical_inverse(uint bits) {
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return float(bits) * 2.3283064365386963e-10;
}

// GGX importance sampling with V = N = R (split-sum prefilter).
void main() {
    vec3 n = uv_to_dir(v_uv);
    vec3 up = abs(n.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
    vec3 t = normalize(cross(up, n));
    vec3 b = cross(n, t);
    float a = pc.roughness * pc.roughness;
    const uint SAMPLES = 192u;
    vec3 sum = vec3(0.0);
    float weight = 0.0;
    for (uint i = 0u; i < SAMPLES; ++i) {
        float u1 = (float(i) + 0.5) / float(SAMPLES);
        float u2 = radical_inverse(i);
        float phi = 2.0 * PI * u2;
        float cos_t = sqrt((1.0 - u1) / (1.0 + (a * a - 1.0) * u1));
        float sin_t = sqrt(1.0 - cos_t * cos_t);
        vec3 h = t * (sin_t * cos(phi)) + b * (sin_t * sin(phi)) + n * cos_t;
        vec3 l = reflect(-n, h);
        float n_o_l = dot(n, l);
        if (n_o_l > 0.0) {
            sum += sky(l) * n_o_l;
            weight += n_o_l;
        }
    }
    out_color = vec4(sum / max(weight, 1e-4), 1.0);
}
