// Shared GPU data layout plus the graphics push constant block (types.glsl has the data).
#include "types.glsl"

// 64 of the 128 push constant bytes.
layout(push_constant, std430) uniform Push {
    Frame frame;
    Instances instances;
    Lights lights;
    Materials materials;
    Vertices vertices;
    uint view_index;            // shadow passes: 0..3 cascade, 4..7 spot; TAA: 1 = reset history
    uint ssao_index;            // bindless slot of the full-resolution AO texture, ~0u = none
    uvec4 gbuf;                 // per-pass texture slots (deferred lighting, SSAO, TAA)
} pc;
