# Module Specifications: Index and Template

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1-M4 |
| Crates | rhi-vulkan (MODULE-001..004), later rhi-vulkan / shader / render-graph / renderer |
| Traces to | MASTER-000 §13.1, §13.4; NFR-QUAL-010 |
| Last updated | 2026-11-02 |

Module specs refine one LAYER spec and must not contradict it, MASTER-000 or an accepted ADR (§13.5 applies). Crate names follow ADR-0008: `foundation`, `platform`, `rhi`, `rhi-vulkan` (Rust path `rhi_vulkan`), `renderer`, `render-graph`, `shader`. There is no `anvil-` prefix.

## 1. Index

| ID | Module | Crate | Layer spec | Milestone | Status |
|---|---|---|---|---|---|
| [MODULE-001](MODULE-001-vulkan-instance.md) | Vulkan instance | rhi-vulkan | LAYER-004 | M1 | Accepted |
| [MODULE-002](MODULE-002-vulkan-device.md) | Vulkan device | rhi-vulkan | LAYER-004 | M1 | Accepted |
| [MODULE-003](MODULE-003-vulkan-swapchain.md) | Vulkan swapchain and surface | rhi-vulkan | LAYER-004 | M1 | Accepted |
| [MODULE-004](MODULE-004-command-manager.md) | Command manager (frame slots, encoder, sync, deferred destruction) | rhi-vulkan | LAYER-004 | M1 | Accepted |
| [MODULE-005](MODULE-005-buffer-allocator.md) | Buffer allocator | rhi-vulkan (API in rhi) | LAYER-007 | M2 | Accepted |
| [MODULE-006](MODULE-006-texture-registry.md) | Texture registry | rhi-vulkan (API in rhi) | LAYER-007 | M2 | Accepted |
| [MODULE-007](MODULE-007-pipeline-cache.md) | Pipeline cache and global bindings | rhi-vulkan (+ shader cache keys) | LAYER-008 / LAYER-007 | M2 global bindings + layout + depth; M5 cache/dedupe | Accepted (M2 scope) |
| [MODULE-008](MODULE-008-shader-compiler.md) | Shader compiler | shader | LAYER-008 | M1 (SPIR-V only) / M2 | Draft (blocked on ADR-0003) |
| [MODULE-009](MODULE-009-render-graph-compiler.md) | Render graph compiler | render-graph | LAYER-006 | M4 | Accepted |
| [MODULE-010](MODULE-010-render-world.md) | Render world (M3a: `Renderer`, `RenderScene`, upload ring, forward + tonemap) | renderer | LAYER-005 | M3 (M3a implemented; M3b lights/shadows, M3c IBL pending) | Accepted (ADR-0011) |

Dependency order for M1 implementation: MODULE-001 -> 002 -> 003 and 004 (003 and 004 are developed together; the frame loop needs both). MODULE-004 also contains the minimal M1 shader-module and graphics-pipeline objects, which MODULE-007 absorbs in M2.

M2 implementation order: MODULE-002 amendment (§16: features, limits, allocator BDA) -> MODULE-007 (`GlobalBindings`, single layout, `Doomed::Pipeline` without layout, depth) -> MODULE-005 (buffers, `UploadState`, submit with upload command buffer) -> MODULE-006 (textures, samplers, bindless slots, quarantine) -> `examples/textured-quad` (LAYER-003 §17.7). MODULE-004 changes (upload command buffer in `FrameSlot`, two-cb submit, `destroy_doomed(&DeviceShared, ..)`) are specified in MODULE-005 §6.3/§8 and MODULE-006 §5.

## 2. Status values

| Status | Meaning |
|---|---|
| Draft | Concept level. Undecided points are listed in Open questions. Not enough to implement. |
| Review | Complete for the milestone. Open questions do not block implementation. |
| Accepted | Review plus concrete signatures, call order and test cases; code may start. |
| Implemented | Implemented and matching the code. |

"verify:" marks an API name or behavior not yet checked against upstream documentation. Resolve each before or during implementation and delete the marker.

## 3. Module-spec template

Copy this skeleton. All 13 sections of MASTER-000 §13.4 are mandatory (NFR-QUAL-010), in this order, followed by the two additional sections.

```markdown
# MODULE-0NN: Name

| Field | Value |
|---|---|
| Status | Draft or Review |
| Crate | crate name (ADR-0008, no anvil- prefix) |
| Layer | LAYER-00N |
| Milestone | M… |
| Traces to | FR-…, NFR-…, MASTER-000 §… |
| Last updated | YYYY-MM-DD |

## 1. Purpose
One paragraph: what the module is for and the one thing it guarantees.

## 2. Responsibilities
What it owns and does.

## 3. Non-responsibilities
What it must not do; who does it instead.

## 4. Dependencies
Allowed and forbidden crates/modules (LAYER README §2.2).

## 5. Public API
Rust signatures. Mark visibility: `pub` (crate API), `pub(crate)` (internal), trait impls.

## 6. Data structures
Structs, enums, invariants, size/representation notes.

## 7. Ownership and lifetime
Who owns each native object, create order, destroy order, `unsafe` invariants (what each SAFETY comment must justify).

## 8. Execution flow
Numbered steps with exact calls and their order.

## 9. Threading model
Which thread, which locks, which Vulkan external-synchronization rules.

## 10. Error handling
Error type, mapping table, recovery policy.

## 11. Performance requirements
Measurable statements only (NFR-PERF-012: no numeric guarantees before benchmark hardware is defined).

## 12. Testing requirements
Unit tests (no GPU) and integration tests (lavapipe or real GPU), each as a named case.

## 13. Acceptance criteria
Checkbox list; every item must be testable.

## 14. Traceability (FR/NFR IDs)
Table: requirement | how this module satisfies it.

## 15. Open questions
Numbered. Undecided design goes here, never into the sections above.
```

## 4. Conventions

- Every `unsafe` block carries `// SAFETY:` stating the invariant relied on; each module spec lists those invariants in section 7.
- No `ash::vk` type in the public API of any crate except `rhi-vulkan`'s `pub(crate)` internals and its tests (LAYER README §2.3).
- Test names in section 12 are the intended `#[test]` function names (snake_case) so reviews can check coverage.
- GPU tests are gated by `ANVIL_GPU_TESTS=1`; CI sets it and runs them on Mesa lavapipe with validation `On` and synchronization validation on.
