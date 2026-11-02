# LAYER-003: Rendering Hardware Interface

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1 (minimal surface); grows in M2 |
| Crates | rhi |
| Traces to | MASTER-000 §4.2, §5.2 rules 2, 3, 5, 7, §8.3, §10, §22.4, §22.5; ADR-0004, ADR-0005; ADR-0006 |
| Last updated | 2026-11-02 |

## 1. Purpose

Backend-agnostic description of the GPU: descriptor structs, enums, typed handles and the `Backend`/`Device`/`CommandEncoder` traits. Higher layers are generic over these traits (ADR-0005). It contains no Vulkan code.

## 2. Scope

| In (M1) | In (M2+, grown from real needs) | Out |
|---|---|---|
| Instance/surface/device creation descriptors, `DeviceInfo` | Buffers, images, samplers, upload/staging API | Vulkan types, `ash` |
| Swapchain create/acquire/present | Bind groups / descriptor layouts | Render graph, passes (LAYER-006) |
| Shader module from SPIR-V words | Compute pipelines | Shader compilation (LAYER-008) |
| Graphics pipeline (dynamic rendering, no render pass) | Timestamp queries, queue families beyond one | Scene, assets, windowing |
| Command encoder: barrier, begin/end rendering, bind, draw, labels | | |
| Frame begin/submit, deferred destruction contract | | |
| Typed handles, `RhiError` | | |

Per MASTER-000 §22.5 the trait surface stays minimal; every method must be justified by M1 (or the milestone that adds it).

## 3. Crates

rhi (Layer 2). Depends on foundation, `raw-window-handle` 0.6, `thiserror`, `bitflags`. Not on `ash`, `winit`, or rhi-vulkan.

## 4. Responsibilities

- Define handles, formats, usage flags, resource states and descriptor structs.
- Define the `Backend`, `Instance`, `Device`, `CommandEncoder` traits and the frame/submission contract.
- Define `RhiError` and the swapchain-status outcomes.
- Define the instrumentation hooks (debug labels in M1; GPU scope samples and `MemorySnapshot` pull API when needed, see LAYER README §2.4).
- Define the deferred-destruction contract (behaviour is implemented by the backend).

## 5. Non-responsibilities

- Any implementation of GPU work; resource pools and allocators (backend).
- Frame graph / pass scheduling, material or mesh concepts.
- Shader source handling.

## 6. Dependencies

| Allowed | Forbidden |
|---|---|
| foundation, raw-window-handle, thiserror, bitflags | `ash`, `gpu-allocator`, `winit`, rhi-vulkan, platform, any layer 3+ crate |

Rule 2: the RHI does not depend on gameplay or scene systems. `platform` is not a dependency: windows are passed as generic `HasWindowHandle + HasDisplayHandle`.

## 7. Key concepts and data flow

```
 app (example)                    rhi (traits)                 rhi-vulkan
 -------------                    ------------------                 ----------------
 VulkanBackend::create_instance(&InstanceDesc{display,..}) -> Instance
 instance.create_surface(Arc<Window>)                       -> Surface   (owns the Arc, so window outlives VkSurfaceKHR)
 instance.create_device(&DeviceDesc, Some(&surface))        -> Device    (present support checked against that surface; None = headless)
 device.create_swapchain(&surface, &SwapchainDesc)          -> SwapchainHandle   (pool slot in backend)
 loop:
   device.begin_frame()         -> FrameToken   (waits slot's timeline value, runs deferred destroys)
   device.acquire(sc)           -> Acquire::Image(SwapchainImage) | Acquire::OutOfDate
   let mut cmd = device.begin_commands(&frame)
   cmd.texture_barrier(..); cmd.begin_rendering(..); cmd.bind_pipeline(..); cmd.draw(..); cmd.end_rendering(); cmd.texture_barrier(..)
   device.submit(cmd.finish()?, &image)    -> signals frame timeline value
   device.present(sc, image)    -> Present::Ok | Present::OutOfDate   (suboptimal => OutOfDate-like: recreate)
   on OutOfDate: device.resize_swapchain(sc, w, h)
```

The Device must not outlive the Instance: the backend Device (and every Surface) holds an `Arc` to the shared instance internals, so the native instance is destroyed only after the last of them is dropped (MODULE-001, MODULE-002).

Higher layers hold `D: Device` as a generic parameter; the only `dyn` allowed is off hot paths (e.g. error sources).

### 7.1 Frames in flight

`pub const DEFAULT_FRAMES_IN_FLIGHT: u32 = 2;` `DeviceDesc::frames_in_flight` overrides it (valid 1..=3; verify upper bound is sensible once measured). Per frame slot the backend keeps a command pool, and the frame completes when the graphics timeline semaphore reaches the value signalled by that slot's submit.

