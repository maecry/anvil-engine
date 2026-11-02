# MODULE-004: Command Manager

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan |
| Layer | LAYER-004 |
| Milestone | M1 |
| Traces to | FR-VK-006, FR-VK-007, FR-RHI-011, FR-RHI-013, FR-RHI-006, FR-RHI-007, FR-RHI-017, FR-VK-008 (M1 subset), FR-VK-012, FR-RES-006 (mechanism), NFR-CONC-002, NFR-PERF-015, NFR-REL-004, NFR-QUAL-004; MASTER-000 §8.4, §10.3, §14 M1; RESEARCH-001; ADR-0004, ADR-0006 |
| Last updated | 2026-11-02 |

## 1. Purpose

Own everything that turns recorded commands into completed GPU work: frame slots (command pool + command buffer + acquire semaphore per slot), the graphics timeline semaphore, the owned command encoder, the `TextureState` -> synchronization2 barrier translation, queue submission, and the deferred-destruction queue keyed on timeline value. It also holds the minimal M1 shader-module and graphics-pipeline objects needed by the triangle (absorbed by MODULE-007 in M2).

## 2. Responsibilities

- `begin_frame`, `begin_commands`, `submit`, `wait_idle`.
- Per-slot command pools, one primary command buffer per slot, reset-by-pool strategy.
- Graphics timeline semaphore: wait for slot reuse, completion value, `last_submitted`.
- Deferred destruction queue and its retirement each frame (contract: LAYER-003 §7.2, ADR-0006).
- `VulkanCommands` (implements `CommandEncoder`) with recorded-first-error semantics.
- Barrier translation table and `vkCmdPipelineBarrier2` emission.
- `create_shader_module`, `create_graphics_pipeline`, `destroy_*` (M1 minimal objects, pool-backed handles).
- Debug labels and object names.

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Swapchain acquire/present calls and render-done semaphores | MODULE-003 (this module lends the acquire semaphore and consumes the render-done semaphore at submit) |
| Buffer/texture creation, staging, uploads | MODULE-005/006 (M2) |
| Automatic barrier derivation | render graph, MODULE-009 (M4); M1 barriers are explicit |
| Multiple queues, parallel recording, secondary buffers | M5 |
| VkPipelineCache persistence, pipeline creation policy | MODULE-007 (M2/M5) |
| Shader compilation | MODULE-008; this module only receives SPIR-V words |

## 4. Dependencies

Allowed: MODULE-002 (`DeviceShared`), MODULE-003 (render-done lookup), foundation (`Pool`, `Handle`), rhi, `ash`, `tracing`, `thiserror`. Forbidden: platform, shader (SPIR-V arrives as `&[u32]`, D4), layer 3+ crates.

## 5. Public API

```rust
// ---- implemented for VulkanDevice (rhi::Device, LAYER-003 §8) ----
fn begin_frame(&mut self) -> Result<FrameToken, RhiError>;
fn begin_commands(&mut self, frame: &FrameToken) -> Result<VulkanCommands, RhiError>;     // owned encoder (D2)
fn submit(&mut self, cmds: VulkanFinished, image: &SwapchainImage) -> Result<(), RhiError>;
fn wait_idle(&self) -> Result<(), RhiError>;
fn create_shader_module(&self, desc: &ShaderModuleDesc) -> Result<ShaderModuleHandle, RhiError>;
fn create_graphics_pipeline(&self, desc: &GraphicsPipelineDesc) -> Result<PipelineHandle, RhiError>;
fn destroy_shader_module(&self, h: ShaderModuleHandle);
fn destroy_pipeline(&self, h: PipelineHandle);

pub struct VulkanCommands { /* private */ }      // impl rhi::CommandEncoder, Finished = VulkanFinished
pub struct VulkanFinished { /* private: cb, slot, epoch */ }
// rhi::FrameToken is defined in rhi; the backend fills it with the slot index and frame counter.
```

