# ADR-0005: RHI with static dispatch

- Status: Accepted
- Date: 2026-11-02

## Context
Master Spec §5 and §8.3 place a backend-agnostic `rhi` below the renderer, with `rhi-vulkan` as the only initial backend. Principle §22.4 forbids unnecessary backend-specific types; §22.5 forbids abstractions without a real requirement.

## Problem
Decide how higher layers (render graph, renderer) call the RHI: dynamic dispatch, static dispatch, or no abstraction.

## Considered alternatives
1. `dyn Device` trait objects: runtime-selectable backend, vtable call per API call, object-safety constraints.
2. Enum dispatch (`enum Device { Vulkan(..) }`): no generics, but every backend variant is compiled into the abstraction and every match arm is boilerplate.
3. No RHI: use Vulkan types directly everywhere.
4. A minimal `Backend` trait with associated types; upper layers are generic over it (static dispatch).

## Decision
Option 4. `rhi` defines:
- Backend-agnostic plain types: descriptors, formats, usage flags, `Handle<T>` (ADR-0006).
- A minimal `Backend` trait whose associated types (`Device`, `CommandList`, `Swapchain`, ...) are implemented by `rhi-vulkan`, and a `Device` trait limited to what the M1/M2 vertical slice needs (create/destroy resources, submit, present, wait).

Higher layers are generic (`Renderer<B: Backend>`). The binary or example selects the backend once. The trait surface grows only when a layer spec requires a method; no speculative methods (§22.5).

## Advantages
- No vtable calls on hot paths; the compiler can inline across the boundary.
- Backend types stay out of upper-layer signatures (§22.4), but can be reached through associated types where needed.
- A second backend remains possible without rewriting upper layers.

## Disadvantages
- Generics propagate through every upper-layer type; longer compile times and noisier signatures.
- Backend cannot be chosen at runtime without extra work.
- Risk of designing a trait around Vulkan only (a one-backend abstraction cannot be validated).

## Consequences
- `renderer` and `render-graph` carry a `B: Backend` parameter; tests may use a mock backend.
- Plain data types live in `rhi` and are shared; Vulkan translation happens only in `rhi-vulkan`.
- The `Backend` trait is reviewed in the `rhi` module spec before M2 grows it.
- Research suggests (RESEARCH-006) that exposing explicit barriers conflicts with portability; the trait must decide this before M2.

## Revisit conditions
- Generic propagation measurably hurts compile times or readability.
- Runtime backend selection becomes a requirement (switch to `dyn` at one boundary or an enum).
- No second backend is planned after M4 (consider Option 3, collapsing the abstraction).
