# ADR-0003: Shader toolchain

- Status: Accepted
- Date: 2026-11-02

## Context
Master Spec §6.2 proposes GLSL compiled with `shaderc` to SPIR-V.

## Problem
`shaderc` builds a large C++ library through CMake unless a system copy is found, which slows down clean builds and CI.

## Considered alternatives
1. `shaderc` crate at runtime or in build.rs (enables hot reload later).
2. Offline `glslc` invoked from build.rs, with SPIR-V embedded.
3. `naga` (pure Rust) for GLSL → SPIR-V.

## Decision
Option 1. Use the `shaderc` crate linked against the system libshaderc (found via `SHADERC_LIB_DIR` or pkg-config, never built from source in CI).
Use the same compiler for build-time compilation in M1 and for runtime compilation and hot reload later (Spec §8.7).

## Advantages
- Same compiler offline and at runtime, with good diagnostics.

## Disadvantages
- Native dependency; CI must install the Vulkan SDK or shaderc.

## Consequences
- CI installs the system shaderc package and gains a "shader compilation" step (Spec §17, item 5).
- Developer machines need system shaderc. On CachyOS/Arch that is `pacman -S shaderc`; on Ubuntu, `libshaderc-dev`.

## Revisit conditions
- Moving to HLSL/Slang, or build times become a problem.