```rust
// ---- internal ----
pub(crate) struct SyncShared {                         // lives in DeviceShared so `&self` create/destroy can enqueue
    pub timeline: vk::Semaphore,                       // TIMELINE, initial value 0
    pub last_submitted: AtomicU64,                     // highest value ever submitted (0 = none)
    pub deferred: Mutex<DeferredQueue>,
}
impl SyncShared {
    pub(crate) fn new(device: &ash::Device) -> Result<Self, RhiError>;
    pub(crate) fn doom_tag(&self) -> u64 { self.last_submitted.load(Acquire) + 1 }   // see 6.2
    pub(crate) fn defer(&self, d: Doomed);             // push(doom_tag(), d)
}

pub(crate) struct FrameSlot {
    pub pool: vk::CommandPool, pub cb: vk::CommandBuffer, pub acquire_sem: vk::Semaphore,
    pub epoch: Arc<AtomicU64>,                         // bumped at each begin_frame; encoders compare in finish()
    pub state: SlotState,
}
pub(crate) enum SlotState { Idle, Open /* begin_commands called */, Submitted }

pub(crate) struct SlotTracker { values: Vec<u64>, frame_counter: u64, last_submitted: u64 }   // pure model, no Vulkan
impl SlotTracker {
    pub(crate) fn new(frames_in_flight: u32) -> Self;
    pub(crate) fn begin_frame(&mut self) -> (usize /* slot */, u64 /* timeline value to wait for; 0 = none */);
    pub(crate) fn record_submit(&mut self, slot: usize) -> u64;     // ++last_submitted; values[slot] = it; returns it
    pub(crate) fn next_value(&self) -> u64;                          // last_submitted + 1
}

pub(crate) struct CommandManager {
    shared: Arc<DeviceShared>, slots: Vec<FrameSlot>, tracker: SlotTracker,
    current: Option<usize>,                            // slot of the open frame
    acquired: bool,                                    // acquire semaphore pending for this frame's submit
}
impl CommandManager {
    pub(crate) fn new(shared: Arc<DeviceShared>, frames_in_flight: u32) -> Result<Self, RhiError>;
    pub(crate) fn begin_frame(&mut self) -> Result<FrameToken, RhiError>;
    pub(crate) fn begin_commands(&mut self, t: &FrameToken) -> Result<VulkanCommands, RhiError>;
    pub(crate) fn acquire_semaphore(&self) -> Result<vk::Semaphore, RhiError>;   // MODULE-003: Err(InvalidDescriptor) outside a frame or if already acquired
    pub(crate) fn mark_acquired(&mut self);
    pub(crate) fn submit(&mut self, f: VulkanFinished, wait_acquire: bool, signal: Option<vk::Semaphore>) -> Result<u64, RhiError>;   // submit(f, image) calls this with render_done[image.index]
    pub(crate) fn shutdown(&mut self);                 // destroys slots after wait idle
}

pub(crate) enum Doomed {
    ShaderModule(vk::ShaderModule),
    Pipeline { pipeline: vk::Pipeline, layout: vk::PipelineLayout },
    // M2 adds Buffer/Image/ImageView/Sampler(+ gpu_allocator::vulkan::Allocation) in MODULE-005/006
}
pub(crate) struct DeferredQueue { items: VecDeque<(u64, Doomed)> }
impl DeferredQueue {
    pub(crate) fn push(&mut self, tag: u64, d: Doomed);                 // debug_assert!(tag >= back tag)
    pub(crate) fn pop_ready(&mut self, completed: u64) -> Option<Doomed>;   // front tag <= completed; no allocation
    pub(crate) fn pop_any(&mut self) -> Option<Doomed>;                 // shutdown drain after wait idle
    pub(crate) fn len(&self) -> usize;
}
pub(crate) unsafe fn destroy_doomed(device: &ash::Device, d: Doomed);

// ---- barrier translation (pure) ----
pub(crate) struct BarrierHalf { pub layout: vk::ImageLayout, pub stage: vk::PipelineStageFlags2, pub access: vk::AccessFlags2 }
pub(crate) fn state_as_source(s: TextureState) -> BarrierHalf;
pub(crate) fn state_as_destination(s: TextureState) -> Result<BarrierHalf, RhiError>;   // Undefined -> InvalidDescriptor
pub(crate) fn image_barrier(image: vk::Image, aspect: vk::ImageAspectFlags, from: TextureState, to: TextureState)
    -> Result<vk::ImageMemoryBarrier2<'static>, RhiError>;

// ---- encoder state machine (pure) ----
pub(crate) struct EncoderState { first_error: Option<RhiError>, rendering: bool, label_depth: u32, formats: [Option<Format>; MAX_COLOR_ATTACHMENTS] }
pub(crate) const MAX_COLOR_ATTACHMENTS: usize = 8;
impl EncoderState {
    pub(crate) fn begin_rendering(&mut self, formats: &[Format]) -> bool;   // false (and records InvalidDescriptor) if already rendering
    pub(crate) fn end_rendering(&mut self) -> bool;
    pub(crate) fn check_draw(&mut self) -> bool;
    pub(crate) fn check_pipeline(&mut self, p: &[Format]) -> bool;           // pipeline color formats must equal the active rendering's
    pub(crate) fn push_label(&mut self); pub(crate) fn pop_label(&mut self) -> bool;
    pub(crate) fn record(&mut self, e: RhiError);                            // keeps the FIRST error only
    pub(crate) fn finish(self) -> Result<(), RhiError>;                      // first_error, else unbalanced rendering/labels
}
pub(crate) fn encode_label(name: &str) -> [u8; 128];                         // truncated at a char boundary, NUL-terminated; no heap
```

