#version 460
#include "common.glsl"

// Shared by the prepass, forward, G-buffer and transparent passes: one compiled vertex
// transform, declared invariant, so depth matches bit for bit across passes.
invariant gl_Position;

layout(location = 0) out vec3 v_world_pos;
layout(location = 1) out vec3 v_normal;
layout(location = 2) out vec2 v_uv;
layout(location = 3) out vec4 v_tangent;
layout(location = 4) flat out uint v_material;
layout(location = 5) out vec4 v_clip;       // unjittered clip position
layout(location = 6) out vec4 v_prev_clip;  // previous frame, unjittered (velocity)
layout(location = 7) flat out uint v_flags;  // GPU-driven draws: LOD (bits 0-2) and phase 2 (bit 3), debug views

void main() {
    Vertex v = pc.vertices.v[gl_VertexIndex];
    // GPU-driven draws pack the LOD and the occlusion phase above the instance slot (cull.glsl).
    Instance inst = pc.instances.i[uint(gl_InstanceIndex) & 0x0fffffffu];
    v_flags = uint(gl_InstanceIndex) >> 28;
    vec4 world = inst.model * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0);
    v_world_pos = world.xyz;
    v_normal = mat3(inst.normal_matrix) * vec3(v.normal[0], v.normal[1], v.normal[2]);
    v_uv = vec2(v.uv[0], v.uv[1]);
    v_tangent = vec4(mat3(inst.model) * vec3(v.tangent[0], v.tangent[1], v.tangent[2]), v.tangent[3]);
    v_material = inst.material;
    v_clip = pc.frame.d.unjit_view_proj * world;
    v_prev_clip = pc.frame.d.prev_view_proj * (inst.prev_model * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0));
    gl_Position = pc.frame.d.view_proj * world;
}
