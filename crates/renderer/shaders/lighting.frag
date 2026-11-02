#version 460
// Deferred lighting: fullscreen pass reading the G-buffer and depth (slots in pc.gbuf).
// Sky pixels (depth 0, reverse-Z) are left to the skybox pass.
#include "lighting.glsl"

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    FrameData f = pc.frame.d;
    ivec2 px = ivec2(gl_FragCoord.xy);
    float depth = texelFetch(textures[nonuniformEXT(pc.gbuf.w)], px, 0).r;
    if (depth == 0.0) discard;
    vec4 a = texelFetch(textures[nonuniformEXT(pc.gbuf.x)], px, 0);
    vec4 n = texelFetch(textures[nonuniformEXT(pc.gbuf.y)], px, 0);
    vec4 e = texelFetch(textures[nonuniformEXT(pc.gbuf.z)], px, 0);
    Surface s;
    s.base = a.rgb;
    s.alpha = 1.0;
    s.metallic = a.a;
    s.n = normalize(n.xyz);
    s.roughness = n.w;
    s.emissive = e.rgb;
    s.ao = e.a;
    vec2 ndc = (gl_FragCoord.xy / f.cluster_params.zw) * 2.0 - 1.0;
    vec4 w = f.inv_view_proj * vec4(ndc, depth, 1.0);
    vec3 world_pos = w.xyz / w.w;
    vec3 diff, spec;
    shade(s, world_pos, gl_FragCoord.xy, diff, spec);
    out_color = vec4(debug_view(f, diff + spec, world_pos, gl_FragCoord.xy), 1.0);
}
