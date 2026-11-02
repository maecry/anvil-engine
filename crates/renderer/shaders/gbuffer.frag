#version 460
// Deferred G-buffer fill: albedo + metallic, world normal + roughness, emissive + AO.
#include "surface.glsl"

layout(location = 0) in vec3 v_world_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) in vec4 v_tangent;
layout(location = 4) flat in uint v_material;
layout(location = 0) out vec4 out_albedo;
layout(location = 1) out vec4 out_normal;
layout(location = 2) out vec4 out_emissive;

void main() {
    Surface s = eval_surface(v_material, pc.frame.d.sampler_index, v_uv, v_normal, v_tangent, gl_FrontFacing);
#ifdef MASK
    if (s.alpha < pc.materials.m[v_material].alpha_cutoff) discard;
#endif
    out_albedo = vec4(s.base, s.metallic);
    out_normal = vec4(s.n, s.roughness);
    out_emissive = vec4(s.emissive, s.ao);
}
