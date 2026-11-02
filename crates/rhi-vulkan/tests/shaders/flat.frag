#version 460

layout(push_constant, std430) uniform Push { uvec2 vertices; vec4 color; float z; } pc;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = pc.color;
}
