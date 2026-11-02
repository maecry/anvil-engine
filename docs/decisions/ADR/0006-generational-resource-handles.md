# ADR-0006: Generational resource handles

- Status: Accepted (mandated by Master Spec §10.2)
- Date: 2026-11-02

## Context
Master Spec §10 requires typed handles with generation tracking, centralized ownership, stale-handle detection and deferred destruction. The GPU may still use a resource when the CPU drops it (§10.3, RESEARCH-001, RESEARCH-002).

## Problem
Define the handle representation, ownership model and destruction timing for GPU resources.

## Considered alternatives
1. Raw backend objects (`vk::Buffer`) or `Arc`-wrapped RAII owners.
2. Plain index handles without generation.
3. Typed generational handles with central pools.

## Decision
Option 3.
- `Handle<T> { index: u32, generation: u32 }` with a zero-sized `PhantomData<T>` marker; `Copy`, `Eq`, `Hash`. `T` is a resource tag (`Buffer`, `Image`, `Pipeline`, ...), not a backend type.
- Each resource kind has a pool: slots hold the resource, its generation and tracked state (layout, access). Creating reuses a free slot and keeps its generation; destroying increments the generation.
- Lookup compares generation; a mismatch yields an error (or a debug panic), never undefined behavior.
- `destroy(handle)` does not free immediately: the slot's resource is moved to a deferred queue tagged with the current frame or timeline value. Each frame the backend frees entries whose tag the GPU has completed, then returns the slot to the free list.
- Pools are owned by the device; callers hold handles only.

## Advantages
- Stale-handle bugs are detected; 8-byte, `Copy`, cache-friendly handles.
- Resource state and diagnostics live in one place.
- Backend objects never leak through handles (compatible with ADR-0005).
- Deferred destruction removes use-after-free on the GPU.

## Disadvantages
- Lookup cost per access (index + generation check).
- Manual `destroy` calls: leaks are possible without RAII; needs leak diagnostics at shutdown.
- A slot whose generation reaches `u32::MAX` is retired and never reused (no wrap-around, so stale handles can never alias). Generation 0 is never issued, so `Handle::NULL` is always invalid (LAYER-001).

## Consequences
- Pools need synchronization if resources are created from several threads (decided with the threading spec, §11).
- The deferred queue is keyed on a monotonic timeline value (ADR-0004) rather than a frame index where possible.
- Dependent objects (views, descriptors) are destroyed through the same deferred path.
- The allocator (ADR-0007) frees memory only when the deferred entry retires.

## Revisit conditions
- Pool lookup shows up in profiles.
- Generation width is insufficient for long-running tools.
