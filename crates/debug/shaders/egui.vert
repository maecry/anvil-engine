#version 460
#extension GL_EXT_buffer_reference : require
// egui meshes: vertices are pulled from a buffer (device address in the push constants), the
// index buffer is bound normally. Positions are in points; colors are premultiplied sRGB RGBA8.
struct V { float x; float y; float u; float v; uint color; };
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer Verts { V v[]; };
layout(push_constant, std430) uniform Push {
    Verts verts;
    vec2 screen; // target size in points
    uint tex;
    uint smp;
    uint srgb_target;
} pc;

layout(location = 0) out vec4 v_color;
layout(location = 1) out vec2 v_uv;

void main() {
    V v = pc.verts.v[gl_VertexIndex];
    v_color = unpackUnorm4x8(v.color);
    v_uv = vec2(v.u, v.v);
    gl_Position = vec4(2.0 * v.x / pc.screen.x - 1.0, 2.0 * v.y / pc.screen.y - 1.0, 0.0, 1.0);
}
