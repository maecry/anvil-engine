#version 460
#include "ibl.glsl"

// Analytic sky: zenith/horizon gradient, ground below the horizon, glow and disk for the sun.
layout(push_constant, std430) uniform Push {
    vec4 sun_dir;     // direction the sunlight travels
    vec4 zenith;
    vec4 horizon;
    vec4 ground;
    vec4 sun_color;
} pc;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec3 d = uv_to_dir(v_uv);
    vec3 col = d.y >= 0.0
        ? mix(pc.horizon.rgb, pc.zenith.rgb, pow(clamp(d.y, 0.0, 1.0), 0.4))
        : mix(pc.horizon.rgb, pc.ground.rgb, smoothstep(0.0, 0.08, -d.y));
    float c = dot(d, -normalize(pc.sun_dir.xyz));
    col += pc.sun_color.rgb * (pow(max(c, 0.0), 64.0) * 0.4 + smoothstep(0.9995, 0.9998, c) * 30.0);
    out_color = vec4(col, 1.0);
}
