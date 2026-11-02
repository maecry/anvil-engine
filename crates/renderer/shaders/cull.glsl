// Shared by the GPU culling compute shaders (cull_camera.comp, cull_shadow.comp): parameters,
// frustum test, LOD selection and indirect command emission. Mirrors gpu.rs GpuCullParams.
#include "types.glsl"

layout(buffer_reference, std430, buffer_reference_align = 4) buffer Visible { uint v[]; };
layout(buffer_reference, std430, buffer_reference_align = 4) buffer Counts { uint c[]; };
layout(buffer_reference, std430, buffer_reference_align = 4) buffer Commands { uint w[]; };

// Views: 0 camera phase 1, 1 camera phase 2, 2..5 shadow cascades, 6..9 spot shadows. Each view
// has one command region and one counter per pipeline variant: index view * 4 + variant.
// counts.c[40..] are statistics: visible, frustum culled, occlusion culled, triangles, camera
// draws, phase 2 draws, shadow draws.
const uint STATS = 40u;
struct CullParams {
    vec4 planes[54];            // frustum planes (inward) of 9 views: camera, then 8 shadow views
    mat4 view;                  // unjittered camera view
    vec4 proj;                  // P00, P11, near, LOD error threshold in pixels (0 = always LOD 0)
    vec4 cam;                   // camera position, pixels per world unit at distance 1
    uvec4 pyramid;              // bindless slot of the depth pyramid chain, mip 0 size, mip count
    Instances instances;
    Meshes meshes;
    Visible visible;
    uint instance_count;
    uint view_mask;             // shadow views to cull for (bit k = shadow view 2 + k)
    uint pad0;
    uint pad1;
    uint region[40];            // first command of each (view, variant) region
};
layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Params { CullParams p; };

bool in_frustum(Params P, uint plane_set, vec4 s) {
    for (uint k = 0u; k < 6u; ++k) {
        vec4 pl = P.p.planes[plane_set * 6u + k];
        if (dot(pl.xyz, s.xyz) + pl.w < -s.w) return false;
    }
    return true;
}

// Coarsest LOD whose projected geometric error is within the threshold.
uint select_lod(Params P, MeshRecord m, Instance inst) {
    float threshold = P.p.proj.w;
    if (threshold <= 0.0) return 0u;
    float dist = max(distance(inst.sphere.xyz, P.p.cam.xyz) - inst.sphere.w, 0.05);
    for (uint l = m.lod_count - 1u; l > 0u; --l) {
        if (m.lod_error[l] * inst.scale / dist * P.p.cam.w <= threshold) return l;
    }
    return 0u;
}

// Appends one draw of `slot` at `lod` to the (view, variant) region. `tag` = bit 31 marks a
// phase 2 draw; bits 28-30 carry the LOD for the debug views (see forward.vert).
void emit_draw(Params P, Commands cmds, Counts counts, uint view, uint variant, MeshRecord m, uint lod, uint slot, uint tag) {
    uint r = view * 4u + variant;
    uint n = atomicAdd(counts.c[r], 1u);
    uint o = (P.p.region[r] + n) * 5u;
    cmds.w[o + 0u] = m.index_count[lod];
    cmds.w[o + 1u] = 1u;
    cmds.w[o + 2u] = m.first_index[lod];
    cmds.w[o + 3u] = uint(m.vertex_offset);
    cmds.w[o + 4u] = slot | (lod << 28) | tag;
}
