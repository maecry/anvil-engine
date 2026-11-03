# ADR-0015: M7 production hardening: golden images, leak and stress tests, device loss, shader validation, corruption tests, API stability, packaging

- Status: Accepted
- Date: 2026-11-03

## Context
M7 (MASTER-000 "M7 — Production Hardening") delivers:
- regression testing
- resource leak testing
- device-loss handling
- shader compatibility tests
- asset corruption handling
- stable public interfaces
- packaging
- user documentation

The completion criterion is **a reproducible release candidate with documented limitations**.

The requirements behind it:
- FR-VK-014: device loss
- FR-SHD-009: shader compatibility tests
- FR-SCN-007: stable public interfaces
- FR-TOOL-009: release contents
- NFR-REL-003: live counts are zero after shutdown
- NFR-REL-005: corrupt assets return errors
- NFR-CI-011: a release checklist
- NFR-CI-012: release channels, revisited here
- MASTER-000 §15.3: image comparison with tolerances
- MASTER-000 §15.5: stress tests
- MASTER-000 §17: release artifacts

State at M6 exit:
- **Driver coverage.** About 230 tests. GPU tests run on RADV locally and on lavapipe (Mesa 25.2) in CI.
- **Device loss.** `RhiError::DeviceLost` exists and is mapped from Vulkan, but nothing above the RHI reacts to it.
- **Leak and memory counters.** `DeviceDiagnostics` already exposes `live` object counts, `pending_destroys` and `memory.allocated_bytes`.
- **Shaders** are compiled by `build.rs` (shaderc) and embedded. There are no committed `.spv` files.
- **Pipeline cache.** It defaults to `target/anvil-pipeline-cache.bin`, which is meaningless in a shipped build.
- **Repository metadata.** There is no LICENSE file, and the version is `0.1.0`.
- **Two known problems:**
  - A per-frame WARN `texture_barrier from-state mismatch`: the renderer imports a persistent target as `Undefined` in the editor viewport and the headless player.
  - Two lavapipe-only tests (`push_constants_reach_shader`, `pipelines_share_one_layout`) are flaky: the readback is all zero, including alpha, even though the clear is `[0,0,0,1]`.

## Decision
1. **Regression tests: golden images (§15.3).**
   - **Scenes.** `crates/renderer/tests/golden.rs` renders fixed scenes headless at 256×256 with fixed cameras:
     - unlit
     - PBR spheres with IBL
     - shadows (directional and point)
     - alpha mask
     - transparency
     - post (bloom and tonemap)
     - a GPU-culled grid
     - the sample project through `runtime`
   - **References.** Each scene is compared against `crates/renderer/tests/golden/<name>.png`, one reference set for all drivers.
   - **Comparison.** Two thresholds must hold: per-pixel error at most 8/255 per channel on at least 99% of pixels, and RMSE at most 2/255. Both thresholds are constants in the test.
   - **On mismatch.** The actual image and an amplified diff are written to `target/golden-failures/`, and CI uploads that directory as an artifact.
   - **Blessing.** `ANVIL_BLESS=1` rewrites the references.
   - **Driver differences.** References are blessed on RADV. If lavapipe differs beyond the thresholds, we fix our code or widen the threshold for that one scene with a comment. We do not keep per-driver references.
2. **Leak tests (NFR-REL-003).**
   - **Live counts.** A shared test helper `assert_no_leaks(&device)` checks that `live` counts are zero, `pending_destroys == 0` and `allocated_bytes == 0` after renderer, runtime or editor teardown. The device's own internal objects are excluded.
   - **Coverage.** Every existing GPU teardown test calls the helper.
   - **Shutdown log.** `Runtime` logs the final diagnostics at shutdown as a structured `info` event, so a leak is visible in any run log.
