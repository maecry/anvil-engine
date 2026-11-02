#version 460
#extension GL_EXT_nonuniform_qualifier : require
#include "ibl.glsl"

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(push_constant, std430) uniform Push { uint sky_index; uint sampler_index; } pc;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

// Sun disk and other very bright texels are clamped: the sun is a directional light.
vec3 sky(vec3 d) {
    return min(textureLod(sampler2D(textures[nonuniformEXT(pc.sky_index)],
                                    samplers[nonuniformEXT(pc.sampler_index)]), dir_to_uv(d), 0.0).rgb,
               vec3(4.0));
}

// Cosine convolution on a uniform (phi, theta) grid; stores E / pi so the shader multiplies by
// the albedo only.
void main() {
    vec3 n = uv_to_dir(v_uv);
    vec3 up = abs(n.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
    vec3 t = normalize(cross(up, n));
    vec3 b = cross(n, t);
    const int NP = 48;
    const int NT = 16;
    vec3 sum = vec3(0.0);
    for (int i = 0; i < NP; ++i) {
        float phi = (float(i) + 0.5) / float(NP) * 2.0 * PI;
        for (int j = 0; j < NT; ++j) {
            float theta = (float(j) + 0.5) / float(NT) * 0.5 * PI;
            vec3 l = t * (sin(theta) * cos(phi)) + b * (sin(theta) * sin(phi)) + n * cos(theta);
            sum += sky(l) * cos(theta) * sin(theta);
        }
    }
    out_color = vec4(sum * PI / float(NP * NT), 1.0);
}
