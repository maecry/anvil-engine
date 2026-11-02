# RESEARCH-001: Vulkan Synchronization

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §4.2, §9.1, §9.3, §10.3; ADR-0004, ADR-0006 |
| Last updated | 2026-11-02 |

## Summary
Vulkan does no implicit synchronization between commands, queues or the presentation engine. Every hazard (RAW, WAR, WAW, layout change, cross-queue use, CPU/GPU sharing) needs an explicit dependency. `VK_KHR_synchronization2` (core in 1.3) replaces the per-stage/per-access split of the original API with a cleaner per-barrier form. Timeline semaphores (core in 1.2) replace most fence and binary-semaphore bookkeeping. Swapchain acquire/present remain binary-semaphore based and have one well-known reuse pitfall (see Analysis).

## Concepts

### Execution dependency vs memory dependency
- An **execution dependency** orders work: "stage X of later commands does not start until stage Y of earlier commands finished". It says nothing about data visibility.
- A **memory dependency** additionally makes writes *available* (flushed from the writer's caches toward memory) and *visible* (invalidated/readable at the reader's caches). It is defined by source/destination stage masks plus source/destination access masks.
- A barrier with stages but no access masks is only an execution dependency. Write-after-read needs only an execution dependency (no cache flush is needed), but read-after-write and write-after-write need availability and visibility.
- `VK_ACCESS_*_MEMORY_*` flags and `ALL_COMMANDS` are legal but over-synchronize; precise masks cost less on some GPUs (hardware-dependent, measure).

### Pipeline barriers and synchronization2
- `vkCmdPipelineBarrier2` takes a `VkDependencyInfo` with `VkMemoryBarrier2`, `VkBufferMemoryBarrier2` and `VkImageMemoryBarrier2`. Each barrier carries its own `srcStageMask/srcAccessMask/dstStageMask/dstAccessMask` (in the original API stages were global to the call).
- Stage and access flags are 64-bit (`VkPipelineStageFlags2`, `VkAccessFlags2`), adding finer stages such as `COPY`, `RESOLVE`, `BLIT`, `CLEAR`, `INDEX_INPUT`, `VERTEX_ATTRIBUTE_INPUT`.
- `vkQueueSubmit2` uses `VkSubmitInfo2` with per-semaphore stage masks and supports timeline values in the same struct.
- Events (`vkCmdSetEvent2`/`vkCmdWaitEvents2`) allow split barriers; rarely needed initially.

### Image layout transitions
- A layout transition is part of an image memory barrier (`oldLayout` -> `newLayout`) and executes between the source and destination scopes.
- `oldLayout = UNDEFINED` discards contents; use it for transient or fully overwritten targets.
- Typical layouts: `COLOR_ATTACHMENT_OPTIMAL`, `DEPTH_ATTACHMENT_OPTIMAL`, `SHADER_READ_ONLY_OPTIMAL`, `TRANSFER_SRC/DST_OPTIMAL`, `GENERAL`, `PRESENT_SRC_KHR`.
- Vulkan 1.3 / `VK_KHR_synchronization2` adds `ATTACHMENT_OPTIMAL` and `READ_ONLY_OPTIMAL` layouts that are not tied to a specific usage.
- With dynamic rendering (ADR-0004) there are no render-pass implicit transitions, so the engine must emit every attachment transition itself.

### Semaphores
| Kind | Signal/wait by | Properties |
|---|---|---|
| Binary | GPU only (queue submit, acquire, present) | Must have a pending signal before each wait; each signal consumed by exactly one wait. Required for swapchain acquire/present. |
| Timeline (1.2 core) | GPU and host (`vkSignalSemaphore`, `vkWaitSemaphores`) | Monotonic 64-bit counter; waiters wait for value >= N; one object can express many dependencies and per-frame completion. |

Timeline semaphores are not usable with swapchain acquire/present in core (those need binary semaphores). Platform and driver limits on the maximum value difference exist (`maxTimelineSemaphoreValueDifference`).

### Fences
`VkFence` is a binary CPU-waitable signal for queue submissions. Frames in flight can be paced with either one fence per frame slot or a single timeline semaphore (host `vkWaitSemaphores` for value `frame - N`). Fences are still required where an API only accepts fences (`vkAcquireNextImageKHR` optional fence, `VK_EXT_swapchain_maintenance1` present fences).

### Frames in flight
CPU records frame N+1 while GPU executes frame N. Per-frame-slot resources: command pools, upload ring regions, dynamic descriptor data, deferred-destruction queue. Typical count is 2 (3 for lower latency variance; measure). The timeline value (or fence) gates reuse of a slot and defines when deferred destruction is safe (see RESEARCH-002, ADR-0006).

### Swapchain acquire/present synchronization
1. `vkAcquireNextImageKHR(..., acquireSemaphore, ...)` signals a binary semaphore when the image is usable; the returned index is only known at the call.
2. The submit that renders into the image waits on `acquireSemaphore` at the stage where the image is first written (`COLOR_ATTACHMENT_OUTPUT` or `TRANSFER` for blits).
3. The same submit transitions the image to `PRESENT_SRC_KHR` and signals a binary `renderFinished` semaphore.
4. `vkQueuePresentKHR` waits on `renderFinished`.