## 6. Data structures

### 6.1 Frame-slot lifecycle (timeline semaphore)

`slot = frame_counter % frames_in_flight`; `frame_counter` increments in `begin_frame`.

```
 Idle --begin_frame (wait timeline >= slot.value; reset pool)--> Idle(ready)
 Idle --begin_commands (begin_command_buffer ONE_TIME_SUBMIT)--> Open
 Open --encoder.finish + submit (signal timeline = ++last_submitted)--> Submitted(slot.value = that)
 Submitted --(timeline >= slot.value, observed at next begin_frame of this slot)--> Idle
 Open/Idle --frame abandoned (acquire OutOfDate / Minimized, no submit)--> Idle   (slot.value unchanged: nothing new was submitted)
```

`slot.value == 0` means never submitted: no wait. The wait is a host `vkWaitSemaphores` on the single timeline; there are no per-frame fences (RESEARCH-001, ADR-0004). Binary semaphores exist only for WSI: acquire (per slot, here) and render-done (per swapchain image, MODULE-003).

### 6.2 Deferred destruction keyed on timeline value

| Rule | Detail |
|---|---|
| Tag | `doom_tag = last_submitted + 1`: the value the next submit will signal. It is >= the value of any submission that may reference the object, including commands recorded but not yet submitted (LAYER-003 §7.2) |
| Retire | An entry is destroyed when `completed >= tag`; `completed` is read with `vkGetSemaphoreCounterValue` once per `begin_frame` after the slot wait |
| Order | Tags are non-decreasing in push order, so retirement pops from the front and stops at the first not-ready entry |
| Handle | The pooled handle becomes stale immediately at `destroy_*` (generation bump, `Pool::remove`); the native object lives in the queue until retired |
| Lag | An object doomed late in a frame is retired about `frames_in_flight` frames later; with no further submits it stays until shutdown drains it after `device_wait_idle` |
| Failed submit | `last_submitted` and `slot.value` are updated only after `vkQueueSubmit2` succeeds |

`maxTimelineSemaphoreValueDifference` is not a concern: the host only waits for values the GPU is at most `frames_in_flight` submits away from.

### 6.3 Barrier translation (`TextureState` -> synchronization2)

Whole-image color barrier: `subresource_range = { COLOR, base_mip 0, level_count REMAINING_MIP_LEVELS, base_layer 0, layer_count REMAINING_ARRAY_LAYERS }` (verified 2026-11-02), queue family indices `QUEUE_FAMILY_IGNORED`. `src_*` come from `state_as_source(from)`, `dst_*` from `state_as_destination(to)`, `old_layout/new_layout` from the layouts.

| TextureState | Layout | As source: stage / access | As destination: stage / access |
|---|---|---|---|
| Undefined | UNDEFINED | COLOR_ATTACHMENT_OUTPUT / NONE | not allowed (`InvalidDescriptor`) |
| ColorAttachment | COLOR_ATTACHMENT_OPTIMAL | COLOR_ATTACHMENT_OUTPUT / COLOR_ATTACHMENT_WRITE | COLOR_ATTACHMENT_OUTPUT / COLOR_ATTACHMENT_READ + COLOR_ATTACHMENT_WRITE |
| Present | PRESENT_SRC_KHR | COLOR_ATTACHMENT_OUTPUT / NONE | NONE / NONE |
| ShaderRead | SHADER_READ_ONLY_OPTIMAL | VERTEX_SHADER + FRAGMENT_SHADER + COMPUTE_SHADER / NONE | same stages / SHADER_SAMPLED_READ |
| TransferDst | TRANSFER_DST_OPTIMAL | ALL_TRANSFER / TRANSFER_WRITE | ALL_TRANSFER / TRANSFER_WRITE |
| TransferSrc | TRANSFER_SRC_OPTIMAL | ALL_TRANSFER / NONE | ALL_TRANSFER / TRANSFER_READ |

