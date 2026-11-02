# Coding Standards

- `cargo fmt` and `cargo clippy --workspace --all-targets` are clean; CI runs them with `-D warnings`.
- Every crate opts in to `[lints] workspace = true`.
- Public items have rustdoc (`missing_docs` warns).
- `unsafe` lives only where it is unavoidable (mainly `rhi-vulkan`). Every `unsafe` block has a `// SAFETY:` comment.
- Errors: `thiserror` in libraries and `anyhow` only in binaries and examples. No `unwrap()` on fallible GPU or IO paths outside tests.
- Logging goes through `tracing`; never use `println!` in engine crates.
- No new crate, dependency or abstraction without a spec or ADR that requires it.
- If code and spec disagree, follow Master Spec §13.5: stop, document the problem, update the spec or ADR, then continue.
