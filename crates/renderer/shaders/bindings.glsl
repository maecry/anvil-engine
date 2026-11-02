// Global bindless set 0 (ADR-0010): sampled images at binding 0, samplers at binding 1. The
// comparison samplers alias the same binding as `samplerShadow`.
#extension GL_EXT_samplerless_texture_functions : require
#include "common.glsl"

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];
layout(set = 0, binding = 1) uniform samplerShadow shadow_samplers[];

vec4 sample_tex(uint tex, uint smp, vec2 uv) {
    return texture(sampler2D(textures[nonuniformEXT(tex)], samplers[nonuniformEXT(smp)]), uv);
}
