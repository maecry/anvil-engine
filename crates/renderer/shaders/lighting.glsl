// BRDF, clustered light loop, shadows and IBL, shared by the forward and deferred shaders.
#include "surface.glsl"
#include "ibl.glsl"

const uint CLUSTERS_X = 16u;
const uint CLUSTERS_Y = 9u;
const uint CLUSTERS_Z = 24u;
const float SPOT_SHADOW_SIZE = 1024.0; // mirrors SPOT_SHADOW_SIZE in the renderer
const float POINT_SHADOW_SIZE = 512.0; // mirrors POINT_SHADOW_SIZE and POINT_NEAR in shadow.rs
const float POINT_NEAR = 0.05;

// GGX normal distribution.
float d_ggx(float n_o_h, float a2) {
    float d = n_o_h * n_o_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

// Smith GGX height-correlated visibility (includes the 1/(4 NoL NoV) term).
float v_smith(float n_o_v, float n_o_l, float a2) {
    float gv = n_o_l * sqrt(n_o_v * n_o_v * (1.0 - a2) + a2);
    float gl = n_o_v * sqrt(n_o_l * n_o_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

vec3 f_schlick(float v_o_h, vec3 f0) {
    return f0 + (1.0 - f0) * pow(clamp(1.0 - v_o_h, 0.0, 1.0), 5.0);
}

// 3x3 taps of hardware (bilinear) PCF.
float pcf(uint tex, vec3 uvz) {
    vec2 texel = 1.0 / vec2(textureSize(textures[nonuniformEXT(tex)], 0));
    float sum = 0.0;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            sum += texture(sampler2DShadow(textures[nonuniformEXT(tex)],
                                           shadow_samplers[nonuniformEXT(pc.frame.d.shadow_sampler)]),
                           vec3(uvz.xy + vec2(x, y) * texel, uvz.z));
        }
    }
    return sum / 9.0;
}

bool in_unit(vec3 c) {
    vec2 uv = c.xy * 0.5 + 0.5;
    return !(any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0))) || c.z < 0.0 || c.z > 1.0);
}

// Visibility from one cascade with a normal-offset lookup; 1 outside its volume.
float cascade_visibility(FrameData f, uint c, vec3 world_pos, vec3 n, float n_o_l) {
    vec3 p = world_pos + n * f.cascade_texel[c] * (1.5 + 2.5 * (1.0 - n_o_l));
    vec3 ndc = (f.cascade_vp[c] * vec4(p, 1.0)).xyz;
    if (!in_unit(ndc)) return 1.0;
    return pcf(f.cascade_tex[c], vec3(ndc.xy * 0.5 + 0.5, ndc.z));
}

uint cascade_of(FrameData f, float view_z) {
    uint c = 0u;
    while (c < 3u && view_z > f.cascade_splits[c]) ++c;
    return c;
}

// Cascade selected by view depth, blended into the next one over the last 10% of its range.
float directional_shadow(FrameData f, vec3 world_pos, vec3 n, float n_o_l, float view_z) {
    uint c = cascade_of(f, view_z);
    float vis = cascade_visibility(f, c, world_pos, n, n_o_l);
    if (c < 3u) {
        float start = f.cascade_splits[c] * 0.9;
        float t = clamp((view_z - start) / (f.cascade_splits[c] - start), 0.0, 1.0);
        if (t > 0.0) vis = mix(vis, cascade_visibility(f, c + 1u, world_pos, n, n_o_l), t);
    }
    return vis;
}

float spot_shadow(FrameData f, uint slot, Light l, vec3 world_pos, vec3 n, float n_o_l) {
    float dist = distance(world_pos, l.position_type.xyz);
    float texel = 2.0 * l.spot.w * 1.1 * dist / SPOT_SHADOW_SIZE; // world size of a texel at this distance
    vec3 p = world_pos + n * texel * (1.5 + 2.5 * (1.0 - n_o_l));
    vec4 c4 = f.spot_vp[slot] * vec4(p, 1.0);
    if (c4.w <= 0.0) return 1.0;
    vec3 ndc = c4.xyz / c4.w;
    if (!in_unit(ndc)) return 1.0;
    return pcf(f.spot_tex[slot], vec3(ndc.xy * 0.5 + 0.5, ndc.z));
}

// Depth the shadow face stores for a light-to-point vector `d`: conventional 0..1 depth of the
// major-axis distance between POINT_NEAR and `far` (see point_face_view_proj in shadow.rs).
float point_depth(vec3 d, float far) {
    vec3 a = abs(d);
    float z = max(a.x, max(a.y, a.z));
    return far * (z - POINT_NEAR) / (z * (far - POINT_NEAR));
}

