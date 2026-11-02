# RESEARCH-005: Forward vs Deferred Rendering

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §4.1, §9.2; RESEARCH-003 |
| Last updated | 2026-11-02 |

## Summary
Forward shading evaluates materials and lights while rasterizing; deferred rasterizes surface attributes to a G-buffer and shades afterwards. Variants (forward+, clustered, visibility buffer) mainly attack the light-count and overdraw costs. MASTER-000 §9.2 orders the work: forward first, deferred in the intermediate stage, clustered lighting in the advanced stage. This research supports that order. No performance numbers are given; costs depend on scene, resolution and GPU and must be measured.

## Concepts
- **Forward**: per object, vertex + fragment shader loops over lights. Cost ~ visible fragments x lights affecting them (without culling: all lights).
- **Forward+ (tiled forward)**: a compute pass bins lights per 2D screen tile (often with a depth pre-pass to bound tile depth); fragment shader loops over the tile's list.
- **Clustered forward**: bins lights in a 3D frustum grid (tiles x depth slices), avoiding tile depth-range problems from depth discontinuities.
- **Deferred**: geometry pass writes albedo, normal, roughness/metal, etc. into several render targets plus depth; a lighting pass reads them per pixel. Variants: tiled/clustered deferred use the same light binning on the lighting pass.
- **Visibility buffer**: geometry pass writes only a triangle/instance ID (and depth); a later pass fetches vertex attributes, interpolates and shades. Needs bindless access to geometry and materials and is tied to GPU-driven rendering.

## Analysis / Comparison

| Criterion | Forward | Forward+ / Clustered fwd | Deferred | Clustered deferred | Visibility buffer |
|---|---|---|---|---|---|
| Light count scaling | Poor (per-fragment loop over all lights) | Good (culled per tile/cluster) | Good (lighting pass cost ~ pixels x lights per pixel) | Good | Good (with clustering) |
| Memory bandwidth | Low (one color + depth) | Low + light lists | High (G-buffer write and read; width depends on layout) | High | Low write (ID), higher read at shading (attribute fetch) |
| MSAA | Native, cheap on tiled/mobile GPUs | Native | Hard: shade per sample at edges; G-buffer size grows with samples | Hard | Hard; needs per-sample shading at edges |
| Transparency | Supported directly (sorted, blended) | Supported (needs light lists for transparents) | Not directly: needs a forward pass for transparents | Same | Same (forward pass for transparents) |
| Material variety | Any shader per object, any BRDF | Same | Constrained by G-buffer layout; special BRDFs need extra channels or a material ID | Same | Flexible (materials resolved in shading pass), but shader divergence/binning needed |
| Overdraw cost | Shading cost multiplied by overdraw unless depth pre-pass | Same; depth pre-pass common | Shading once per pixel (geometry pass cheap) | Same | Minimal shading overdraw |
| Screen-space effects (SSAO, SSR, decals) | Need normals/depth pre-pass | Same | G-buffer provides inputs | Same | Needs reconstruction/extra targets |
| TAA interplay | Needs motion vectors (extra target or pre-pass) and jitter; MSAA is an alternative | Same | Motion vectors written in G-buffer; TAA runs post-lighting | Same | Motion vectors via reconstruction or separate target |
| Implementation complexity | Lowest | Medium (compute culling, light buffers) | Medium | High | Highest (bindless, GPU-driven) |
| Hardware assumptions | None | Compute shaders | Multiple render targets | Compute shaders | Bindless / indirect drawing |

### Notes on TAA
TAA requires sub-pixel jitter, per-pixel motion vectors, history reprojection and clamping. It addresses aliasing in deferred and visibility-buffer pipelines where MSAA is costly; with forward it can complement or replace MSAA. It also increases the importance of correct motion vectors for skinned/animated meshes.

### Decision matrix for Anvil stages
| Stage (MASTER-000 §9.2) | Suggested path | Reason |
|---|---|---|
| Initial | Forward, single light loop, depth test | Smallest working vertical slice |
| Intermediate | Add PBR, shadows, IBL, post; introduce deferred as an experiment/alternative path | Tests render-graph and G-buffer handling |
| Advanced | Clustered lighting (forward or deferred), TAA, SSAO | Many lights, screen-space effects |
| Experimental | Visibility buffer, GPU-driven | Requires bindless and indirect drawing |

## Implications for Anvil
- Start forward, with a depth pre-pass optional. Keep light data in a storage buffer from the start so the shader interface survives the later move to clustering.
- Make lighting independent of geometry submission (separate shader includes) to reuse across forward and deferred.
- Add forward+ / clustered binning before deferred if light count is the first bottleneck; deferred mainly pays off when screen-space effects or very many lights demand it.
- Even in deferred, a forward pass remains for transparents; the render graph (RESEARCH-003) should model both paths.
- Reserve a motion-vector target and jittered projection in the camera model early to avoid later API breaks.
- Do not commit to visibility buffer before bindless and GPU-driven culling exist.

## Open questions
- Is MSAA required in the initial scope, given the TAA plan?
- G-buffer layout and bit-depth budget (octahedral normals, packed material parameters)?
- Cluster grid parameters and light-list representation?

## References
- Olsson, Billeter, Assarsson, "Clustered Deferred and Forward Shading", HPG 2012: https://www.cse.chalmers.se/~uffe/clustered_shading_preprint.pdf (verify)
- Harada, McKee, Yang, "Forward+: Bringing Deferred Lighting to the Next Level", Eurographics 2012 short paper (verify URL)
- Burns, Hunt, "The Visibility Buffer: A Cache-Friendly Approach to Deferred Shading", JCGT 2013: https://jcgt.org/published/0002/02/04/
- Karis, "High Quality Temporal Supersampling", SIGGRAPH 2014 (Unreal TAA): https://de45xmedrsdbp.cloudfront.net/Resources/files/TemporalAA_small-59732822.pdf (verify)
- Real-Time Rendering, 4th ed., chapter on Lighting/Shading (book)
- Filament design document (PBR and clustered forward): https://google.github.io/filament/Filament.html
