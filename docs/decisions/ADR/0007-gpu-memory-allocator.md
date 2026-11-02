# ADR-0007: GPU memory allocator

- Status: Accepted
- Date: 2026-11-02

## Context
Vulkan limits live allocations (`maxMemoryAllocationCount`), so suballocation is mandatory (Master Spec §10.3). RESEARCH-002 compares `gpu-allocator` and `vk-mem` (VMA bindings).

## Problem
Select the allocator for `rhi-vulkan` rather than writing one first.

## Considered alternatives
1. `gpu-allocator` (pure Rust).
2. `vk-mem` (bindings over C++ VMA).
3. Custom allocator written for Anvil.

## Decision
Use `gpu-allocator` with its Vulkan backend, wrapped behind a small internal module in `rhi-vulkan` so no allocator type is exposed outside it. Dedicated allocations are used for large render targets. Per-frame upload uses a custom linear ring on top of persistently mapped memory.

## Advantages
- Pure Rust: no C++ toolchain, simpler CI and cross-compilation (cf. ADR-0003).
- Smaller unsafe/FFI surface; allocator code readable in the same language.
- Supports other backends later if ADR-0005 materializes.

## Disadvantages
- No defragmentation (verify) and fewer pool and aliasing features than VMA.
- Smaller heuristic tuning history than VMA.
- Memory budget (`VK_EXT_memory_budget`) and richer statistics may need our own code.

## Consequences
- Transient aliasing in the render graph (M4) may need our own placement on top of dedicated blocks if the crate cannot bind to shared ranges (RESEARCH-002 open question).
- Memory diagnostics (Spec §10.2) combine allocator reports with our own counters.
- The wrapper allows swapping to `vk-mem` with local changes.

## Revisit conditions
- Aliasing or defragmentation become hard requirements the crate cannot meet.
- Fragmentation or allocator overhead appears in profiles.
- The crate becomes unmaintained or diverges from the `ash` version we use.
