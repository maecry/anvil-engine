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

## Quick start (M6)

```sh
# Run the sample project (window; ANVIL_WINDOW=0 runs headless, ANVIL_EXIT_AFTER_FRAMES=n, ANVIL_SCREENSHOT=out.png)
cargo run -p player -- samples/sample-project

# Edit it: viewport (RMB + WASD to fly, click to select, drag a gizmo axis), hierarchy, inspector,
# asset browser, material editor, debug panels. File > New Project creates a project from a template.
cargo run -p editor -- samples/sample-project

# Script the editor (RON list of commands, see docs/specs/layers/LAYER-010-runtime-and-editor.md section 7b)
ANVIL_WINDOW=0 ANVIL_EDITOR_SCRIPT=script.ron cargo run -p editor
```

Linux is the initial platform. Vulkan SDK and validation layers are required from M1 onward.
