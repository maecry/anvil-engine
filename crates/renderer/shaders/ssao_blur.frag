#version 460
// Bilateral (depth-aware) blur fused with the upsample from half to full resolution.
// pc.gbuf = (half-resolution AO, depth).
#include "bindings.glsl"

layout(location = 0) out float out_ao;

void main() {
    FrameData f = pc.frame.d;
    ivec2 full = ivec2(f.cluster_params.zw);
    ivec2 half_size = (full + 1) / 2;
    ivec2 px = ivec2(gl_FragCoord.xy);
    float depth = texelFetch(textures[nonuniformEXT(pc.gbuf.y)], px, 0).r;
    out_ao = 1.0;
    if (depth == 0.0) return;
    float d0 = f.cluster_params.x / depth;
    vec2 hp = (vec2(px) + 0.5) * 0.5 - 0.5; // position in half-resolution texels
    ivec2 base = ivec2(floor(hp + 0.5));
    float sum = 0.0;
    float wsum = 0.0;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            ivec2 q = clamp(base + ivec2(x, y), ivec2(0), half_size - 1);
            float dq = texelFetch(textures[nonuniformEXT(pc.gbuf.y)], min(q * 2, full - 1), 0).r;
            float lin = dq == 0.0 ? 1.0e9 : f.cluster_params.x / dq;
            float w = exp(-abs(lin - d0) / (0.03 * d0 + 0.01)) * exp(-dot(vec2(q) - hp, vec2(q) - hp) * 0.7);
            sum += texelFetch(textures[nonuniformEXT(pc.gbuf.x)], q, 0).r * w;
            wsum += w;
        }
    }
    out_ao = wsum > 1e-5 ? sum / wsum : 1.0;
}