### 7.2 Deferred destruction contract

`destroy_*` never destroys immediately. The backend queues the object with the timeline value of the last submission that may reference it (conservatively: the most recent submitted value, or the current frame's value if unsubmitted) and destroys it once the GPU has passed that value. Handles become stale immediately at `destroy_*` (generation bump), even though the native object lives on.

### 7.3 Resource states (M1 barriers)

`TextureState { Undefined, ColorAttachment, Present, ShaderRead, TransferDst, TransferSrc }`. The backend maps pairs of states to `VkImageLayout` + sync2 stage/access masks. A full automatic state tracker is out of scope until the render graph (LAYER-006).

## 8. Public interface sketch

```rust
// ---- handles (foundation Handle<T>, ADR-0006) ----
pub struct BufferTag; pub struct TextureTag; pub struct SamplerTag; pub struct ShaderModuleTag;
pub struct PipelineTag; pub struct SwapchainTag;
pub type BufferHandle = Handle<BufferTag>;       // declared in M1 for stability; creation arrives in M2
pub type TextureHandle = Handle<TextureTag>;
pub type ShaderModuleHandle = Handle<ShaderModuleTag>;
pub type PipelineHandle = Handle<PipelineTag>;   // graphics pipeline in M1
pub type SwapchainHandle = Handle<SwapchainTag>;

// ---- descriptors ----
pub enum ValidationMode {
    Auto, // debug builds: enable if the validation layer is present, warn if missing; release builds: off
    On,   // error (RhiError::Unsupported) if the layer is missing; tests use this
    Off,
}
pub struct InstanceDesc {
    pub app_name: String,
    pub validation: ValidationMode,
    pub sync_validation: bool,                         // default from env ANVIL_SYNC_VALIDATION=1; tests set true
    pub display: Option<RawDisplayHandle>,             // None = headless (no surface extensions)
}
pub struct DeviceDesc {
    pub preferred_gpu: Option<String>,                 // substring of device name; env ANVIL_GPU overrides
    pub frames_in_flight: u32,                         // default DEFAULT_FRAMES_IN_FLIGHT
}
pub struct DeviceInfo { pub name: String, pub vendor_id: u32, pub kind: GpuKind, pub api_version: (u32,u32,u32), pub driver: String }
pub struct SwapchainDesc { pub width: u32, pub height: u32, pub vsync: bool /* true = FIFO */, pub preferred_format: Format }
pub struct ShaderModuleDesc<'a> { pub spirv: &'a [u32], pub label: Option<&'a str> }
pub struct GraphicsPipelineDesc<'a> {
    pub label: Option<&'a str>,
    pub vertex: ShaderStageDesc<'a>,  pub fragment: ShaderStageDesc<'a>,   // { module, entry: &str }
    pub color_formats: &'a [Format],  pub depth_format: Option<Format>,
    pub topology: PrimitiveTopology,  pub cull: CullMode, pub front_face: FrontFace,
    pub blend: &'a [BlendState],      // one per color target; default = opaque write-all
    // M1: no vertex buffers, no descriptor sets, no push constants (triangle uses gl_VertexIndex).
}
pub struct RenderingDesc<'a> { pub color: &'a [ColorAttachment], pub depth: Option<DepthAttachment>, pub width: u32, pub height: u32 }
pub struct ColorAttachment { pub texture: TextureHandle, pub load: LoadOp, pub store: StoreOp, pub clear: [f32; 4] }
pub struct SwapchainImage { pub swapchain: SwapchainHandle, pub texture: TextureHandle, pub index: u32, pub extent: (u32, u32), pub format: Format }
pub enum Format { Bgra8Srgb, Bgra8Unorm, Rgba8Srgb, Rgba8Unorm, D32Sfloat /* extended in M2 */ }

// ---- traits (ADR-0005: static generics, associated types) ----
pub trait Backend: 'static {
    type Instance: Instance<Surface = Self::Surface, Device = Self::Device>;
    type Surface: Send + Sync + 'static;
    type Device: Device<Surface = Self::Surface>;
    fn create_instance(desc: &InstanceDesc) -> Result<Self::Instance, RhiError>;
}

pub trait Instance: Sized + Send + Sync {
    type Surface;
    type Device;
    /// The returned Surface owns `window`, so the window outlives the native surface.
    fn create_surface<W>(&self, window: Arc<W>) -> Result<Self::Surface, RhiError>
        where W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static;
    /// `compatible_surface`: the selected queue family must be able to present to it. `None` = headless.
    fn create_device(&self, desc: &DeviceDesc, compatible_surface: Option<&Self::Surface>) -> Result<Self::Device, RhiError>;
}

pub trait Device: Sized {
    type Surface;
    type Commands: CommandEncoder;
    fn info(&self) -> &DeviceInfo;

    fn create_swapchain(&self, surface: &Self::Surface, desc: &SwapchainDesc) -> Result<SwapchainHandle, RhiError>;
    fn resize_swapchain(&self, sc: SwapchainHandle, width: u32, height: u32) -> Result<(), RhiError>;
    fn swapchain_format(&self, sc: SwapchainHandle) -> Result<Format, RhiError>;
    fn destroy_swapchain(&self, sc: SwapchainHandle);

    fn create_shader_module(&self, desc: &ShaderModuleDesc) -> Result<ShaderModuleHandle, RhiError>;
    fn create_graphics_pipeline(&self, desc: &GraphicsPipelineDesc) -> Result<PipelineHandle, RhiError>;
    fn destroy_shader_module(&self, h: ShaderModuleHandle);
    fn destroy_pipeline(&self, h: PipelineHandle);

    fn begin_frame(&mut self) -> Result<FrameToken, RhiError>;
    fn acquire(&mut self, sc: SwapchainHandle) -> Result<Acquire, RhiError>;
    /// Returns an owned encoder (no borrow of the device while recording).
    fn begin_commands(&mut self, frame: &FrameToken) -> Result<Self::Commands, RhiError>;
    /// M1: `image` identifies the swapchain and image whose acquire/render-done semaphores are wired. A general SubmitDesc arrives in M2 only when needed.
    fn submit(&mut self, cmds: <Self::Commands as CommandEncoder>::Finished, image: &SwapchainImage) -> Result<(), RhiError>;
    fn present(&mut self, sc: SwapchainHandle, image: SwapchainImage) -> Result<PresentStatus, RhiError>;
    fn wait_idle(&self) -> Result<(), RhiError>;
}

pub trait CommandEncoder {
    type Finished;
    fn texture_barrier(&mut self, t: TextureHandle, from: TextureState, to: TextureState);
    fn begin_rendering(&mut self, desc: &RenderingDesc);
    fn end_rendering(&mut self);
    fn bind_pipeline(&mut self, p: PipelineHandle);
    fn set_viewport(&mut self, v: Viewport);
    fn set_scissor(&mut self, r: Rect2D);
    fn draw(&mut self, vertex_count: u32, instance_count: u32, first_vertex: u32, first_instance: u32);
    fn begin_label(&mut self, name: &str, color: [f32; 4]);
    fn end_label(&mut self);
    fn finish(self) -> Result<Self::Finished, RhiError>;     // first recorded error (e.g. stale handle) surfaces here
}

pub enum Acquire { Image(SwapchainImage), OutOfDate, Minimized }   // Minimized: extent is 0, skip the frame
pub enum PresentStatus { Ok, Recreate }                            // Recreate = OUT_OF_DATE or SUBOPTIMAL
pub struct FrameToken { /* frame slot index, timeline value; opaque */ }
```

Notes:
- Resource creation takes `&self` (backend pools use interior mutability); frame operations take `&mut self` (single owner on the render thread in M1).
- The `Finished` associated type lets the backend return a raw command buffer without exposing it; it is consumed by `submit`.
- Command methods are infallible; errors (stale handle, invalid state) are recorded and returned from `finish()`. Rationale: keeps hot-path signatures free of `Result`.
- Coordinates: M1 uses native Vulkan conventions (Y down in clip space, positive viewport height, no flip). The engine-wide convention is decided with math (M3).

## 9. Ownership and lifetime

- Backend pools own all GPU objects (MASTER-000 §10.2); the public API only ever shows handles. Dropping a handle does nothing; callers destroy explicitly, or higher layers wrap handles in RAII guards that call `destroy_*` (not part of this layer in M1).
- Stale handle access returns `RhiError::StaleHandle` from fallible calls.
- `Device` drop: waits idle, drains the deferred queue, then destroys remaining pooled objects, logging each leak at `warn` (leak = handle never destroyed).
- `Surface` owns the `Arc<W>` and the native surface; each swapchain keeps its `Surface` alive, so the window outlives the `VkSurfaceKHR` regardless of drop order.
- The Device holds an `Arc` to the shared instance internals and so cannot outlive the Instance; the native instance is destroyed when the last Instance/Surface/Device handle is dropped.

## 10. Threading model

M1: a `Device` is used from one thread. `Device: Send` is required (so it can move to a render thread later); `Sync` is not required. Resource creation methods take `&self` so that M2 uploads from worker threads can be added behind interior locking without a trait break; command recording in parallel (one pool per thread per frame) is deferred to M5.

## 11. Error handling

```rust
#[derive(thiserror::Error, Debug)]
pub enum RhiError {
    #[error("GPU device lost")]                          DeviceLost,           // distinct, never auto-retried
    #[error("out of device memory")]                     OutOfDeviceMemory,
    #[error("out of host memory")]                       OutOfHostMemory,
    #[error("no suitable GPU: {reason}")]                NoSuitableDevice { reason: String },
    #[error("unsupported: {what}")]                      Unsupported { what: String },
    #[error("surface lost")]                             SurfaceLost,          // fatal in M1: the app shuts down cleanly
    #[error("stale or invalid handle ({kind})")]         StaleHandle { kind: &'static str },
    #[error("invalid descriptor: {0}")]                  InvalidDescriptor(String),
    #[error("backend failure in {op}: code {code}")]     Backend { op: &'static str, code: i32 },
}
```

- Swapchain out-of-date / suboptimal is a status (`Acquire::OutOfDate`, `PresentStatus::Recreate`), not an error.
- `SurfaceLost` (VK_ERROR_SURFACE_LOST_KHR) is fatal in M1; surface recreation is deferred.
- `DeviceLost` is surfaced to the app; recovery (recreate device) is M7 (MASTER-000 §14).
- `Backend.code` is the raw native result code; no native types in the public API.

## 12. Diagnostics and observability

- Debug labels via `begin_label`/`end_label` (no-op when the backend lacks support).
- GPU scope samples (`GpuScopeSample`) and `MemorySnapshot` are pulled by layer 5 through `Device` methods added when the profiler is specified (M5); the sample types already exist in foundation.
- `tracing` events for device selection, swapchain recreation, leaked handles.

## 13. Testing strategy

- Unit: descriptor validation helpers, format properties, error mapping tables (in the backend).
- A `NullBackend` test double in `rhi` dev-dependencies (feature `testing`) implementing the traits with counters, to test higher layers without a GPU (small; justified from M2 on, optional in M1).
- Compile-fail/pass tests: higher-layer code compiles against `impl Device`; no `ash` in `cargo tree -p rhi`.
- GPU integration tests live in rhi-vulkan (LAYER-004).

## 14. Acceptance criteria

- [ ] `rhi` has no dependency on `ash`, `winit` or rhi-vulkan (`cargo tree`).
- [ ] The traits above are sufficient to write the triangle example generic over `D: Device`.
- [ ] Every public item documented; no `dyn` on the per-frame path.
- [ ] `RhiError` covers all backend failure modes by the mapping table in LAYER-004 §11.
- [ ] Handle tags are distinct types (`BufferHandle` cannot be passed as `TextureHandle`; compile-fail test).

## 15. Open questions

1. Pipeline creation in M1 passes explicit formats (`color_formats`) instead of a render-pass object (dynamic rendering). Confirm that attaching formats to the pipeline desc is acceptable for the render graph.
2. Where do push constants and descriptor layouts first appear: M2 with the first textured example?

### Resolved (lead review 2026-11-02)

1. Surface before device: `Backend` separates `Instance` and `Device`; `Instance::create_surface` returns a `Surface` that owns the `Arc<W>`; `Instance::create_device(&DeviceDesc, Option<&Surface>)` checks present support against that surface (None = headless); `Device::create_swapchain(&Surface, &SwapchainDesc)`. `InstanceDesc` carries `app_name`, validation mode and `display`; `DeviceDesc` no longer carries `display`. The Device holds an `Arc` to the shared instance internals so it cannot outlive the Instance.
2. `begin_commands` returns an owned encoder. Resource creation takes `&self`, frame operations take `&mut self`.
3. `submit(cmds, &SwapchainImage)` is sufficient for M1; a general `SubmitDesc` arrives in M2 only when needed (supersedes "M5").
4. Clip-space and viewport convention is deferred to M3 (math). M1 uses native Vulkan conventions with no flip.
5. `ValidationMode`: Auto (debug builds: enable if present, warn if missing), On (error if missing; tests use this), Off.
6. `VK_ERROR_SURFACE_LOST_KHR` is fatal in M1 (`RhiError::SurfaceLost`); surface recreation is deferred.
7. `SwapchainImage` gained a `swapchain: SwapchainHandle` field so `submit` can find the per-image semaphores.

## 16. Related modules

None dedicated; see MODULE-005 and MODULE-006 (LAYER-007) for the M2 resource API, and MODULE-004-command-manager (LAYER-004) for the encoder implementation.
