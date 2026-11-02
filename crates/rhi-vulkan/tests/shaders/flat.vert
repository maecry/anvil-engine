#version 460
#extension GL_EXT_buffer_reference : require

struct Vertex { vec2 pos; vec2 uv; };
layout(buffer_reference, std430, buffer_reference_align = 8) readonly buffer Vertices { Vertex v[]; };
layout(push_constant, std430) uniform Push { Vertices vertices; vec4 color; float z; } pc;

void main() {
    gl_Position = vec4(pc.vertices.v[gl_VertexIndex].pos, pc.z, 1.0);
}
