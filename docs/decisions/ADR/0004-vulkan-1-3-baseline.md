# ADR-0004: Vulkan 1.3 baseline

- Status: Accepted
- Date: 2026-11-02

## Context
Anvil targets modern desktop GPUs (Master Spec §4.2) and uses `ash` (ADR-0001). Barriers, frame pacing and attachment setup are central to the design (RESEARCH-001).

## Problem
Choose the minimum Vulkan version and which features are mandatory, since this decides the code paths we must write and test.

## Considered alternatives
1. Vulkan 1.0 plus extensions (`VK_KHR_dynamic_rendering`, `VK_KHR_synchronization2`, `VK_KHR_timeline_semaphore`, ...).
2. Vulkan 1.2 baseline, with the three features as optional extensions.
3. Vulkan 1.3 baseline.

## Decision
Require Vulkan 1.3 (instance and device `apiVersion >= 1.3`) and enable these features:
- `dynamicRendering`: no `VkRenderPass` or `VkFramebuffer` objects anywhere.
- `synchronization2`: `vkCmdPipelineBarrier2`, `vkQueueSubmit2`.
- `timelineSemaphore` (core 1.2): frame pacing and deferred destruction.

Further features (descriptor indexing/bindless, buffer device address) are decided in layer specs. Device selection rejects GPUs lacking the baseline with a clear message.

## Advantages
- Single code path; no extension fallbacks to write or test.
- Dynamic rendering removes render-pass and framebuffer compatibility management and fits a render graph.
- Synchronization2 gives per-barrier stage masks and 64-bit flags.
- Timeline semaphores simplify frame pacing and give a monotonic key for deferred destruction (ADR-0006).

## Disadvantages
- Excludes older GPUs/drivers without 1.3.
- No tile-based-GPU render-pass optimizations (subpass inputs); dynamic rendering local read is a later extension (verify).
- macOS via MoltenVK may not meet 1.3 (see below).

## Consequences
- Driver coverage (all "verify" against current releases):
  - Linux Mesa RADV (AMD): 1.3 support in current Mesa releases (verify minimum version).
  - Linux Mesa ANV (Intel): 1.3 on Gen9+ in current Mesa (verify per generation).
  - NVIDIA proprietary: 1.3 on recent drivers (verify minimum driver and GPU generation).
  - macOS MoltenVK: Vulkan 1.3 conformance status unverified; dynamic rendering and sync2 may be partial (verify). macOS is therefore not guaranteed for M1.
- Instance creation requests `apiVersion` 1.3; feature chain `VkPhysicalDeviceVulkan13Features` and `Vulkan12Features` is validated at startup.
- Image layout transitions and attachment load/store ops are explicit in the RHI.

## Revisit conditions
- A required target platform cannot offer Vulkan 1.3.
- Mobile/tile-based GPUs become a target and need render-pass or local-read features.
