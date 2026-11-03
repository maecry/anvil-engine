#version 460
// Debug lines: vertices are pulled from a buffer (device address in the push constants); colors are
// RGBA8. Uses the camera's jittered view-projection so lines match the depth buffer.
#include "types.glsl"

struct DebugVertex { float pos[3]; uint color; };
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer DebugVertices { DebugVertex v[]; };
layout(push_constant, std430) uniform Push { Frame frame; DebugVertices verts; } pc;

layout(location = 0) out vec4 v_color;

void main() {
    DebugVertex v = pc.verts.v[gl_VertexIndex];
    v_color = unpackUnorm4x8(v.color);
    gl_Position = pc.frame.d.view_proj * vec4(v.pos[0], v.pos[1], v.pos[2], 1.0);
}
