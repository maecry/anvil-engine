#version 460
#extension GL_EXT_nonuniform_qualifier : require

layout(push_constant, std430) uniform Push {
    uvec2 verts;
    vec2 screen;
    uint tex;
    uint smp;
    uint srgb_target;
} pc;

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) in vec4 v_color;
layout(location = 1) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

vec3 linear_from_gamma(vec3 c) {
    return mix(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, lessThan(c, vec3(0.04045)));
}

void main() {
    // egui's look is defined by multiplying gamma-space premultiplied colors; an sRGB target
    // re-encodes on store, so hand it linear values.
    vec4 c = v_color * texture(sampler2D(textures[nonuniformEXT(pc.tex)],
                                         samplers[nonuniformEXT(pc.smp)]), v_uv);
    if (pc.srgb_target == 1u) c.rgb = linear_from_gamma(c.rgb);
    out_color = c;
}
