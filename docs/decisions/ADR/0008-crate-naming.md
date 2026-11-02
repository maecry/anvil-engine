# ADR-0008: Crate names without the `anvil-` prefix

- Status: Accepted
- Date: 2026-11-02

## Context
MASTER-000 §7 names every crate `anvil-*`. Inside the workspace the prefix is redundant noise (`anvil_rhi::Device`).

## Problem
Pick crate names that are short, unambiguous and valid.

## Considered alternatives
1. Keep `anvil-*`.
2. Drop the prefix everywhere, renaming `anvil-core` to `core`.
3. Drop the prefix and rename `anvil-core` to `foundation`.

## Decision
Option 3. The `anvil-X` crates become `X`, except `anvil-core`, which becomes `foundation` (matching LAYER-001-foundation).
`core` is rejected because it collides with Rust's built-in `core` crate, which std macros reference via `::core`.

## Advantages
- Shorter paths (`rhi::Device`, `rhi_vulkan::…`).

## Disadvantages
- Generic names such as `rhi`, `platform` and `renderer` are already taken on crates.io, so they cannot be published as-is.

## Consequences
- MASTER-000 §7/§8 crate names map through this ADR. All other docs use the new names.
- Future crates follow the same rule. Any name that collides with a std crate (`core`, `alloc`, `std`, `proc_macro`, `test`) is forbidden.

## Revisit conditions
- Publishing to crates.io. Then use `package.name = "anvil-X"` with `[lib] name = "X"`, or rename crates.
