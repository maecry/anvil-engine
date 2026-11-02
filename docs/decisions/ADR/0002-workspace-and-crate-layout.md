# ADR-0002: Cargo workspace with incrementally introduced crates

- Status: Accepted
- Date: 2026-11-02

## Context
Master Spec §7 lists 18 crates. Most have no defined interface yet.

## Problem
Decide when crates come into existence.

## Considered alternatives
1. Create all crates now as empty stubs.
2. A single crate, split later.
3. A workspace where each crate is added once it has a spec and code.

## Decision
Option 3. A virtual workspace (`members = ["crates/*", "examples/*"]`) starts with exactly the crates M1 needs:
`foundation`, `platform`, `rhi`, `rhi-vulkan` and `examples/triangle`. Every other crate in Master Spec §7 is added
only when its layer spec is accepted. Dependencies must follow `docs/specs/layers/README.md`.

## Advantages
- No empty crates that pretend boundaries are settled.
- Boundaries are drawn from real code, which keeps compile units honest.

## Disadvantages
- Early code may later move between crates.

## Consequences
- The M1 crates exist as documented stubs from M0. Code lands once their module specs are accepted.
- The `triangle` example is the M1 vertical slice and the first integration target.

## Revisit conditions
- Compile times or dependency cycles show that the boundaries are wrong.
