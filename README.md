# Anvil Engine

*Forged for performance. Built for creation.*

A modular, renderer-first 3D game engine in Rust on Vulkan. Status: **M0 — Foundation**.

- Master specification (source of truth): [docs/specs/master/MASTER-000-master-specification.md](docs/specs/master/MASTER-000-master-specification.md)
- Specs: [master](docs/specs/master/) · [layers](docs/specs/layers/) (includes dependency rules) · [modules](docs/specs/modules/)
- Research: [docs/research/](docs/research/)
- Decisions: [docs/decisions/ADR/](docs/decisions/ADR/)
- Coding standards: [docs/development/coding-standards.md](docs/development/coding-standards.md)

## Build

```sh
cargo build --workspace
cargo test --workspace
```

Linux is the initial platform. Vulkan SDK and validation layers are required from M1 onward.
