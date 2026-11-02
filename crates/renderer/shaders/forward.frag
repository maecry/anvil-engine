#version 460
#extension GL_EXT_samplerless_texture_functions : require
#include "common.glsl"

// Global bindless set 0 (ADR-0010): sampled images at binding 0, samplers at binding 1. The
// comparison samplers alias the same binding as `samplerShadow`.
layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];
layout(set = 0, binding = 1) uniform samplerShadow shadow_samplers[];

layout(location = 0) in vec3 v_world_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) in vec4 v_tangent;
layout(location = 4) flat in uint v_material;
layout(location = 0) out vec4 out_color;

const float PI = 3.14159265;

vec4 sample_tex(uint tex, uint smp, vec2 uv) {
    return texture(sampler2D(textures[nonuniformEXT(tex)], samplers[nonuniformEXT(smp)]), uv);
}

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

// Shadow visibility in [0, 1]: normal-offset lookup, 3x3 taps of hardware (bilinear) PCF.
float shadow_factor(vec3 world_pos, vec3 n, float n_o_l) {
    FrameData f = pc.frame.d;
    vec3 p = world_pos + n * f.shadow_texel_world * (1.5 + 2.5 * (1.0 - n_o_l));
    vec3 c = (f.light_view_proj * vec4(p, 1.0)).xyz;
    vec2 uv = c.xy * 0.5 + 0.5;
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0))) || c.z < 0.0 || c.z > 1.0) {
        return 1.0; // outside the fitted volume: lit
    }
    vec2 texel = 1.0 / vec2(textureSize(textures[nonuniformEXT(f.shadow_index)], 0));
    float sum = 0.0;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            sum += texture(sampler2DShadow(textures[nonuniformEXT(f.shadow_index)],
                                           shadow_samplers[nonuniformEXT(f.shadow_sampler)]),
                           vec3(uv + vec2(x, y) * texel, c.z));
        }
    }
    return sum / 9.0;
}

void main() {
    FrameData f = pc.frame.d;
    Material m = pc.materials.m[v_material];
    uint s = f.sampler_index;

    vec4 base = m.base_color * sample_tex(m.base_color_tex, s, v_uv);
    vec4 mr = sample_tex(m.mr_tex, s, v_uv);
    float roughness = clamp(m.roughness * mr.g, 0.045, 1.0);
    float metallic = clamp(m.metallic * mr.b, 0.0, 1.0);
    float ao = 1.0 + m.occlusion_strength * (sample_tex(m.occlusion_tex, s, v_uv).r - 1.0);

    vec3 n = normalize(v_normal);
    vec3 t = normalize(v_tangent.xyz - n * dot(n, v_tangent.xyz));
    vec3 b = cross(n, t) * v_tangent.w;
    vec3 tn = sample_tex(m.normal_tex, s, v_uv).xyz * 2.0 - 1.0;
    tn.xy *= m.normal_scale;
    n = normalize(mat3(t, b, n) * tn);

    vec3 v = normalize(f.camera_pos.xyz - v_world_pos);
    float n_o_v = max(dot(n, v), 1e-4);
    vec3 f0 = mix(vec3(0.04), base.rgb, metallic);
    float a = roughness * roughness;
    float a2 = a * a;

    // shortcut: hemisphere ambient placeholder (diffuse + tinted specular) until IBL in M3c.
    vec3 sky = vec3(0.35, 0.45, 0.6);
    vec3 ground = vec3(0.12, 0.1, 0.09);
    vec3 r = reflect(-v, n);
    vec3 color = mix(ground, sky, n.y * 0.5 + 0.5) * 0.4 * base.rgb * (1.0 - metallic) * ao;
    color += mix(ground, sky, r.y * 0.5 + 0.5) * 0.4 * f_schlick(n_o_v, f0) * (1.0 - 0.6 * roughness) * ao;

    for (uint i = 0; i < f.light_count; ++i) {
        Light l = pc.lights.l[i];
        int type = int(l.position_type.w);
        vec3 L;
        float att = 1.0;
        if (type == 0) {
            L = -normalize(l.direction_range.xyz);
        } else {
            vec3 d = l.position_type.xyz - v_world_pos;
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
        float n_o_l = dot(n, L);
        if (n_o_l <= 0.0 || att <= 0.0) continue;
        if (i == f.shadow_light) att *= shadow_factor(v_world_pos, normalize(v_normal), n_o_l);

        vec3 h = normalize(L + v);
        float n_o_h = clamp(dot(n, h), 0.0, 1.0);
        float v_o_h = clamp(dot(v, h), 0.0, 1.0);
        vec3 fr = f_schlick(v_o_h, f0);
        vec3 spec = d_ggx(n_o_h, a2) * v_smith(n_o_v, n_o_l, a2) * fr;
        vec3 diff = (1.0 - fr) * (1.0 - metallic) * base.rgb / PI;
        color += (diff + spec) * l.color_intensity.rgb * att * n_o_l;
    }

    color += m.emissive.rgb * sample_tex(m.emissive_tex, s, v_uv).rgb;
    out_color = vec4(color, base.a);
}
