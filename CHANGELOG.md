# Changelog

One entry per milestone (see `docs/specs/master/MASTER-004-development-roadmap.md` for exit
evidence). Versions before 1.0.0-rc.1 were internal (`0.1.0`).

## 1.0.0-rc.1 - M7 Production hardening

- Golden-image regression tests with tolerances; resource leak checks after every GPU teardown;
  stress tests (churn, 50 000 instances, streaming, hot reload, 3 000-frame session).
- Device loss: `DeviceLost` propagates as an error, the runtime logs a diagnostics snapshot, the
  player and editor exit with code 3; `ANVIL_SIMULATE_DEVICE_LOST=<frame>` injects it for tests.
- Shader validation in the build; the per-frame `texture_barrier from-state mismatch` warning is fixed.
- Corrupt-asset tests: about 14 000 seeded mutations of glb, gltf, png, jpeg, exr, ktx2, material,
  scene, meta, project and cache files. Fixed: unbounded allocation from a GLB header length,
  from tiny images declaring huge dimensions, a panic in `gltf-json` on a dangling accessor, a
  damaged import cache that could leave an asset loading forever or decode into inconsistent data
  (the cache now carries a checksum).
- API stability document (`docs/specs/master/API-STABILITY.md`); `cargo doc` with warnings denied in CI.
- Pipeline cache default moved to `$XDG_CACHE_HOME/anvil/pipeline-cache.bin` (`ANVIL_PIPELINE_CACHE` overrides).
- Reproducible release archive: `scripts/package.sh` (`--verify`), pinned toolchain 1.99.1, CI `package` job on `v*` tags.
- User guide, limitations, release checklist.
- Version 1.0.0-rc.1. No license has been chosen yet.

## M6 - Engine runtime and tooling

- `assets`: asset database with `.meta` ids, glTF/PNG/JPEG/EXR/KTX2 importers, imported-artifact cache, async loading, hot reload.
- `scene`: ECS-backed world, hierarchy, components (transform, mesh renderer, light, camera), RON scenes.
- `runtime`: composition root, `anvil-player`, sample project.
- `debug`: egui on the RHI, diagnostics panels, overlay.
- `editor`: viewport with picking and gizmo, hierarchy, inspector, asset browser, material editor,
  debug panels, project lifecycle, play/stop, scripted `EditorCommand`s. Exit test: a project is
  created, saved, reopened and run.

## M5 - Performance engineering

- Benchmark baseline (BENCH-001), per-pass GPU timestamps, rayon worker pool (`ANVIL_THREADS`).
- Persisted pipeline cache with header validation.
- Compute and indirect drawing in the RHI; GPU-driven rendering with two-phase Hi-Z occlusion culling, LOD (meshopt), mesh arena.
- Point-light cube shadows, CPU-side texture mip streaming with budgets, debug geometry.

## M4 - Render graph and advanced rendering

- `render-graph`: declared reads and writes, pass culling, barrier planning, transient pool, DOT/JSON dump.
- Depth prepass, clustered lights, forward and deferred paths, alpha mask and blend, cascaded and spot shadows.
- SSAO, TAA, bloom as graph passes; debug views.

## M3 - Core renderer

- `renderer`: forward HDR with tonemapping, Cook-Torrance PBR, directional, point and spot lights,
  shadow mapping, baked sky IBL, skybox, `pbr-scene` example.

## M2 - RHI and resources

- Buffers, textures, samplers, bindless descriptors, queued uploads, deferred destruction,
  generational handles; `textured-quad` example.

## M1 - Vulkan foundation

- `platform` (winit window and loop), `rhi` traits, `rhi-vulkan` (ash): instance, device, swapchain,
  frames in flight, synchronization, validation layer integration; `triangle` example.
- Structured logging with JSON and file output.

## M0 - Foundation

- Specifications, ADRs, Cargo workspace, CI configuration, development standards, `foundation` crate.