**Known pitfall.** The semaphore waited on by present has no completion signal the application can observe in core Vulkan: when `vkQueuePresentKHR` returns, the app does not know when the presentation engine has finished waiting on it. Reusing a per-frame-in-flight `renderFinished` semaphore can therefore signal a semaphore that a previous present still waits on, and validation reports reuse of a semaphore that is still in use (VUID about `vkQueueSubmit` signaling a semaphore that may still have a pending wait; see the Vulkan-Docs swapchain semaphore reuse guidance, verify exact VUID). The accepted workaround is one present-wait (`renderFinished`) semaphore **per swapchain image**, indexed by the acquired image index; an image can only be re-acquired after its previous present has consumed the semaphore. The acquire semaphores are the opposite case: they are indexed by frame slot (or a free-list), because the image index is not known before the acquire call. `VK_EXT_swapchain_maintenance1` (promoted in part to `VK_KHR_swapchain_maintenance1`, verify) adds `VkSwapchainPresentFenceInfoEXT`, letting the app learn when a present operation's semaphores are reusable, and also supports releasing acquired images and present-mode switching. Support must be queried; a per-image semaphore array remains the portable baseline.

### Queue family ownership transfer
For resources with `VK_SHARING_MODE_EXCLUSIVE` used on a different queue family, ownership must be transferred with a **release** barrier on the source queue and a matching **acquire** barrier on the destination queue, with identical `srcQueueFamilyIndex`/`dstQueueFamilyIndex` (and identical layout transition for images). The two halves must be ordered by a semaphore. In the release barrier the destination access/stage masks are ignored (treated as 0 / `NONE`), and in the acquire barrier the source masks are ignored. With synchronization2 the same struct is used for both halves. `VK_SHARING_MODE_CONCURRENT` avoids transfers at the cost of possibly lower performance (driver dependent). Relevant for async compute and dedicated transfer queues.

## Analysis / Comparison

### Synchronization primitives
| Primitive | Scope | CPU-visible | Typical use |
|---|---|---|---|
| Pipeline barrier | Within one queue, one command buffer stream | No | Hazards between passes, layout transitions |
| Event | Within one queue, split barrier | Host can set/reset | Overlapping unrelated work between signal and wait |
| Binary semaphore | Between queue operations / present | No | Swapchain, legacy cross-queue |
| Timeline semaphore | Between queues and host | Yes | Frame pacing, cross-queue, upload completion, deferred destruction |
| Fence | Queue -> host | Yes | Legacy frame pacing, swapchain maintenance1 present fence |

### Per-frame vs per-image swapchain semaphores
| Semaphore | Indexed by | Reason |
|---|---|---|
| acquire | frame slot (or free-list) | Image index unknown until acquire returns |
| present-wait (renderFinished) | swapchain image index | Only safe reuse point is re-acquire of that image (unless maintenance1 present fences are used) |
| frame pacing | frame slot, or one timeline | CPU/GPU throttling |

### Fence-per-frame vs one timeline
| Aspect | Fence per frame slot | Single timeline |
|---|---|---|
| Objects | N fences | 1 semaphore |
| Cross-queue use | needs extra semaphores | same object |
| Deferred destruction key | frame index | monotonic value |
| Extra feature requirement | none | timeline semaphores (core 1.2) |

## Implications for Anvil
- Baseline Vulkan 1.3 gives synchronization2 and timeline semaphores without extension juggling (ADR-0004).
- One timeline semaphore per queue is a natural key for frame pacing and deferred destruction (ADR-0006); binary semaphores are confined to swapchain code.
- Swapchain code must allocate present-wait semaphores per swapchain image and recreate them with the swapchain. Optionally use maintenance1 when present.
- Dynamic rendering removes implicit render-pass transitions, so a barrier source is needed: the render graph (MASTER-000 §9.3) should derive barriers from declared resource usage rather than leaving them to pass authors (RESEARCH-003).
- Validation layers with synchronization validation (`VK_VALIDATION_FEATURE_ENABLE_SYNCHRONIZATION_VALIDATION_EXT`) should be part of debug CI.
- Queue ownership transfer is only needed when a dedicated transfer or compute queue is introduced; the first version can use a single graphics+compute+transfer queue.

## Open questions
- Is a separate transfer queue worth the ownership-transfer complexity for uploads, or is a ring buffer on the graphics queue enough?
- 2 or 3 frames in flight as default?
- Do we require `VK_KHR_swapchain_maintenance1` on platforms where it is available, or keep per-image semaphores only?
- How are barriers tracked for resources not managed by the render graph (long-lived textures, uploads)?

## References
- Vulkan Specification, Synchronization and Cache Control: https://docs.vulkan.org/spec/latest/chapters/synchronization.html
- Vulkan-Docs, Synchronization Examples: https://github.com/KhronosGroup/Vulkan-Docs/wiki/Synchronization-Examples
- Khronos Vulkan Guide, Synchronization: https://docs.vulkan.org/guide/latest/synchronization.html
- Khronos blog, "Vulkan Timeline Semaphores": https://www.khronos.org/blog/vulkan-timeline-semaphores
- Vulkan Guide, "Swapchain Semaphore Reuse" (verify page path): https://docs.vulkan.org/guide/latest/swapchain_semaphore_reuse.html
- `VK_EXT_swapchain_maintenance1` proposal: https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_swapchain_maintenance1.html (verify)
- Khronos, "Understanding Vulkan Synchronization" blog: https://www.khronos.org/blog/understanding-vulkan-synchronization
