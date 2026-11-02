#version 460
#extension GL_EXT_buffer_reference : require

struct Vertex { vec2 pos; vec2 uv; };
layout(buffer_reference, std430, buffer_reference_align = 8) readonly buffer Vertices { Vertex v[]; };
layout(push_constant, std430) uniform Push { Vertices vertices; uint texture_index; uint sampler_index; } pc;

layout(location = 0) out vec2 v_uv;

void main() {
    Vertex v = pc.vertices.v[gl_VertexIndex];
    gl_Position = vec4(v.pos, 0.0, 1.0);
    v_uv = v.uv;
}
