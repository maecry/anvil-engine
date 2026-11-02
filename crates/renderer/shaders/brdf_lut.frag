#version 460
#include "ibl.glsl"

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

float radical_inverse(uint bits) {
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return float(bits) * 2.3283064365386963e-10;
}

// Split-sum BRDF integration: x = N.V, y = roughness; outputs (scale, bias) of F0.
void main() {
    float n_o_v = max(v_uv.x, 1e-3);
    float rough = max(v_uv.y, 0.02);
    float a = rough * rough;
    vec3 v = vec3(sqrt(1.0 - n_o_v * n_o_v), 0.0, n_o_v);
    const uint SAMPLES = 256u;
    float A = 0.0;
    float B = 0.0;
    for (uint i = 0u; i < SAMPLES; ++i) {
        float u1 = (float(i) + 0.5) / float(SAMPLES);
        float phi = 2.0 * PI * radical_inverse(i);
        float cos_t = sqrt((1.0 - u1) / (1.0 + (a * a - 1.0) * u1));
        float sin_t = sqrt(1.0 - cos_t * cos_t);
        vec3 h = vec3(sin_t * cos(phi), sin_t * sin(phi), cos_t);
        vec3 l = 2.0 * dot(v, h) * h - v;
        float n_o_l = clamp(l.z, 0.0, 1.0);
        float n_o_h = clamp(h.z, 0.0, 1.0);
        float v_o_h = clamp(dot(v, h), 0.0, 1.0);
        if (n_o_l > 0.0) {
            // Smith height-correlated visibility, as in forward.frag.
            float a2 = a * a;
            float gv = n_o_l * sqrt(n_o_v * n_o_v * (1.0 - a2) + a2);
            float gl = n_o_v * sqrt(n_o_l * n_o_l * (1.0 - a2) + a2);
            float vis = 0.5 / max(gv + gl, 1e-5);
            float g_vis = vis * 4.0 * n_o_l * v_o_h / max(n_o_h, 1e-4);
            float fc = pow(1.0 - v_o_h, 5.0);
            A += (1.0 - fc) * g_vis;
            B += fc * g_vis;
        }
    }
    out_color = vec4(A / float(SAMPLES), B / float(SAMPLES), 0.0, 1.0);
}
