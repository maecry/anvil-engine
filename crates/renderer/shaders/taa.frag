#version 460
// TAA resolve in HDR. pc.gbuf = (current color, depth, velocity, history); pc.view_index = 1
// resets (no history). History alpha holds the linear depth of the pixel for disocclusion tests.
#include "bindings.glsl"

layout(location = 0) out vec4 out_color;

vec3 to_ycocg(vec3 c) {
    return vec3(0.25 * c.r + 0.5 * c.g + 0.25 * c.b, 0.5 * c.r - 0.5 * c.b, -0.25 * c.r + 0.5 * c.g - 0.25 * c.b);
}
vec3 from_ycocg(vec3 c) {
    return vec3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

void main() {
    FrameData f = pc.frame.d;
    ivec2 size = ivec2(f.cluster_params.zw);
    ivec2 px = ivec2(gl_FragCoord.xy);
    vec2 uv = gl_FragCoord.xy / f.cluster_params.zw;
    vec2 ndc = uv * 2.0 - 1.0;
    uint hdr_i = pc.gbuf.x, depth_i = pc.gbuf.y, vel_i = pc.gbuf.z, hist_i = pc.gbuf.w;

    vec3 cur = texelFetch(textures[nonuniformEXT(hdr_i)], px, 0).rgb;
    vec3 mn = vec3(1e9), mx = vec3(-1e9);
    float closest = -1.0;
    ivec2 cpx = px;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            ivec2 q = clamp(px + ivec2(x, y), ivec2(0), size - 1);
            vec3 c = to_ycocg(texelFetch(textures[nonuniformEXT(hdr_i)], q, 0).rgb);
            mn = min(mn, c);
            mx = max(mx, c);
            float d = texelFetch(textures[nonuniformEXT(depth_i)], q, 0).r;
            if (d > closest) { closest = d; cpx = q; }   // reverse-Z: larger = nearer
        }
    }
    float depth = texelFetch(textures[nonuniformEXT(depth_i)], px, 0).r;
    float lin = depth == 0.0 ? 0.0 : f.cluster_params.x / depth;

    // Camera-only reprojection (exact for static geometry and the sky, homogeneous at infinity).
    vec4 w4 = f.inv_unjit_view_proj * vec4(ndc, depth, 1.0);
    vec4 prev_clip = f.prev_view_proj * w4;
    vec2 cam_vel = uv - (prev_clip.xy / prev_clip.w * 0.5 + 0.5);
    vec2 vel = closest == 0.0 ? cam_vel : texelFetch(textures[nonuniformEXT(vel_i)], cpx, 0).rg;

    vec2 huv = uv - vel;
    bool valid = pc.view_index == 0u && all(greaterThanEqual(huv, vec2(0.0))) && all(lessThanEqual(huv, vec2(1.0)));
    vec3 result = cur;
    if (valid) {
        vec4 hist_n = texelFetch(textures[nonuniformEXT(hist_i)], clamp(ivec2(huv * vec2(size)), ivec2(0), size - 1), 0);
        // Disocclusion: static pixels must find the depth the camera motion predicts.
        float expected = prev_clip.w;
        bool is_static = length(vel - cam_vel) < 1e-4;
        if (is_static && depth != 0.0 && abs(hist_n.a - expected) > 0.1 * expected + 0.05) valid = false;
    }
    if (valid) {
        vec3 hist = texture(sampler2D(textures[nonuniformEXT(hist_i)], samplers[nonuniformEXT(f.sampler_index)]), huv).rgb;
        hist = from_ycocg(clamp(to_ycocg(hist), mn, mx));
        result = mix(hist, cur, 0.1);
    }
    out_color = vec4(result, lin);
}
