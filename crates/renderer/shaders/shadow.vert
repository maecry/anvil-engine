#version 460
#include "common.glsl"

// Depth-only pass: same instance/vertex pulling as forward.vert, light view-projection.
void main() {
    Vertex v = pc.vertices.v[gl_VertexIndex];
    Instance inst = pc.instances.i[gl_InstanceIndex];
    gl_Position = pc.frame.d.light_view_proj * (inst.model * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0));
}