Principles: source access holds write bits only (read-only states need an execution dependency, not availability); destination access holds exactly what the next use needs; never `ALL_COMMANDS` or `*_MEMORY_*` flags in barriers. Standard sequences:

| Step | Barrier | Pairs with |
|---|---|---|
| acquire -> render | `Undefined -> ColorAttachment` | submit waits the acquire semaphore at COLOR_ATTACHMENT_OUTPUT; the barrier's source stage COLOR_ATTACHMENT_OUTPUT chains the layout transition after that wait |
| render -> present | `ColorAttachment -> Present` | submit signals render-done after all commands; present waits it |

M1 rule, deliberate difference from the LAYER-004 §7.6 table: `Undefined` and `Present` as source use `COLOR_ATTACHMENT_OUTPUT` instead of `TOP_OF_PIPE`/`NONE`. A layout transition with an earlier source stage can execute before the acquire semaphore wait completes, which synchronization validation reports as a hazard. This is valid because M1 `submit` always waits the acquire semaphore at COLOR_ATTACHMENT_OUTPUT. Non-swapchain `Undefined` sources get a narrower stage in M2 (MODULE-006). verify: confirm against sync-validation output on lavapipe and one real driver. `from == to` is legal (emits a same-layout memory barrier, e.g. ColorAttachment -> ColorAttachment between passes).

### 6.4 M1 pipeline objects

`VkShaderModule { raw, label }` and `VkPipeline { pipeline, layout, color_formats: [Option<Format>; 8] }` live in `pools.shader_modules` / `pools.pipelines` (RwLock). Pipeline state: dynamic rendering (`PipelineRenderingCreateInfo { color_attachment_formats }`, no render pass), empty vertex input, empty pipeline layout (no sets, no push constants; the triangle uses `gl_VertexIndex`), viewport and scissor dynamic with count 1, `polygon_mode FILL`, `line_width 1.0`, 1 sample, no depth/stencil (a `depth_format` of `Some` -> `Unsupported{what:"depth attachments (M2)"}`), blend array empty means opaque write-all for each color target (else its length must equal `color_formats.len()`). M1 coordinates are native Vulkan (D3): the example should use `CullMode::None` so winding conventions cannot hide the triangle.

## 7. Ownership and lifetime

| Object | Owner | Created | Destroyed |
|---|---|---|---|
| `VkSemaphore` timeline | `SyncShared` in `DeviceShared` | `create_device` step 9 | `DeviceShared::drop`, after wait idle |
| command pool, command buffer, acquire semaphore (per slot) | `FrameSlot` in `CommandManager` | `CommandManager::new` | `CommandManager::shutdown` (pool destroy frees the buffer) |
| `VulkanCommands` | the caller, owned, `Send` | `begin_commands` | consumed by `finish`; dropped unfinished -> slot stays `Open` until the next `begin_frame` of that slot resets it |
| `VulkanFinished` | caller, consumed by `submit` | `finish` | `submit` |
| `VkShaderModule`, `VkPipeline`, `VkPipelineLayout` | pool entries; native objects move to `DeferredQueue` at `destroy_*` | `create_*` | `destroy_doomed` after retirement, or shutdown drain |

Create order in `CommandManager::new`: for each slot: pool (`TRANSIENT`), allocate 1 primary command buffer, acquire semaphore (binary). On error destroy what exists in reverse. Destroy order in `shutdown`: after `device_wait_idle` and the deferred drain, per slot: acquire semaphore, command pool.

Encoder lifetime rule: an encoder must be finished (or dropped) before the next `begin_frame` that reuses its slot, because that call resets the pool. `finish` and `submit` check `slot.epoch` against the epoch captured at `begin_commands` and fail with `InvalidDescriptor("encoder outlived its frame")`.

Unsafe invariants:

