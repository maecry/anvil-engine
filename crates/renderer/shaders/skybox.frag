#version 460
#include "common.glsl"
#include "ibl.glsl"

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[];

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 out_color;

void main() {
    FrameData f = pc.frame.d;
    // The projection has the Y flip folded in, so ndc.y = -1 is the top of the screen.
    vec3 d = normalize(f.cam_forward.xyz + f.cam_right.xyz * (v_ndc.x * f.cam_right.w)
                       - f.cam_up.xyz * (v_ndc.y * f.cam_up.w));
    vec3 c = textureLod(sampler2D(textures[nonuniformEXT(f.sky_index)],
                                  samplers[nonuniformEXT(f.ibl_sampler)]), dir_to_uv(d), 0.0).rgb;
    out_color = vec4(c * f.env_intensity, 1.0);
}