3. **Stress tests (§15.5).** `crates/runtime/tests/stress.rs`, GPU-gated, with moderate counts by default. `ANVIL_STRESS=1` multiplies the counts by 10 for local soak runs.
   - **Churn.** Create and destroy 2,000 buffers, textures and materials in batches.
   - **Large scene.** 50,000 instances.
   - **Resize.** 50 swapchain resizes. This needs a window, so it is skipped headless.
   - **Streaming.** A budget smaller than the texture set, cycled.
   - **Hot reload.** Reload a material and a texture 100 times.
   - **Long session.** 3,000 headless frames. `allocated_bytes` and `live` must be identical at frame 500 and at the last frame.
   - Every stress test ends with `assert_no_leaks`.
4. **Device loss (FR-VK-014).**
   - **Propagation.** `DeviceLost` from submit, wait, acquire or present propagates as `Err` without a panic.
   - **Runtime.** `Runtime` turns it into `RuntimeError::DeviceLost`. It logs an `error` event that includes the diagnostics snapshot, skips GPU waits that would hang, and releases CPU-side state.
   - **Exit codes.** The player and the editor exit with code 3 (1 stays generic failure).
   - **Fault injection.** `ANVIL_SIMULATE_DEVICE_LOST=<frame>` makes the Vulkan backend's submit return `DeviceLost` at that frame. No real loss occurs. A test runs the player with it and asserts exit code 3 and the logged event.
   - shortcut: there is no device recreation. A lost device ends the process cleanly. Upgrade when a target needs to survive a TDR (recreate the device, then re-upload from the asset server, which is the source of truth).
5. **Shader compatibility (FR-SHD-009).**
   - **Validation.** A test runs `spirv-val --target-env vulkan1.3` on every SPIR-V blob in the renderer, debug and RHI test shaders. The test is skipped with a message if `spirv-val` is missing; CI installs `spirv-tools`.
   - **Pipeline smoke test.** A GPU test creates every renderer pipeline variant on the current device. This runs on both RADV and lavapipe.
6. **Asset corruption (NFR-REL-005).**
   - **Mutation tests.** `crates/assets/tests/corruption.rs` applies seeded, deterministic mutations to every format we read: glb, gltf, png, jpeg, exr, ktx2, material, scene, meta, project and the cache `.bin`. The mutations are:
     - truncation at many offsets
     - byte flips
     - an empty file
     - random junk
     - header fields set to huge values
   - **Assertion.** Each load returns `Err` or `LoadState::Failed`. It never panics, and it never allocates more than the file could justify. The cache reader bounds-checks header lengths against the file size before allocating.
   - About 10,000 cases, under 30 s in release-debug CI.
   - cargo-fuzz is deferred because it needs nightly. The mutation corpus covers the same entry points.
7. **Stable public interfaces (FR-SCN-007).**
   - **Stable at 1.0:**
     - the `runtime` application API: `Runtime`, `Application`, `Context`, `RunOptions`
     - the `scene` components and `World` API
     - the `assets` handles and `LoadState`
     - the file formats: project, scene, material and meta, all versioned
     - the env vars listed in the user guide
   - **Unstable (may change in minor versions):**
     - `rhi`, `rhi-vulkan`, `render-graph` and `renderer` internals
     - `debug`
     - `editor`
   - These lists are recorded in `docs/specs/master/API-STABILITY.md`.
   - **CI** adds `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`.
   - **Version.** The workspace version becomes `1.0.0-rc.1`.
   - cargo-semver-checks is adopted after 1.0.0, when a baseline exists.
8. **Packaging and reproducibility (FR-TOOL-009, §17).**
   - **Script.** `scripts/package.sh` builds `--release --locked` and writes `dist/anvil-<version>-linux-x86_64.tar.gz`. The archive contains:
     - `bin/anvil-player` and `bin/anvil-editor`
     - `samples/sample-project/`
     - `docs/`: the user guide, the limitations doc and the changelog
     - `VERSION`: the version, git commit, rustc version and build date from `SOURCE_DATE_EPOCH`
     - a `SHA256SUMS` file
   - **Shader artifacts** stay embedded in the binaries, so no separate shader directory ships.
   - **Reproducibility:**
     - `rust-toolchain.toml` pins `1.99.1`.
     - `Cargo.lock` is used with `--locked`.
     - `--remap-path-prefix` strips local paths.
     - The tarball is deterministic: `tar --sort=name --mtime=@$SOURCE_DATE_EPOCH --owner=0 --group=0`, then `gzip -n`.
     - `scripts/package.sh --verify` builds twice and compares the checksums.
   - **CI.** A `package` job on `v*` tags (and on manual dispatch) runs the script and uploads the tarball.
   - **Runtime paths.**
     - The pipeline cache moves to `$XDG_CACHE_HOME/anvil/pipeline-cache.bin`, falling back to `~/.cache/anvil/`. `ANVIL_PIPELINE_CACHE=<path>` overrides it.
     - The imported-asset cache stays per project.
   - **Channels (NFR-CI-012).** Stay deferred. The only channel is a tagged release candidate. Linux x86_64 only.
