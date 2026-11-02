# RESEARCH-006: Vulkan (ash) vs wgpu

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §1.3, §4.2, §6.2; ADR-0001, ADR-0004, ADR-0005; RESEARCH-004 |
| Last updated | 2026-11-02 |

## Summary
`wgpu` is a safe, portable GPU API following WebGPU semantics, implemented over Vulkan, Metal, D3D12, OpenGL and browser WebGPU. `ash` is a thin unsafe binding to Vulkan. Anvil's goals (explicit sync, allocation and learning, MASTER-000 §1.3) favor `ash`; portability and safety favor `wgpu`. This document records the trade-offs behind ADR-0001. Versions and feature sets evolve; verify against current releases.

## Concepts
- **wgpu** hides barriers (usage tracking), memory (internal allocation), descriptor management (bind groups) and pipeline/layout compatibility. Validation is built in. A `wgpu-hal` layer exposes a lower-level unsafe interface.
- **ash** gives one-to-one function and struct access to the Vulkan registry, with loaders and extension helpers; everything else (allocation, sync, lifetimes) is the user's work.

## Analysis / Comparison

| Area | `ash` (Vulkan) | `wgpu` |
|---|---|---|
| Control over synchronization | Full: explicit barriers, timeline semaphores, queues, ownership transfer | Automatic barrier insertion; single queue exposed; no timeline control |
| Memory / allocation | Full: choose types, suballocation, aliasing, ReBAR | Internal allocator; limited hints; no aliasing control |
| Command submission | Multi-queue, secondary command buffers, parallel recording by design | Command encoders; submission through one queue |
| Bindless / descriptor indexing | Descriptor indexing, descriptor buffers (extensions) | Binding arrays supported behind native-only features; evolving (verify) |
| Ray tracing | `VK_KHR_acceleration_structure`, ray query/pipeline | Experimental / ray-query support in some versions; not on all backends (verify) |
| Mesh shaders | `VK_EXT_mesh_shader` | Experimental native feature (verify) |
| Portability | Vulkan platforms; macOS via MoltenVK; no web | Vulkan, Metal, D3D12, GL, WebGPU (browser) |
| Safety | Large `unsafe` surface; validation layers help | Safe API; internal `unsafe` maintained upstream |
| Debugging | Validation layers, RenderDoc, vendor tools, full debug utils | Validation in the library, RenderDoc works on native backends; less visibility into emitted barriers |
| Shader languages | SPIR-V (GLSL/HLSL/Slang offline) | WGSL native; SPIR-V and GLSL via naga (feature-dependent) |
| Maturity | Binding crate stable and widely used; correctness is the application's | Widely used (Firefox, Bevy); API changes between releases |
| Learning value | High: exposes the real API | Lower: abstracted |
| Build / dependency weight | Small | Larger dependency tree |

### What an app-level engine loses with wgpu
- Cannot express precise barriers, split barriers or async compute queue overlap.
- Cannot implement transient aliasing or a custom allocator.
- Feature exposure lags the native APIs.

### What an app-level engine gains with wgpu
- Safe code, portability including web and consoles-like platforms through backends, much less initial work, upstream validation.

## Implications for Anvil
- Anvil's stated goals (explicit GPU management, render graph with barrier derivation and aliasing, measurable sync) cannot be met on `wgpu` without going through `wgpu-hal`, which gives up most of its benefits.
- The cost accepted with `ash`: unsafe surface (contain in `rhi-vulkan`), no web target, macOS only via MoltenVK (ADR-0004).
- A backend-agnostic `rhi` (ADR-0005) leaves a future wgpu/other backend possible, but only if the RHI stays free of Vulkan-specific concepts; that conflicts mildly with exposing explicit barriers. Decide how much sync the RHI exposes.

## Open questions
- Should the RHI expose barriers explicitly or only via the render graph?
- Is a minimal `wgpu`-style backend worth building later to get web support?
- What is MoltenVK's Vulkan 1.3 coverage (see ADR-0004)?

## References
- ash: https://docs.rs/ash and https://github.com/ash-rs/ash
- wgpu: https://docs.rs/wgpu and https://github.com/gfx-rs/wgpu
- wgpu-hal: https://docs.rs/wgpu-hal
- WebGPU specification (W3C): https://www.w3.org/TR/webgpu/
- Vulkan specification: https://docs.vulkan.org/spec/latest/index.html
- Khronos Vulkan Validation Layers: https://github.com/KhronosGroup/Vulkan-ValidationLayers
- MoltenVK: https://github.com/KhronosGroup/MoltenVK
