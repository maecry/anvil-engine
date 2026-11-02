#version 460
// Opaque, alpha-masked (MASK) and blended forward shading. Blended output is premultiplied:
// diffuse scaled by alpha, specular and emission not.
#include "lighting.glsl"

layout(location = 0) in vec3 v_world_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) in vec4 v_tangent;
layout(location = 4) flat in uint v_material;
layout(location = 7) flat in uint v_flags;
layout(location = 0) out vec4 out_color;

void main() {
    FrameData f = pc.frame.d;
    Surface s = eval_surface(v_material, f.sampler_index, v_uv, v_normal, v_tangent, gl_FrontFacing);
#ifdef MASK
    if (s.alpha < pc.materials.m[v_material].alpha_cutoff) discard;
#endif
    vec3 diff, spec;
    shade(s, v_world_pos, gl_FragCoord.xy, diff, spec);
    vec3 color = debug_view(f, diff * s.alpha + spec, v_world_pos, gl_FragCoord.xy);
    out_color = vec4(color * instance_tint(f.debug_mode, v_flags), s.alpha);
}
