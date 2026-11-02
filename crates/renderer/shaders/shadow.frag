#version 460
// Depth is written by the fixed-function stage; the RHI requires a fragment stage. The MASK
// variant alpha-tests.
#include "surface.glsl"

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_material;

void main() {
#ifdef MASK
    if (surface_alpha(v_material, pc.frame.d.sampler_index, v_uv) < pc.materials.m[v_material].alpha_cutoff) discard;
#endif
}
