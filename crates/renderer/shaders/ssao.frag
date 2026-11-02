#version 460
// Half-resolution hemisphere SSAO from the prepass depth and octahedral view-space normals.
// pc.gbuf = (depth, view normal). Output: occlusion factor, 1 = open.
#include "bindings.glsl"

layout(location = 0) out float out_ao;

const int SAMPLES = 16;
const float RADIUS = 0.8;
const float BIAS = 0.03;

vec3 oct_decode(vec2 e) {
    vec3 n = vec3(e, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) n.xy = (1.0 - abs(n.yx)) * vec2(n.x >= 0.0 ? 1.0 : -1.0, n.y >= 0.0 ? 1.0 : -1.0);
    return normalize(n);
}

// View-space position of a uv and reverse-Z depth (infinite far: depth = near / distance).
vec3 view_pos(FrameData f, vec2 uv, float depth) {
    float d = f.cluster_params.x / depth;
    vec2 ndc = uv * 2.0 - 1.0;
    // Inverse of the (jittered) projection the depth was rendered with.
    return vec3(d * (ndc.x + f.proj[2][0]) / f.proj[0][0], d * (ndc.y + f.proj[2][1]) / f.proj[1][1], -d);
}

void main() {
    FrameData f = pc.frame.d;
    vec2 full = f.cluster_params.zw;
    vec2 half_size = ceil(full * 0.5);
    vec2 uv = gl_FragCoord.xy / half_size;
    ivec2 px = min(ivec2(uv * full), ivec2(full) - 1);
    float depth = texelFetch(textures[nonuniformEXT(pc.gbuf.x)], px, 0).r;
    out_ao = 1.0;
    if (depth == 0.0) return;
    vec3 p = view_pos(f, uv, depth);
    vec3 n = oct_decode(texelFetch(textures[nonuniformEXT(pc.gbuf.y)], px, 0).rg);
    // Per-pixel rotation of the kernel (interleaved gradient noise).
    float noise = fract(52.9829189 * fract(dot(vec2(px), vec2(0.06711056, 0.00583715))));
    float angle = noise * 6.2831853;
    vec3 rv = vec3(cos(angle), sin(angle), 0.0);
    vec3 t = normalize(rv - n * dot(rv, n));
    if (abs(dot(rv, n)) > 0.99) t = normalize(cross(n, vec3(0.0, 1.0, 0.0)));
    vec3 b = cross(n, t);
    float occ = 0.0;
    for (int i = 0; i < SAMPLES; ++i) {
        float h = (float(i) + 0.5) / float(SAMPLES);    // height above the surface
        float phi = float(i) * 2.39996323;
        float r = sqrt(1.0 - h * h);
        vec3 k = vec3(cos(phi) * r, sin(phi) * r, h);
        float scale = mix(0.15, 1.0, h * h);
        vec3 sp = p + (t * k.x + b * k.y + n * k.z) * RADIUS * scale;
        vec4 clip = f.proj * vec4(sp, 1.0);
        vec2 suv = clip.xy / clip.w * 0.5 + 0.5;
        if (any(lessThan(suv, vec2(0.0))) || any(greaterThan(suv, vec2(1.0)))) continue;
        float sd = texelFetch(textures[nonuniformEXT(pc.gbuf.x)], min(ivec2(suv * full), ivec2(full) - 1), 0).r;
        float scene_d = sd == 0.0 ? 1.0e9 : f.cluster_params.x / sd;
        float sample_d = -sp.z;
        if (scene_d < sample_d - BIAS) {
            occ += smoothstep(0.0, 1.0, RADIUS / abs(-p.z - scene_d));
        }
    }
    out_ao = pow(clamp(1.0 - occ / float(SAMPLES), 0.0, 1.0), 2.0);
}
