# LAYER-008: Shader System

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M1 (precompiled SPIR-V); M2-M5 (compiler, reflection, cache, hot reload) |
| Crates | shader (introduced when needed) |
| Traces to | MASTER-000 §6.2, §8.7, §12, §17; ADR-0003 (Accepted: shaderc, system lib) |
| Last updated | 2026-11-02 |

## 1. Purpose

Provide shader modules to the RHI: source management, compilation to SPIR-V, reflection, pipeline compatibility checks, caching and hot reload.

## 2. Scope

| In | Out |
|---|---|
| M1: the triangle's `build.rs` compiles GLSL with `shaderc` into `OUT_DIR`, and the result is embedded with `include_bytes!`; no crate needed | Choosing another language (ADR-0003 revisit) |
| Later: GLSL -> SPIR-V compile, include handling, defines/permutations | Pipeline object creation (RHI) |
| Reflection: bindings, push constants, vertex inputs | Material system (material) |
| Shader cache (hash of source + defines + compiler version) | Vulkan pipeline cache blob persistence details (MODULE-007) |
| Hot reload (dev builds), compile error reporting | |

## 3. Crates

No crate in M1: the triangle example compiles its GLSL in `build.rs` (`shaderc` as a build-dependency), embeds the SPIR-V and uses `rhi::ShaderModuleDesc`. No `.spv` files are committed. The `shader` crate (Layer 3) is introduced in M2, once a second consumer exists. It depends on foundation and rhi, plus the compiler crate chosen in ADR-0003 (only here).

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: producing valid SPIR-V words and reflection data; clear diagnostics with file/line; deterministic cache keys.
- Not responsible: GPU object lifetime (RHI handles), render pass logic, asset import of shader files (LAYER-009 supplies bytes if shaders become assets).

## 6. Dependencies

Allowed: foundation, rhi, compiler crate. Forbidden: rhi-vulkan, `ash`, layer 4-5 crates.

## 7. Key concepts and data flow

```
 source (.glsl) + defines -> [cache lookup] -> compile (shaderc/naga) -> SPIR-V + reflection
      -> ShaderModuleDesc{spirv} -> Device::create_shader_module -> ShaderModuleHandle
 file watcher (dev) -> invalidate -> recompile -> notify renderer -> pipeline rebuild (deferred destroy old)
```

M1 build step: shader compilation happens inside `cargo build`, so the CI step (MASTER-000 §17) is covered once CI installs `libshaderc-dev`.

## 8. Public interface sketch (concept)

```rust
pub struct ShaderSource { pub path: PathBuf, pub stage: ShaderStage, pub defines: Vec<(String,String)> }
pub struct CompiledShader { pub spirv: Vec<u32>, pub reflection: Reflection }
pub trait ShaderCompiler { fn compile(&self, src: &ShaderSource) -> Result<CompiledShader, ShaderError>; }
```

A trait here is acceptable (compile time, not per-frame hot path).

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: compiled blobs owned by the cache; module handles owned by the RHI.
- Threading: compilation is CPU-heavy and runs on worker threads; results handed to the render thread by channel.
- Errors: `ShaderError { Io, Compile{file,line,msg}, Reflect, Incompatible }`; compile errors never crash a running app (keep old module).
- Diagnostics: compile time per shader (`tracing` span), cache hit rate.

## 13. Testing strategy

Golden reflection tests on small shaders; cache key stability; invalid shader error messages; `spirv-val` in CI for committed binaries (verify tool availability).

## 14. Acceptance criteria (Draft)

- M1: triangle shaders load from embedded SPIR-V; `spirv-val` passes.
- M2+: a shader edit is picked up without restart; compile errors reported with file and line.

## 15. Open questions

1. ~~ADR-0003 outcome~~: resolved, shaderc with the system library (2026-11-02).
2. Shader language long-term (GLSL vs HLSL/Slang).
3. Permutation strategy (defines vs specialization constants).
4. Where reflection feeds pipeline-layout derivation vs explicit layouts in descriptors.
5. Are shaders assets (LAYER-009) or loose files?

## 16. Related modules

MODULE-008-shader-compiler, MODULE-007-pipeline-cache.
