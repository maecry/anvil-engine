#version 460
#include "common.glsl"

// Depth-only pass: same instance/vertex pulling as forward.vert; pc.view_index selects the
// cascade (0..3) or spot (4..7) matrix. uv and material feed the MASK fragment variant.
layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_material;

void main() {
    Vertex v = pc.vertices.v[gl_VertexIndex];
    Instance inst = pc.instances.i[gl_InstanceIndex];
    FrameData f = pc.frame.d;
    mat4 vp = pc.view_index < 4u ? f.cascade_vp[pc.view_index] : f.spot_vp[pc.view_index - 4u];
    v_uv = vec2(v.uv[0], v.uv[1]);
    v_material = inst.material;
    gl_Position = vp * (inst.model * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0));
}
