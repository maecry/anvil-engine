# RESEARCH-002: GPU Memory Allocation

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §10.3, §22.6; ADR-0006, ADR-0007; RESEARCH-001 |
| Last updated | 2026-11-02 |

## Summary
Vulkan exposes raw memory heaps and types; the application must allocate, suballocate, bind and free. Drivers limit the number of live `vkAllocateMemory` calls, so engines suballocate from large blocks. Two Rust options exist: `gpu-allocator` (pure Rust, Embark/Traverse Research lineage) and `vk-mem` (bindings to AMD's C++ Vulkan Memory Allocator). This document compares them; the decision is in ADR-0007.

## Concepts

### Memory heaps and types
- `vkGetPhysicalDeviceMemoryProperties` returns heaps (physical pools) and memory types (a heap + property flags).
- Key property flags: `DEVICE_LOCAL`, `HOST_VISIBLE`, `HOST_COHERENT`, `HOST_CACHED`, `LAZILY_ALLOCATED`.
- Each resource reports `VkMemoryRequirements` (size, alignment, `memoryTypeBits`); the allocator picks a type whose bit is set and whose flags satisfy the intended usage.
- Discrete GPUs: separate VRAM heap (device local) and system RAM heap (host visible). Integrated GPUs: usually one heap that is both.
- `bufferImageGranularity` forces separation or padding between linear and optimal-tiling resources in the same block.

### Allocation count limit
`maxMemoryAllocationCount` guarantees only 4096 live allocations (minimum required by the spec; many drivers allow more, some platforms less in practice, verify per target). Per-resource `vkAllocateMemory` is therefore not viable; allocators carve blocks (commonly tens to hundreds of MiB) and suballocate.

### Suballocation strategies
| Strategy | Alloc | Free | Fragmentation | Typical use |
|---|---|---|---|---|
| Linear / bump | O(1) | whole block / ring reset | none inside a lifetime | per-frame uploads, staging rings, transient scratch |
| Stack (linear, LIFO) | O(1) | LIFO only | none | scoped temporary data |
| Free-list (first/best fit) | O(n) | O(1)+merge | moderate | general purpose, simple |
| Buddy | O(log n) | O(log n) | internal (power-of-two rounding) | fixed-size classes, cheap merge |
| TLSF (two-level segregated fit) | O(1) | O(1) | low external | general purpose, real-time friendly |

VMA offers linear, buddy and a TLSF-derived default; `gpu-allocator` offers a free-list allocator and a dedicated-block allocator (verify current set of strategies in docs.rs).

### Dedicated allocations
Large render targets, resources the driver prefers dedicated (`VK_KHR_dedicated_allocation`, core in 1.1: `VkMemoryDedicatedRequirements`) and resources needing external-memory export should get their own `VkDeviceMemory`. Suballocating them can be slower or unsupported on some drivers.

### Staging and upload
- Discrete GPU default path: host-visible staging buffer -> `vkCmdCopyBuffer/CopyBufferToImage` -> device-local resource.
- Persistent mapped ring buffers per frame slot avoid map/unmap and allow freeing a region once the frame's timeline value completes (RESEARCH-001).
- Non-coherent memory requires `vkFlushMappedMemoryRanges`/`vkInvalidate...` aligned to `nonCoherentAtomSize`.
- Large asset streaming favors a dedicated transfer queue (ownership transfer needed, RESEARCH-001).

### ReBAR / host-visible device-local memory
With Resizable BAR (PCIe) or on UMA hardware, a memory type that is both `DEVICE_LOCAL` and `HOST_VISIBLE` can be large (not limited to the traditional 256 MiB window). This allows writing directly into VRAM for dynamic data and skipping staging, at the cost of uncached/write-combined CPU access (write sequentially, never read back) and PCIe bandwidth for CPU writes. Whether to use it per resource class must be measured. Detect by inspecting memory types and heap size rather than assuming a vendor feature name.

### Aliasing for transient resources
Resources whose lifetimes within a frame do not overlap can share one memory range (`VkImage`/`VkBuffer` bound to the same offset). Requirements: compatible memory type bits, correct size/alignment, and a barrier with `oldLayout = UNDEFINED` plus an initializing write or clear at first use (contents of an aliased image are undefined). A render graph that knows lifetimes provides the intervals (RESEARCH-003). Allocators expose this via bind-to-existing-allocation or "aliasing" allocation flags (VMA: `vmaCreateAliasingImage`; verify in gpu-allocator).

### Deferred destruction
The GPU may still be reading a resource when the CPU drops it. Standard pattern: move the resource and its allocation into a queue tagged with the current frame index or timeline value; each frame, release entries whose tag the GPU has passed. Destroying views/descriptors referencing the resource follows the same rule. See ADR-0006.

### Defragmentation
Relocating live allocations needs the engine to recreate and copy resources and patch references. VMA includes a defragmentation API (`vmaBeginDefragmentation`) that plans moves for the caller to perform. Handle-based indirection (ADR-0006) makes moving feasible, but few games defragment VRAM; usually not required if allocation patterns are bounded.

## Analysis / Comparison

### gpu-allocator vs vk-mem
Facts below are from public repositories as of the author's knowledge; confirm versions on crates.io before deciding (verify).

| Aspect | `gpu-allocator` | `vk-mem` (VMA bindings) |
|---|---|---|
| Language | Pure Rust | Rust bindings over C++ VMA (compiled via `cc`) |
| Build dependencies | None beyond Rust; Vulkan feature uses `ash` | C++ compiler; bundles VMA source; bindgen optional (verify) |
| Vulkan binding | `ash` (feature `vulkan`) | `ash` |
| Other APIs | Optional D3D12 and Metal backends | Vulkan only |
| Maturity | Maintained by Traverse Research; used in several Rust engines and wgpu-adjacent projects (verify) | VMA is very mature in C++, industry standard; Rust crate tracks VMA releases with some lag (verify) |
| Strategies | Free-list sub-allocator, dedicated blocks | Default TLSF-like, linear, buddy pools, custom pools |
| Defragmentation | Not provided (verify) | Provided by VMA |
| Aliasing / custom pools | Limited (verify) | Supported |
| Budget / statistics | Reports allocation info and visualizer helpers (verify); `VK_EXT_memory_budget` usage not built in (verify) | Budget queries, JSON stats dump, rich statistics |
| Debugging | Optional leak logging, `visualizer` feature (verify) | Statistics string, debug margins, corruption detection |
| Unsafe surface | Small wrapper API; allocator internals safe Rust | Wraps C++ via FFI; allocator internals not auditable in Rust |
| Cross-compilation / CI | Simple | Needs C++ toolchain on every target |

### Where each shines
| Need | Better fit |
|---|---|
| Minimal build, pure Rust, auditable code | gpu-allocator |
| Defragmentation, custom pools, budget, aliasing helpers | VMA / vk-mem |
| Future D3D12/Metal backend sharing an allocator | gpu-allocator |
| Battle-tested heuristics | VMA |

## Implications for Anvil
- An allocator is an implementation detail of `rhi-vulkan` (MASTER-000 §8.4); no allocator type may leak into `rhi`.
- Anvil needs, in order: general device-local allocation, a linear per-frame staging ring, dedicated allocation for large targets, then transient aliasing (M4). Defragmentation is not needed early.
- `rhi-vulkan` should wrap the allocator behind a small internal interface so it can be swapped.
- Memory statistics (MASTER-000 §10.2 "expose memory diagnostics") need per-heap used/budget numbers; confirm the chosen crate or add `VK_EXT_memory_budget` queries ourselves.
- Deferred destruction keyed on timeline values is allocator-independent.

## Open questions
- Is aliasing/custom-pool support in gpu-allocator sufficient for the render graph's transient allocator, or do we write our own placement over dedicated blocks?
- How does each crate behave under `VK_EXT_memory_budget` pressure?
- Frame-ring size and policy when uploads exceed the ring (stall vs grow)?
- Is ReBAR memory worth a separate pool class?

## References
- Vulkan Specification, Memory Allocation: https://docs.vulkan.org/spec/latest/chapters/memory.html
- Khronos Vulkan Guide, Memory Allocation: https://docs.vulkan.org/guide/latest/memory_allocation.html
- GPUOpen Vulkan Memory Allocator: https://gpuopen.com/vulkan-memory-allocator/
- VMA documentation: https://gpuopen-librariesandsdks.github.io/VulkanMemoryAllocator/html/
- `gpu-allocator` on docs.rs: https://docs.rs/gpu-allocator
- `gpu-allocator` repository: https://github.com/Traverse-Research/gpu-allocator
- `vk-mem` on docs.rs: https://docs.rs/vk-mem
- `vk-mem-rs` repository: https://github.com/gwihlidal/vk-mem-rs (verify)
- Masmano et al., "TLSF: a New Dynamic Memory Allocator for Real-Time Systems", ECRTS 2004 (search by title; URL not verified)
