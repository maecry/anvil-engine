# MODULE-008: Shader Compiler

| Field | Value |
|---|---|
| Status | Draft |
| Crate | shader (M2; in M1 the triangle compiles its shaders in `build.rs`) |
| Layer | LAYER-008 |
| Milestone | M1 (load precompiled SPIR-V) / M2 (compile, reflection) |
| Traces to | FR-SHD-001, FR-SHD-002, FR-SHD-003, FR-SHD-004, FR-SHD-005, FR-SHD-006, FR-SHD-007, FR-SHD-008, NFR-CI-005, NFR-PERF-009; MASTER-000 §6.2, §8.7, §17; ADR-0003 |
| Last updated | 2026-11-02 |

## 1. Purpose

Turn shader sources into valid SPIR-V words plus reflection data, with clear diagnostics and a deterministic cache key. Toolchain: the `shaderc` crate linked to the system libshaderc (ADR-0003, Accepted 2026-11-02). It is used both at build time (M1 triangle `build.rs`) and at runtime (hot reload). The M1 modules consume SPIR-V as `&[u32]` and nothing else (lead decision D4).

## 2. Responsibilities

- M1: none (the triangle's `build.rs` calls `shaderc` directly). M2: this crate wraps `shaderc` for build-time and runtime use.
- M2+: `ShaderCompiler` trait: source + stage + defines -> `CompiledShader { spirv, reflection }`.
- Source management: paths, includes, defines/permutations (FR-SHD-002).
- Diagnostics with file and line (FR-SHD-007); a compile error never crashes a running app (keep the old module).
- Reflection: bindings, push constants, vertex inputs (FR-SHD-003) for pipeline compatibility checks (FR-SHD-004).
- Cache: key = hash(source, defines, compiler id and version); in-memory first, disk in M5 (FR-SHD-005).
- Hot reload in dev builds (FR-SHD-006, M6).
- CI step "shader compilation" and `spirv-val` of committed binaries (FR-SHD-008, NFR-CI-005).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Creating `VkShaderModule`, pipelines | rhi / rhi-vulkan (`ShaderModuleDesc { spirv: &[u32] }`) |
| Pipeline cache blob | MODULE-007 |
| Material parameter layout | material (LAYER-005/007) |
| Importing shaders as assets | LAYER-009 (open) |
| Choosing the shading language long-term | ADR (open) |

## 4. Dependencies

Allowed: foundation, rhi, `shaderc` (ADR-0003; only here), a SPIR-V reflection library (undecided). Forbidden: rhi-vulkan, `ash`, layer 4-5 crates. The M1 modules (MODULE-001..004) do not depend on this crate.

## 5. Public API

Concept (LAYER-008 §8):

```rust
pub struct ShaderSource { pub path: PathBuf, pub stage: ShaderStage, pub defines: Vec<(String, String)> }
pub struct CompiledShader { pub spirv: Vec<u32>, pub reflection: Reflection }
pub trait ShaderCompiler { fn compile(&self, src: &ShaderSource) -> Result<CompiledShader, ShaderError>; }
pub enum ShaderError { Io(std::io::Error), Compile { file: String, line: u32, msg: String }, Reflect(String), Incompatible(String) }

/// Toolchain-neutral (M1): bytes of a .spv file -> aligned SPIR-V words.
pub fn spirv_words(bytes: &[u8]) -> Result<Vec<u32>, ShaderError>;   // len % 4 == 0, magic 0x07230203 (either endianness), converts to native order
```

## 6. Data structures

`Reflection { descriptor_bindings, push_constant_ranges, vertex_inputs, entry_points }` (shape undecided); `ShaderCacheKey` (stable hash); `CompilerInfo { name, version }` mixed into the key.

### 6.1 Toolchain options (ADR-0003: option 1, shaderc, Accepted; table kept for the revisit conditions)

| Option | Source language | Pro | Con | Hot reload | CI impact |
|---|---|---|---|---|---|
| 1. `shaderc` crate (runtime or build.rs) | GLSL | One compiler offline and at runtime; good diagnostics | Native C++ dependency; slow clean builds unless a system shaderc is found (`SHADERC_LIB_DIR`) | Yes | Needs shaderc or Vulkan SDK on runners |
| 2. Offline `glslc` from build.rs, SPIR-V embedded | GLSL | Fast builds; no runtime compiler; simplest | No runtime compile, so no hot reload without an extra dev-only path; needs `glslc` installed | Only via a separate dev tool | Needs glslc (Vulkan SDK or package) |
| 3. `naga` (pure Rust) | GLSL (frontend maturity: verify), WGSL | No native dependency; hot reload possible | GLSL frontend coverage/feature gaps (verify); different diagnostics | Yes | None |
| 4. Slang / HLSL via DXC (LAYER-008 Q2, long term) | HLSL / Slang | Modern language, strong reflection (Slang) | Large toolchain; ADR-0003 revisit condition | Yes | Heavy |

The interface in section 5 works for all four. The M1 triangle can ship committed `.spv` files compiled with whichever tool the owner chooses (LAYER-008 §7).

## 7. Ownership and lifetime

Compiled blobs are owned by the cache; the module hands out `Vec<u32>` or `&[u32]` borrowed from the cache; the RHI copies words into `VkShaderModule` at `create_shader_module`, so the caller may free them afterwards. Module handles belong to the RHI.

## 8. Execution flow

M1: `include_bytes!("triangle.vert.spv")` is not guaranteed 4-byte aligned, so it must not be cast to `&[u32]`; load with `spirv_words` (copies into a `Vec<u32>`) or embed through an aligned wrapper (`#[repr(align(4))]` struct around `[u8; N]`, or `include_bytes!` into a `[u32]` via a build step). Then `Device::create_shader_module(&ShaderModuleDesc { spirv: &words, label })`.
M2+: `compile(src)` -> key -> cache hit? return : preprocess (includes/defines) -> compile -> reflect -> store -> return. Hot reload (dev): file watcher -> invalidate -> recompile -> on success notify renderer to rebuild pipelines (old objects destroyed deferred), on failure keep old and `error!` with file and line.

## 9. Threading model

Compilation is CPU-heavy: runs on worker threads, results returned over a channel to the render thread (LAYER-008 §9-12). The cache is `Send + Sync` behind a lock or sharded map. M1 has no threads here.

## 10. Error handling

`ShaderError` as above; `Compile` carries file, line, message from the toolchain diagnostics; `spirv_words` rejects bad length and magic. Errors are values; the renderer decides to keep the old shader.

## 11. Performance requirements

Measure compile time per shader (span `shader_compile`, NFR-PERF-009) and cache hit rate. No targets before benchmark hardware exists (NFR-PERF-012). Cache lookup must be cheap enough to run per pipeline request, not per frame.

## 12. Testing requirements

Unit: `spirv_words` (misaligned slice input, wrong length, wrong magic, byte-swapped magic); cache key stability (same input same key, any define or compiler version change alters it); reflection golden tests on small shaders (once a toolchain exists); invalid-shader diagnostics contain file and line. CI: `spirv-val` over all committed `.spv` (verify availability on runners); shader compilation step (FR-SHD-008) once ADR-0003 is Accepted.

## 13. Acceptance criteria (Draft)

- [ ] M1: triangle shaders load from committed SPIR-V through the neutral loader; `spirv-val` passes; no compiler dependency in the M1 crates.
- [ ] M2: a shader compiles with file/line diagnostics on error.
- [ ] M2: reflection golden tests pass.
- [ ] M6: editing a shader file updates the running app without restart; an invalid edit keeps the previous module.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-SHD-001 | compile GLSL to SPIR-V with shaderc (M1 via the triangle's `build.rs`, M2 via this crate) |
| FR-SHD-002, FR-SHD-007 | source management and diagnostics |
| FR-SHD-003, FR-SHD-004 | reflection and compatibility (M2*) |
| FR-SHD-005 | cache (M5*) |
| FR-SHD-006 | hot reload (M6*) |
| FR-SHD-008, NFR-CI-005 | CI shader step and `spirv-val` |
| NFR-PERF-009 | compile-time measurement |

## 15. Open questions

1. ~~ADR-0003 outcome~~: resolved, shaderc with the system library.
2. Shading language long term (GLSL vs HLSL/Slang, LAYER-008 Q2).
3. Permutations: preprocessor defines vs specialization constants (LAYER-008 Q3).
4. Reflection library and whether reflection derives pipeline layouts or layouts stay explicit (LAYER-008 Q4).
5. Are shaders assets (LAYER-009) or loose files (LAYER-008 Q5)?
6. Where `spirv_words` lives while no `shader` crate exists (default: a tiny helper in the triangle example until the crate is introduced).
