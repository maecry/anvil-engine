# API stability (FR-SCN-007, ADR-0015 §7)

Status: binding from 1.0.0. Applies to the workspace version (`Cargo.toml`, currently 1.0.0-rc.1);
all crates are versioned together.

## Stable

Breaking changes to these need a major version.

| Surface | Where |
|---|---|
| Application API: `Runtime`, `Application`, `Context`, `RunOptions` (and `RunReport`, `RuntimeError`, `Project`) | `runtime` |
| Scene components and the `World` API: `Transform`, `MeshRenderer`, `Light`, `Camera`, `World`, `SceneFile`, `EntityData` | `scene` |
| Asset handles and loading: `AssetId`, `AssetServer`, `LoadState`, `AssetEvent`, `Asset` and its CPU asset types | `assets` |
| File formats: `project.anvil`, scene, material and `.meta`. Each carries a `version`; a build reads every version up to its own and rejects newer ones with an error. Changes are additive (new optional fields) or bump the version with a reader for the old one. | `runtime`, `scene`, `assets` |
| Environment variables listed in `docs/user/USER-GUIDE.md` | all |
| Exit codes of `anvil-player` and `anvil-editor` (0, 1, 3) | `player`, `editor` |

## Unstable

May change in any minor version.

| Surface | Why |
|---|---|
| `rhi`, `rhi-vulkan`, `render-graph`, `renderer` public items | Internals of the renderer; the RHI follows Vulkan and the renderer's feature set. |
| `debug` | Diagnostics UI. |
| `editor` (library API, `EditorCommand` set, script details) | Tooling; the command set grows. |
| `platform`, `foundation` items not re-exported through a stable crate | Support crates. |
| The imported-asset cache (`.anvil/`) and the pipeline cache file | Regenerated when invalid. |
| Log message text and field names | Observability, not an interface (the `ANVIL_LOG*` variables are stable). |

## Rules

- Stable items are documented (`missing_docs` is a lint) and covered by tests.
- `cargo doc --workspace --no-deps` runs in CI with `RUSTDOCFLAGS=-D warnings`.
- `cargo-semver-checks` is adopted after 1.0.0, when a published baseline exists.
- Deprecation: a stable item is marked `#[deprecated]` for at least one minor version before removal in a major version.
- Pre-release (`1.0.0-rc.N`): stable surfaces are frozen except for fixes found in the release candidate.
