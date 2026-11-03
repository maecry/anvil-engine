// Material evaluation shared by the forward and G-buffer fragment shaders.
#include "bindings.glsl"

struct Surface {
    vec3 base;
    float alpha;
    float metallic;
    float roughness;
    float ao;
    vec3 n;         // shading normal, world space
    vec3 emissive;
};

// Normal is flipped for back faces (double-sided materials); single-sided back faces are culled.
Surface eval_surface(uint material, uint smp, vec2 uv, vec3 n_in, vec4 tangent, bool front) {
    Material m = pc.materials.m[material];
    Surface s;
    vec4 base = m.base_color * sample_tex(m.base_color_tex, smp, uv);
    vec4 mr = sample_tex(m.mr_tex, smp, uv);
    s.base = base.rgb;
    s.alpha = base.a;
    s.roughness = clamp(m.roughness * mr.g, 0.045, 1.0);
    s.metallic = clamp(m.metallic * mr.b, 0.0, 1.0);
    s.ao = 1.0 + m.occlusion_strength * (sample_tex(m.occlusion_tex, smp, uv).r - 1.0);
    vec3 n = normalize(n_in) * (front ? 1.0 : -1.0);
    vec3 t = normalize(tangent.xyz - n * dot(n, tangent.xyz));
    vec3 b = cross(n, t) * tangent.w;
    vec3 tn = sample_tex(m.normal_tex, smp, uv).xyz * 2.0 - 1.0;
    tn.xy *= m.normal_scale;
    s.n = normalize(mat3(t, b, n) * tn);
    s.emissive = m.emissive.rgb * sample_tex(m.emissive_tex, smp, uv).rgb;
    return s;
}

// Alpha only (prepass and shadow alpha test).
float surface_alpha(uint material, uint smp, vec2 uv) {
    Material m = pc.materials.m[material];
    return m.base_color.a * sample_tex(m.base_color_tex, smp, uv).a;
}

// Per-instance debug tints (debug_mode 4: LOD level, 5: drawn by occlusion phase 2, 6: resident
// mip of the material's base color texture). `flags` is forward.vert's v_flags: LOD in bits 0-2,
// phase 2 in bit 3.
vec3 instance_tint(uint mode, uint flags, uint material) {
    if (mode == 6u) {
        const vec3 mip[6] = vec3[6](vec3(0.3, 1.0, 0.3), vec3(0.7, 1.0, 0.2), vec3(1.0, 1.0, 0.2),
                                    vec3(1.0, 0.55, 0.1), vec3(1.0, 0.15, 0.15), vec3(0.9, 0.2, 1.0));
        return mip[min(pc.materials.m[material].base_mip, 5u)];
    }
    if (mode == 4u) {
        const vec3 lod[4] = vec3[4](vec3(0.3, 1.0, 0.3), vec3(1.0, 1.0, 0.2), vec3(1.0, 0.5, 0.1), vec3(1.0, 0.15, 0.15));
        return lod[min(flags & 7u, 3u)];
    }
    if (mode == 5u && (flags & 8u) != 0u) return vec3(1.0, 0.1, 1.0);
    return vec3(1.0);
}
