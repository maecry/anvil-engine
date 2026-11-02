#version 460
#include "common.glsl"

// Global bindless set 0 (ADR-0010): sampled images at binding 0, samplers at binding 1.
layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) in vec3 v_world_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) in vec4 v_tangent;
layout(location = 4) flat in uint v_material;
layout(location = 0) out vec4 out_color;

vec4 sample_tex(uint tex, uint smp, vec2 uv) {
    return texture(sampler2D(textures[nonuniformEXT(tex)], samplers[nonuniformEXT(smp)]), uv);
}

// M3a shading: Lambert + Blinn-Phong from the first directional light, hemisphere ambient.
// M3b replaces this with full PBR over the whole light array.
void main() {
    FrameData f = pc.frame.d;
    Material m = pc.materials.m[v_material];
    uint s = f.sampler_index;

    vec4 base = m.base_color * sample_tex(m.base_color_tex, s, v_uv);
    vec4 mr = sample_tex(m.mr_tex, s, v_uv);
    float roughness = clamp(m.roughness * mr.g, 0.05, 1.0);
    float metallic = clamp(m.metallic * mr.b, 0.0, 1.0);
    float ao = 1.0 + m.occlusion_strength * (sample_tex(m.occlusion_tex, s, v_uv).r - 1.0);

    vec3 n = normalize(v_normal);
    vec3 t = normalize(v_tangent.xyz - n * dot(n, v_tangent.xyz));
    vec3 b = cross(n, t) * v_tangent.w;
    vec3 tn = sample_tex(m.normal_tex, s, v_uv).xyz * 2.0 - 1.0;
    tn.xy *= m.normal_scale;
    n = normalize(mat3(t, b, n) * tn);

    vec3 view_dir = normalize(f.camera_pos.xyz - v_world_pos);
    vec3 diffuse_color = base.rgb * (1.0 - metallic);
    vec3 f0 = mix(vec3(0.04), base.rgb, metallic);

    vec3 sky = vec3(0.35, 0.45, 0.6);
    vec3 ground = vec3(0.12, 0.1, 0.09);
    vec3 color = mix(ground, sky, n.y * 0.5 + 0.5) * 0.4 * diffuse_color * ao;

    for (uint i = 0; i < f.light_count; ++i) {
        Light l = pc.lights.l[i];
        if (l.position_type.w != 0.0) continue;
        vec3 L = -normalize(l.direction_range.xyz);
        float ndl = max(dot(n, L), 0.0);
        vec3 h = normalize(L + view_dir);
        float shininess = clamp(2.0 / (roughness * roughness * roughness * roughness) - 2.0, 1.0, 4096.0);
        float spec = pow(max(dot(n, h), 0.0), shininess) * (shininess + 8.0) / (8.0 * 3.14159265);
        color += (diffuse_color / 3.14159265 + f0 * spec) * l.color_intensity.rgb * ndl;
        break;
    }

    color += m.emissive.rgb * sample_tex(m.emissive_tex, s, v_uv).rgb;
    out_color = vec4(color, base.a);
}