| Block | Invariant to justify |
|---|---|
| `create_command_pool`, `allocate_command_buffers` | queue family is the device's; pool freed only in `shutdown` after idle |
| `reset_command_pool` | executed in `begin_frame` only after the timeline wait proved the slot's last submission completed, and no encoder from the previous use is still recording (epoch rule) |
| `begin_command_buffer` / `cmd_*` / `end_command_buffer` | the buffer belongs to an `Open` slot whose pool was reset this frame; single thread per pool; handles were resolved against the pools at record time under the read lock |
| `cmd_*` using `vk::ImageView`/`vk::Pipeline` resolved from handles | native objects stay alive: stale handles are rejected at record time, and a handle destroyed later is deferred past the submission that records it (`doom_tag`) |
| `queue_submit2` | queue mutex held; every `*SubmitInfo` slice and the semaphore handles live through the call; the acquire wait is only included if `acquired` is true; the render-done semaphore is unsignalled (re-acquired image) |
| `wait_semaphores` / `get_semaphore_counter_value` | the timeline semaphore is alive (owned by `DeviceShared`); values waited were previously queued for signal by a successful submit |
| `destroy_doomed` | tag retirement proved the GPU no longer uses the object; exactly-once because entries are popped |
| `device_wait_idle` | queue mutex held |

## 8. Execution flow

### 8.1 `begin_frame`

1. `current.is_some()` (previous frame never submitted nor abandoned explicitly) is allowed: an abandoned frame is simply replaced; log at `trace`.
2. `(slot, wait) = tracker.begin_frame()`.
3. If `wait > 0`: `device.wait_semaphores(&SemaphoreWaitInfo { semaphores: [timeline], values: [wait], flags: empty }, u64::MAX)`. `TIMEOUT` is impossible with an infinite timeout; errors map via section 10.
4. `completed = device.get_semaphore_counter_value(timeline)`.
5. Retire: `while let Some(d) = deferred.pop_ready(completed) { destroy_doomed(d) }` (lock held per pop, not across Vulkan calls).
6. `device.reset_command_pool(slot.pool, CommandPoolResetFlags::empty())` (no `RELEASE_RESOURCES`: memory is reused across frames; verify the trade-off by measurement later).
7. `slot.epoch += 1; slot.state = Idle; acquired = false; current = Some(slot)`.
8. Return `FrameToken { slot, frame }`.

### 8.2 `begin_commands(token)`

1. `token.slot == current` else `InvalidDescriptor`; `state == Idle` else `InvalidDescriptor("commands already begun for this frame")`.
2. `begin_command_buffer(cb, CommandBufferBeginInfo { flags: ONE_TIME_SUBMIT })`.
3. `state = Open`; return `VulkanCommands { shared: Arc clone, cb, slot, epoch, state: EncoderState::default() }`.

### 8.3 Encoder methods (each: if `first_error` set, return; resolve handles under the pool read lock; on failure `record(err)` and return)

| Method | Vulkan | Checks (record error) |
|---|---|---|
| `texture_barrier(t, from, to)` | resolve texture -> `image`, aspect from format (color in M1); `cmd_pipeline_barrier2(cb, &DependencyInfo { image_memory_barriers: [image_barrier(..)] })` | `StaleHandle{texture}`; `to == Undefined` -> `InvalidDescriptor`; not inside rendering (`InvalidDescriptor`) |
| `begin_rendering(desc)` | resolve each color texture -> view/format; `RenderingAttachmentInfo { image_view, image_layout COLOR_ATTACHMENT_OPTIMAL, load_op, store_op, clear_value float32 }`; `RenderingInfo { render_area {0,0,w,h}, layer_count 1, color_attachments }`; `cmd_begin_rendering`; then default `cmd_set_viewport` (x 0, y 0, w, h, depth 0..1, positive height; D3) and `cmd_set_scissor` of the full area | already rendering; `color.len() > 8` or 0; `desc.depth.is_some()` -> `Unsupported` (no depth textures until M2); zero w/h; stale texture |
| `end_rendering` | `cmd_end_rendering` | not rendering |
| `bind_pipeline(p)` | resolve pipeline; `cmd_bind_pipeline(cb, GRAPHICS, pipeline)` | stale; `check_pipeline`: pipeline color formats equal the active rendering formats; must be inside rendering |
| `set_viewport(v)` / `set_scissor(r)` | `cmd_set_viewport` / `cmd_set_scissor` | inside rendering |
| `draw(..)` | `cmd_draw` | `check_draw`: inside rendering and a pipeline bound |
| `begin_label(name, color)` | `cmd_begin_debug_utils_label` with `encode_label(name)`; no-op if debug_utils is absent (depth still counted) | - |
| `end_label()` | `cmd_end_debug_utils_label` | unbalanced -> record |
| `finish()` | `EncoderState::finish` (first error, then unbalanced rendering/labels), epoch check, `end_command_buffer` | any failure returns `Err`; the buffer is left to the next pool reset |