// Cube shadow: normal-offset lookup, then four hardware-compared taps (each bilinear, so about a
// 3x3 kernel) spread +-half a texel across the plane perpendicular to the lookup direction. The
// reference depth is recomputed per tap, so taps that cross onto a neighbouring face compare
// against that face's own depth and the seams stay invisible (cube sampling is seamless).
float point_shadow(FrameData f, uint slot, Light l, vec3 world_pos, vec3 n, float n_o_l) {
    float far = l.direction_range.w;
    vec3 light = l.position_type.xyz;
    float dist = distance(world_pos, light);
    if (dist >= far) return 1.0;
    float texel = 2.0 * dist / POINT_SHADOW_SIZE;
    vec3 d = world_pos + n * texel * (1.5 + 2.5 * (1.0 - n_o_l)) - light;
    vec3 up = abs(d.y) < 0.9 * length(d) ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
    vec3 t1 = normalize(cross(d, up));
    vec3 t2 = normalize(cross(d, t1));
    float step_len = 0.5 * 2.0 * length(d) / POINT_SHADOW_SIZE;
    uint tex = f.point_tex[slot];
    float sum = 0.0;
    for (int i = 0; i < 4; ++i) {
        vec2 o = vec2((i & 1) == 0 ? -1.0 : 1.0, (i & 2) == 0 ? -1.0 : 1.0);
        vec3 q = d + (t1 * o.x + t2 * o.y) * step_len;
        sum += texture(samplerCubeShadow(cubes[nonuniformEXT(tex)],
                                         shadow_samplers[nonuniformEXT(f.shadow_sampler)]),
                       vec4(q, point_depth(q, far)));
    }
    return sum * 0.25;
}

float ssao_factor(vec2 frag_xy) {
    if (pc.ssao_index == 0xFFFFFFFFu) return 1.0;
    return texelFetch(textures[nonuniformEXT(pc.ssao_index)], ivec2(frag_xy), 0).r;
}

uint cluster_index(FrameData f, vec2 frag_xy, float view_z) {
    uvec2 tile = min(uvec2(frag_xy / f.cluster_params.zw * vec2(CLUSTERS_X, CLUSTERS_Y)),
                     uvec2(CLUSTERS_X - 1u, CLUSTERS_Y - 1u));
    float s = log(max(view_z, f.cluster_params.x) / f.cluster_params.x) * f.cluster_params.y;
    uint slice = min(uint(max(s, 0.0)), CLUSTERS_Z - 1u);
    return (slice * CLUSTERS_Y + tile.y) * CLUSTERS_X + tile.x;
}

float view_depth(FrameData f, vec3 world_pos) {
    return -(f.view * vec4(world_pos, 1.0)).z;
}

// One light's contribution (BRDF times irradiance); `diff` and `spec` are added separately so
// blended surfaces can scale only the diffuse term by alpha.
void add_light(FrameData f, Light l, uint index, Surface s, vec3 world_pos, vec3 v, float n_o_v,
               vec3 f0, float a2, float view_z, inout vec3 diff, inout vec3 spec) {
    int type = int(l.position_type.w);
    vec3 L;
    float att = 1.0;
    if (type == 0) {
        L = -normalize(l.direction_range.xyz);
    } else {
        vec3 d = l.position_type.xyz - world_pos;
        float dist2 = max(dot(d, d), 1e-4);
        L = d * inversesqrt(dist2);
        att = 1.0 / dist2; // inverse square (candela)
        float range = l.direction_range.w;
        if (range > 0.0) { // KHR_lights_punctual smooth range window
            float q = dist2 / (range * range);
            float w = clamp(1.0 - q * q, 0.0, 1.0);
            att *= w * w;
        }
        if (type == 2) {
            float cd = dot(-L, normalize(l.direction_range.xyz));
            att *= smoothstep(l.spot.y, l.spot.x, cd);
        }
    }
    float n_o_l = dot(s.n, L);
    if (n_o_l <= 0.0 || att <= 0.0) return;
    if (l.spot.z >= 0.0) {
        if (type == 0) att *= directional_shadow(f, world_pos, s.n, n_o_l, view_z);
        else if (type == 2) att *= spot_shadow(f, uint(l.spot.z), l, world_pos, s.n, n_o_l);
        else att *= point_shadow(f, uint(l.spot.z), l, world_pos, s.n, n_o_l);
    }
    vec3 h = normalize(L + v);
    float n_o_h = clamp(dot(s.n, h), 0.0, 1.0);
    float v_o_h = clamp(dot(v, h), 0.0, 1.0);
    vec3 fr = f_schlick(v_o_h, f0);
    vec3 radiance = l.color_intensity.rgb * att * n_o_l;
    spec += d_ggx(n_o_h, a2) * v_smith(n_o_v, n_o_l, a2) * fr * radiance;
    diff += (1.0 - fr) * (1.0 - s.metallic) * s.base / PI * radiance;
}

