# Release checklist (NFR-CI-011)

A release archive contains the runtime binaries, a sample project, user documentation, version
information and checksums (FR-TOOL-009). Shaders are embedded in the binaries. The only channel
is a tagged release candidate on Linux x86_64 (NFR-CI-012 is deferred).

## Blocks a public release

- [ ] **A `LICENSE` file exists** in the repository root and the archive. None is chosen yet; the
      owner decides (ADR-0015 §11). Until then do not publish the archive or make the
      repository public. `scripts/package.sh` packages without one and includes `LICENSE` when present.

## Before tagging

- [ ] `main` is green in CI: fmt, clippy, tests (GPU tests on lavapipe), `cargo doc` with warnings denied.
- [ ] Local: `ANVIL_GPU_TESTS=1 ANVIL_VSYNC=0 cargo test --workspace --all-features` on a hardware driver.
- [ ] `grep -rn "shortcut:" crates examples` matches `docs/user/LIMITATIONS.md`.
- [ ] `CHANGELOG.md` has an entry for the release; `docs/user/USER-GUIDE.md` env var table matches `grep -rn "ANVIL_" crates`.
- [ ] Workspace version in `Cargo.toml` is the release version; `Cargo.lock` is committed and current (`cargo build --locked`).
- [ ] `rust-toolchain.toml` pins the intended toolchain.
- [ ] `docs/specs/master/API-STABILITY.md` still matches the public API.
- [ ] MASTER-004 status log is updated.

## Build and verify

- [ ] `scripts/package.sh --verify` prints `reproducible` (two clean builds, identical archives). Record both checksums.
- [ ] Unpack the archive on a clean directory: `sha256sum -c SHA256SUMS` passes; `VERSION` shows the tag's commit and no `-dirty`.
- [ ] `bin/anvil-player samples/sample-project` runs (a window, or `ANVIL_WINDOW=0 ANVIL_EXIT_AFTER_FRAMES=60 ANVIL_SCREENSHOT=out.png`); the screenshot is not black.
- [ ] `bin/anvil-editor samples/sample-project` opens, and `ANVIL_WINDOW=0 ANVIL_EDITOR_SCRIPT=... bin/anvil-editor` runs a script.
- [ ] The archive does not contain `.anvil/` caches or local paths (`strings bin/anvil-player | grep "$HOME"` is empty).

## Publish

- [ ] Tag `vX.Y.Z[-rc.N]` on the verified commit; the CI `package` job builds and uploads the archive.
- [ ] The CI archive checksum equals the locally verified one.
- [ ] Attach `anvil-<version>-linux-x86_64.tar.gz` and its `.sha256` to the release notes (copy the changelog entry).