### 8.4 `submit(finished, image)`

1. `finished.slot == current` and epoch matches else `InvalidDescriptor`; `acquired` must be true (the image proves an acquire) else `InvalidDescriptor`.
2. `render_done = swapchains.render_done(image)` (MODULE-003; `StaleHandle` if the swapchain was recreated since acquire).
3. `value = tracker.next_value()`.
4. Build one `SubmitInfo2`:
   - wait: `{ acquire_sem[slot], value 0, stage COLOR_ATTACHMENT_OUTPUT }`
   - command buffer: `{ cb }`
   - signal: `{ render_done[image.index], value 0, stage ALL_COMMANDS }` and `{ timeline, value, stage ALL_COMMANDS }` (verify: ALL_COMMANDS is the conservative signal stage; narrower stage after validation review).
5. Lock queue; `queue_submit2(queue, &[info], vk::Fence::null())`; unlock.
6. On success: `tracker.record_submit(slot)`, `last_submitted.store(value, Release)`, `state = Submitted`, `acquired = false`. On error: nothing committed; map error; `DeviceLost` is fatal.

`submit_unwindowed` (`pub(crate)`, used by tests and later headless rendering): same without the acquire wait and render-done signal; only the timeline is signalled.

### 8.5 `wait_idle`

Lock queue; `device_wait_idle`; map errors. Not used in the frame loop (NFR-PERF-015); used by resize, destroy-swapchain and shutdown.

### 8.6 `create_shader_module` / `create_graphics_pipeline` / `destroy_*`

`create_shader_module`: `spirv.len() >= 5` (header) and `spirv[0] == 0x0723_0203` else `InvalidDescriptor("not SPIR-V")`; `create_shader_module(&ShaderModuleCreateInfo { code })`; insert into pool; name via `set_name`.

`create_graphics_pipeline`: resolve both module handles (read lock; `StaleHandle`); build stage infos with `CString` entry names (NUL -> `InvalidDescriptor`); create the empty pipeline layout; build `GraphicsPipelineCreateInfo` per section 6.4 with `p_next` -> `PipelineRenderingCreateInfo`; `create_graphics_pipelines(shared.pipeline_cache, &[info], None)` (ash returns `Err((pipelines, result))` on failure: destroy any non-null pipeline, destroy the layout); insert into pool; name it.

`destroy_*`: `Pool::remove` (stale now), then `sync.defer(Doomed::..)`. Unknown/stale handle: `warn!`, no-op.

## 9. Threading model

M1: `begin_frame`, `begin_commands`, `submit`, `acquire`, `present` take `&mut self` and run on one thread. `VulkanCommands: Send` (may be recorded on another thread while the owner keeps `&mut Device`, but only one encoder exists per slot and per frame in M1). Locks: `pools.*` RwLocks (read on encoder calls: `texture_barrier`, `begin_rendering`, `bind_pipeline`, a handful per frame), `sync.deferred` Mutex (push on destroy, pop in `begin_frame`), queue Mutex (submit/present/idle). The deferred lock is never held across a Vulkan call. Vulkan external synchronization: one command pool per slot, used by one thread at a time; the queue is guarded by the mutex. Parallel recording (a pool per thread per slot) and a submit thread are M5.

## 10. Error handling

| Source | Result |
|---|---|
| `ERROR_DEVICE_LOST` (wait, submit, idle) | `DeviceLost`; the app stops; no auto-retry |
| `ERROR_OUT_OF_HOST_MEMORY` / `ERROR_OUT_OF_DEVICE_MEMORY` | `OutOfHostMemory` / `OutOfDeviceMemory` |
| stale handle at record time | recorded; surfaces from `finish()` as `StaleHandle{kind}` |
| misuse (draw outside rendering, double begin, wrong slot, format mismatch, unbalanced labels) | `InvalidDescriptor(<message>)`, recorded or returned |
| unsupported M1 feature (depth, >8 attachments) | `Unsupported` |
| any other `vk::Result` | `Backend{op:"vkQueueSubmit2" etc., code}` |

Validation messages are logged by MODULE-001, never converted to errors. `destroy_*` never fails. No `unwrap()` outside tests.

## 11. Performance requirements