// Lights `s` at `world_pos` seen from the camera: IBL, directional lights, then the lights of the
// fragment's cluster. `diff` is the diffuse part, `spec` specular plus emission.
void shade(Surface s, vec3 world_pos, vec2 frag_xy, out vec3 diff, out vec3 spec) {
    FrameData f = pc.frame.d;
    vec3 v = normalize(f.camera_pos.xyz - world_pos);
    float n_o_v = max(dot(s.n, v), 1e-4);
    vec3 f0 = mix(vec3(0.04), s.base, s.metallic);
    float a = s.roughness * s.roughness;
    float a2 = a * a;
    float view_z = view_depth(f, world_pos);

    // Image-based lighting: irradiance for diffuse, two nearest prefiltered roughness levels
    // and the split-sum BRDF LUT for specular.
    vec3 r = reflect(-v, s.n);
    vec3 irradiance = textureLod(sampler2D(textures[nonuniformEXT(f.irradiance_index)],
                                           samplers[nonuniformEXT(f.ibl_sampler)]), dir_to_uv(s.n), 0.0).rgb;
    float level = s.roughness * 5.0;
    uint l0 = uint(floor(level));
    uint l1 = min(l0 + 1u, 5u);
    vec2 r_uv = dir_to_uv(r);
    vec3 pre = mix(textureLod(sampler2D(textures[nonuniformEXT(f.prefilter[l0])],
                                        samplers[nonuniformEXT(f.ibl_sampler)]), r_uv, 0.0).rgb,
                   textureLod(sampler2D(textures[nonuniformEXT(f.prefilter[l1])],
                                        samplers[nonuniformEXT(f.ibl_sampler)]), r_uv, 0.0).rgb,
                   level - float(l0));
    vec2 brdf = texture(sampler2D(textures[nonuniformEXT(f.brdf_lut_index)],
                                  samplers[nonuniformEXT(f.ibl_sampler)]),
                        vec2(clamp(n_o_v, 0.5 / 256.0, 1.0 - 0.5 / 256.0), s.roughness)).rg; // U wraps: keep off the edge
    // Screen-space AO multiplies only the ambient (IBL) term.
    float ambient_ao = s.ao * ssao_factor(frag_xy);
    diff = irradiance * s.base * (1.0 - s.metallic) * ambient_ao * f.env_intensity;
    spec = pre * (f0 * brdf.x + brdf.y) * ambient_ao * f.env_intensity;

    for (uint i = 0u; i < f.dir_count; ++i) {
        add_light(f, pc.lights.l[i], i, s, world_pos, v, n_o_v, f0, a2, view_z, diff, spec);
    }
    uint c = cluster_index(f, frag_xy, view_z);
    uvec2 range = ClusterRanges(f.cluster_ranges).r[c];
    LightIndices list = LightIndices(f.cluster_indices);
    for (uint k = 0u; k < range.y; ++k) {
        uint i = list.i[range.x + k];
        add_light(f, pc.lights.l[i], i, s, world_pos, v, n_o_v, f0, a2, view_z, diff, spec);
    }
    spec += s.emissive;
}

vec3 heat(float t) {
    t = clamp(t, 0.0, 1.0);
    return clamp(vec3(4.0 * t - 2.0, 2.0 - abs(4.0 * t - 2.0), 2.0 - 4.0 * t), 0.0, 1.0);
}

// Debug views: 1 = lights per cluster as a heatmap (0 -> black, 32+ -> red), 2 = cascade tint.
vec3 debug_view(FrameData f, vec3 color, vec3 world_pos, vec2 frag_xy) {
    float z = view_depth(f, world_pos);
    if (f.debug_mode == 1u) {
        uint count = ClusterRanges(f.cluster_ranges).r[cluster_index(f, frag_xy, z)].y;
        return count == 0u ? vec3(0.0, 0.0, 0.05) : heat(float(count) / 32.0) * 0.8 + color * 0.2;
    }
    if (f.debug_mode == 2u) {
        const vec3 tint[4] = vec3[4](vec3(1.0, 0.25, 0.25), vec3(0.25, 1.0, 0.25),
                                     vec3(0.3, 0.4, 1.0), vec3(1.0, 1.0, 0.25));
        return color * tint[cascade_of(f, z)];
    }
    if (f.debug_mode == 3u) return vec3(clamp(1.0 - (1.0 - ssao_factor(frag_xy)) * 3.0, 0.0, 1.0)) * 2.0; // occlusion exaggerated 3x; x2 = white after tonemap
    return color;
}
