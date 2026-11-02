#version 460
// Depth prepass: depth comes from the fixed-function stage, the color target holds the
// view-space normal, octahedral encoded (for SSAO). MASK variant alpha-tests.
#include "surface.glsl"

layout(location = 0) in vec3 v_world_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) in vec4 v_tangent;
layout(location = 4) flat in uint v_material;
layout(location = 0) out vec2 out_normal;

vec2 oct_encode(vec3 n) {
    n /= abs(n.x) + abs(n.y) + abs(n.z);
    vec2 e = n.xy;
    if (n.z < 0.0) e = (1.0 - abs(e.yx)) * vec2(e.x >= 0.0 ? 1.0 : -1.0, e.y >= 0.0 ? 1.0 : -1.0);
    return e;
}

void main() {
#ifdef MASK
    if (surface_alpha(v_material, pc.frame.d.sampler_index, v_uv) < pc.materials.m[v_material].alpha_cutoff) discard;
#endif
    vec3 n = normalize(v_normal) * (gl_FrontFacing ? 1.0 : -1.0);
    out_normal = oct_encode(normalize(mat3(pc.frame.d.view) * n));
}