- Steady-state frame loop performs no heap allocation in this module: fixed-size arrays for submit/barrier/attachment slices, `encode_label` uses a stack buffer, deferred queue capacity is retained (`VecDeque`). Verified by an allocation-counting test allocator in a unit test around `begin_frame`..`submit` (measure, then assert zero).
- Exactly one `vkWaitSemaphores`, one `vkGetSemaphoreCounterValue`, one `vkResetCommandPool`, one `vkQueueSubmit2` per frame; no `vkDeviceWaitIdle` (NFR-PERF-015).
- Locks: at most one uncontended `Mutex` (queue) per submit; encoder pool read locks are per call, not per draw. `draw`, `set_viewport`, `set_scissor` take no locks.
- Counters exposed for later `MemorySnapshot`/profiler: frame number, `deferred.len()`, `last_submitted`. No numeric targets (NFR-PERF-012).

## 12. Testing requirements

Unit tests (no GPU):

| Test | Checks |
|---|---|
| `tracker_slot_rotation_n1_n2_n3` | slot = frame % N for N = 1, 2, 3 |
| `tracker_first_frames_do_not_wait` | wait value 0 until a slot was submitted |
| `tracker_waits_for_slot_value` | with N = 2, frame 2 waits the value of frame 0's submit |
| `tracker_abandoned_frame_keeps_value` | begin without submit leaves `values[slot]` unchanged |
| `tracker_values_strictly_increase` | `record_submit` returns 1, 2, 3, ... |
| `deferred_ordering_fifo` | pushes with tags 3, 3, 5; `pop_ready(4)` returns two, stops at 5 |
| `deferred_equal_tag_retires` | `completed == tag` is ready; `completed == tag - 1` is not |
| `deferred_push_decreasing_tag_asserts` | debug assertion fires |
| `deferred_drain_returns_all` | `pop_any` order and emptiness |
| `doom_tag_is_next_submit_value` | `last_submitted = 7` -> tag 8; object doomed while its commands are unsubmitted retires only after that submit completes |
| `barrier_table_total` | for all 6x5 valid `(from, to)` pairs `image_barrier` returns `Ok` with layouts from the table |
| `barrier_to_undefined_rejected` | `InvalidDescriptor` |
| `barrier_swapchain_acquire_to_color` | exact stage/access/layout values of section 6.3 row sequence |
| `barrier_color_to_present` | src COLOR_ATTACHMENT_OUTPUT + COLOR_ATTACHMENT_WRITE, dst NONE/NONE, layouts COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR |
| `barrier_source_access_is_write_only` | property over all states: source access has no read bits |
| `barrier_never_all_commands` | no barrier uses ALL_COMMANDS or MEMORY_* |
| `encoder_state_first_error_wins` | second `record` ignored |
| `encoder_state_rendering_machine` | begin twice, end without begin, draw outside, unbalanced at finish |
| `encoder_state_pipeline_format_mismatch` | `check_pipeline` records `InvalidDescriptor` |
| `encoder_state_labels_balance` | push/pop and underflow |
| `encode_label_truncates_on_char_boundary` | multi-byte char at the cut, NUL-terminated, 128 bytes |
| `spirv_header_check` | empty, short, wrong magic rejected; valid header accepted |
| `pipeline_desc_validation` | blend length mismatch, `depth_format: Some` -> `Unsupported`, NUL in entry name |
| `error_mapping_table` | `vk::Result` -> `RhiError` for the section 10 rows |
| `commands_and_finished_are_send` | compile-time assertion |

Integration tests (lavapipe on CI, `ANVIL_GPU_TESTS=1`, validation `On`, sync validation on; assert `validation_counts()` is zero at the end unless stated):

| Test | Checks |
|---|---|
| `headless_frames_n1_n2_n3` | 1000 empty frames through `submit_unwindowed`: timeline counter equals frames submitted; no validation messages |
| `timeline_value_monotonic_and_matches_slots` | after each frame the counter value read equals `last_submitted` once idle |
| `deferred_destroy_not_before_gpu` | destroy a pipeline right after submitting commands that bind it; assert it is still in the queue until `completed >= tag`, then gone (inspect `deferred.len()` and counter) |
| `destroy_while_recording_is_safe` | destroy a pipeline after `bind_pipeline` but before `finish`/`submit`; the submission is validation-clean |
| `stale_handle_bind_reports_at_finish` | bind a destroyed pipeline -> `finish()` returns `StaleHandle` |
| `begin_commands_twice_errors` / `encoder_outlives_frame_errors` | `InvalidDescriptor` |
| `create_shader_module_and_pipeline_from_fixed_spirv` | committed `triangle.vert.spv`/`triangle.frag.spv`; pipeline creation succeeds on lavapipe; destroy; shutdown with zero leaks |
| `pipeline_format_mismatch_is_reported` | pipeline built for `Bgra8Srgb` bound in a rendering targeting `Rgba8Unorm` -> error at `finish()` |
| `triangle_300_frames_with_swapchain` | in-crate headless surface (MODULE-003): acquire, barrier, render, barrier, submit, present x 300, zero validation messages, sync validation on |
| `triangle_frames_resize_every_30` | same loop with resizes (MODULE-003 recreation path) |
| `shutdown_drains_deferred` | items still queued at shutdown are destroyed; no "object not destroyed" messages |
| `unwindowed_submit_after_device_idle_waits_nothing` | `begin_frame` after `wait_idle` does not block |

