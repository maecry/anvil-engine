#version 460
#include "common.glsl"

// Depth-only pass: same instance/vertex pulling as forward.vert; pc.view_index selects the
// cascade (0..3), spot (4..7) or point-light cube face (8..31) matrix. uv and material feed the MASK fragment variant.
layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_material;

void main() {
    Vertex v = pc.vertices.v[gl_VertexIndex];
    Instance inst = pc.instances.i[uint(gl_InstanceIndex) & 0x0fffffffu];
    mat4 vp = pc.view_index < 4u ? pc.frame.d.cascade_vp[pc.view_index]
            : pc.view_index < 8u ? pc.frame.d.spot_vp[pc.view_index - 4u] : pc.frame.d.point_vp[pc.view_index - 8u];
    v_uv = vec2(v.uv[0], v.uv[1]);
    v_material = inst.material;
    gl_Position = vp * (inst.model * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0));
}