9. **User documentation.** Under `docs/user/`:
   - `USER-GUIDE.md`: build, run the player and the editor, project layout and formats, editor scripts, the environment variable reference, troubleshooting, and validation and debug views.
   - `LIMITATIONS.md`: every `shortcut:` in the code, plus platform limits (Linux and Vulkan 1.3 only, no device recreation, no undo, no compressed textures, and so on).

   Also:
   - `CHANGELOG.md` with one entry per milestone.
   - A README status update.
   - `docs/development/release-checklist.md` (NFR-CI-011).
10. **Known issues fixed in M7.**
    - **Barrier warning.** Persistent targets keep their last layout across frames (the graph imports them with their real state), so the `texture_barrier from-state mismatch` WARN disappears. A test asserts that no WARN is logged over 10 frames of headless player.
    - **Flaky lavapipe readback.**
      - Root-cause it in code: the submission or wait path of `render_offscreen` and `read_buffer`, and whether mapped memory is invalidated or flushed.
      - If no cause is found, mark the two tests `#[ignore]` on lavapipe only, with a `shortcut:` naming the symptom. They keep running on other drivers.
      - In either case, list them in `LIMITATIONS.md`.
11. **License.** None is chosen here; it is the owner's decision. Packaging works without one, and the release checklist blocks a public release until a LICENSE exists.

## Rationale
- **Golden images with tolerances** are the cheapest regression net for a renderer, and §15.3 asks for them. One reference set with thresholds avoids blessing references on a driver we cannot run locally.
- **Fault injection** tests the device-loss path deterministically. A real TDR cannot be triggered in CI.
- **Mutation tests** run on stable Rust, are deterministic, and cover every reader. Bounding allocations by file size closes the "huge length" class of bugs.
- **Toolchain pinning, `--locked`, path remapping and deterministic tar** are what "reproducible" needs in practice, and `--verify` proves it.

## Alternatives
- **Per-driver golden references:** we cannot bless lavapipe locally, and the references would double.
- **Perceptual diff (FLIP, SSIM):** more code; plain thresholds are enough at 256×256.
- **Device recreation now:** large (every GPU resource must be rebuilt) for a case that is rare on desktop Linux. Deferred and documented.
- **cargo-fuzz:** needs nightly. It can be added later, reusing the same entry points.
- **Distro packages, Flatpak or AppImage:** deferred along with channels.

## Consequences
- New CI pieces: the `spirv-tools` apt package, the golden-failure artifact upload, `cargo doc -D warnings`, and the tag-triggered `package` job.
- The toolchain is pinned, so bumping it becomes an explicit change.
- The version moves to `1.0.0-rc.1`, and the stability document binds the stable surfaces.
- `LIMITATIONS.md` becomes the single list of known gaps and must be updated whenever a `shortcut:` is added.

## Phasing
- **M7a (engine hardening):** golden images, leak helper and stress tests, device loss with fault injection, shader validation, the barrier-warning fix and the flaky-test investigation.
- **M7b (release):** corruption tests and cache bounds checks, the API-stability doc and version, the pipeline-cache path, packaging plus `--verify` plus the CI job, the toolchain pin, user docs, the changelog, the release checklist and the README.
- M7a and M7b run in parallel and are merged after both are done. Then come the final verification, the `v1.0.0-rc.1` package build, and the "M7 is complete" status in MASTER-004.