## 13. Acceptance criteria

- [ ] Per slot: one command pool (TRANSIENT) and one primary command buffer, reset by `vkResetCommandPool` in `begin_frame` after the timeline wait.
- [ ] Frame pacing uses only the timeline semaphore plus WSI binary semaphores; no `VkFence` anywhere.
- [ ] `frames_in_flight` = 1, 2, 3 all pass the 300-frame test; default is 2.
- [ ] The frame loop contains no `vkDeviceWaitIdle`.
- [ ] The acquire -> render -> present barrier/semaphore sequence is synchronization-validation clean.
- [ ] Every barrier follows section 6.3; none uses ALL_COMMANDS or MEMORY_* masks.
- [ ] Objects destroyed through `destroy_*` are freed only after `completed >= doom_tag`; handles are stale immediately.
- [ ] Encoder misuse and stale handles surface as `Err` from `finish()` and never as UB or panics.
- [ ] No `VkRenderPass` or `VkFramebuffer` is created anywhere (grep check in CI).
- [ ] Shutdown drains the deferred queue and reports zero leaks for the triangle.
- [ ] Zero steady-state allocations in the frame path (allocation-counting test).
- [ ] `clippy -D warnings` clean; every `unsafe` block has `// SAFETY:` covering the section 7 invariants.

## 14. Traceability (FR/NFR IDs)

| Requirement | Satisfied by |
|---|---|
| FR-VK-006 | frame slots, pools, buffers (6.1, 8.1-8.2) |
| FR-VK-007 | timeline-based frames in flight (6.1), 1..=3 slots |
| FR-RHI-011 | encoder (8.3) |
| FR-RHI-013 | barriers (6.3), semaphores (8.4), timeline |
| FR-VK-008 | M1 subset: texture barriers; buffers and ranges in M2/M4 |
| FR-RHI-006, FR-RHI-007 | 6.4, 8.6 (minimal M1 objects) |
| FR-RHI-017 | labels (8.3), object names |
| FR-VK-012 | `triangle_300_frames_with_swapchain` |
| FR-RES-006 | deferred destruction mechanism (6.2); full resource coverage in M2 |
| NFR-CONC-002 | explicit semaphores, queue mutex, sync validation in tests |
| NFR-PERF-015 | no idle wait in the frame loop |
| NFR-REL-004 | shutdown tests |
| ADR-0004 | sync2, dynamic rendering, timeline |
| ADR-0006 | deferred destruction keyed on timeline value |

## 15. Open questions

1. verify: signal stage for render-done and timeline in `SemaphoreSubmitInfo` (ALL_COMMANDS now); narrow after reviewing sync-validation guidance.
2. verify: `Undefined`/`Present` source stage rule (6.3) on real drivers; non-swapchain images in M2.
3. `CommandPoolResetFlags::RELEASE_RESOURCES` vs reuse: measure at M5.
4. Several command buffers per frame, secondary buffers, and a pool per recording thread: M5.
5. Should pipeline/shader-module objects move to MODULE-007 at the start of M2 (default: yes, with the `Doomed` variants kept)?
6. Depth attachments and `DepthAttachment` handling arrive with texture creation in M2.
7. (verified 2026-11-02) ash 0.38 names for `queue_submit2`, `cmd_pipeline_barrier2`, `cmd_begin_rendering`, `wait_semaphores`, `get_semaphore_counter_value`, `reset_command_pool`, and `SemaphoreSubmitInfo`/`CommandBufferSubmitInfo` builders.
8. Whether the encoder should hold the pool read lock for the whole recording (fewer lock operations) or per call (current); decide after measuring lock traffic.
