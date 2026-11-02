# ADR-0001: Rust with Vulkan via `ash`

- Status: Accepted
- Date: 2026-11-02

## Context
Anvil is renderer-first and requires explicit control over GPU memory, synchronization and command submission (Master Spec §1.3, §4.2).

## Problem
Choose the implementation language and the way to access the GPU.

## Considered alternatives
1. `ash`: thin, unsafe 1:1 Vulkan bindings.
2. `wgpu`: safe, portable WebGPU-style API.
3. `vulkano`: safe Vulkan wrapper.

## Decision
Use Rust stable and `ash`. Only `rhi-vulkan` may depend on `ash`.

## Advantages
- Full Vulkan feature access (barriers, timeline semaphores, bindless, ray tracing later).
- No hidden synchronization or allocation, so we can design and measure them ourselves.
- Learning value matches the project goals.

## Disadvantages
- Large `unsafe` surface; correctness is our burden.
- macOS needs MoltenVK; no web target.

## Consequences
- `unsafe` stays inside the Vulkan backend, and every `unsafe` block carries a `// SAFETY:` comment.
- Validation layers are mandatory in debug builds.

## Revisit conditions
- A second backend (D3D12/Metal) becomes a real requirement.
- `unsafe` defects in the backend dominate the bug count.
