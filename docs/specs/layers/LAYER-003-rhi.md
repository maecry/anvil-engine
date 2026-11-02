# LAYER-003: Rendering Hardware Interface

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1 (minimal surface); M2 API in §17; M3 additions in §18 |
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
    #[error("stale or invalid {kind} handle (index {index}, generation {generation})")] StaleHandle { kind: &'static str, index: u32, generation: u32 },
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
2. Resolved by ADR-0010: push constants and the global descriptor layout appear in M2 (§17).

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

## 17. M2 API (ADR-0010)

Everything here is **M2** and extends the M1 items in §8 without renaming any of them. Doc comments are mandatory on every item (rustdoc `missing_docs` is on). Resource creation and `upload_*`/`write_*`/`read_*` take `&self`; frame operations stay `&mut self`.

### 17.1 Types (crate `rhi`)

```rust
/// Buffer usage flags. A `u32` newtype with consts instead of the `bitflags` crate: M2 needs
/// six flags and `|`, `contains`; no extra dependency (ADR-0002 keeps `rhi` dependency-light).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BufferUsage(pub u32);
impl BufferUsage {
    pub const NONE: Self = Self(0);
    /// Index buffer (`bind_index_buffer`).
    pub const INDEX: Self = Self(1 << 0);
    /// Read/written by shaders through its device address or as a storage buffer.
    pub const STORAGE: Self = Self(1 << 1);
    /// Uniform buffer.
    pub const UNIFORM: Self = Self(1 << 2);
    /// Source of a copy.
    pub const TRANSFER_SRC: Self = Self(1 << 3);
    /// Destination of a copy (required by `upload_buffer` and `copy_buffer_to_buffer` dst).
    pub const TRANSFER_DST: Self = Self(1 << 4);
    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub const fn is_empty(self) -> bool { self.0 == 0 }
}
impl std::ops::BitOr for BufferUsage { type Output = Self; fn bitor(self, r: Self) -> Self { Self(self.0 | r.0) } }

/// Where a buffer lives (maps to gpu-allocator `MemoryLocation`, ADR-0010 §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Memory {
    /// Device-local, not host-visible. Filled with `upload_buffer`.
    GpuOnly,
    /// Host-visible, persistently mapped, CPU-write (`write_buffer`).
    Upload,
    /// Host-visible, persistently mapped, CPU-read (`read_buffer`).
    Readback,
}

/// Parameters for [`Device::create_buffer`].
#[derive(Clone, Copy, Debug)]
pub struct BufferDesc<'a> {
    /// Size in bytes, > 0.
    pub size: u64,
    /// Usage; must not be empty. The backend always adds device-address usage (ADR-0010 §2).
    pub usage: BufferUsage,
    /// Memory placement.
    pub memory: Memory,
    /// Debug label (ADR-0009 §4).
    pub label: Option<&'a str>,
}

/// Texture usage flags (same newtype pattern as [`BufferUsage`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureUsage(pub u32);
impl TextureUsage {
    pub const NONE: Self = Self(0);
    /// Sampled in shaders through the bindless table; gets a texture index.
    pub const SAMPLED: Self = Self(1 << 0);
    /// Color render target (color formats only).
    pub const COLOR_ATTACHMENT: Self = Self(1 << 1);
    /// Depth render target (depth formats only).
    pub const DEPTH_ATTACHMENT: Self = Self(1 << 2);
    /// Source of a copy (`copy_texture_to_buffer`).
    pub const TRANSFER_SRC: Self = Self(1 << 3);
    /// Destination of a copy (required by `upload_texture`).
    pub const TRANSFER_DST: Self = Self(1 << 4);
    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub const fn is_empty(self) -> bool { self.0 == 0 }
}
impl std::ops::BitOr for TextureUsage { type Output = Self; fn bitor(self, r: Self) -> Self { Self(self.0 | r.0) } }

/// Parameters for [`Device::create_texture`]. M2: 2D, one mip, one layer, optimal tiling.
#[derive(Clone, Copy, Debug)]
pub struct TextureDesc<'a> {
    /// Width in texels, > 0.
    pub width: u32,
    /// Height in texels, > 0.
    pub height: u32,
    /// Texel format.
    pub format: Format,
    /// Usage; must not be empty.
    pub usage: TextureUsage,
    /// Debug label.
    pub label: Option<&'a str>,
}

/// Texture filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter { Nearest, Linear }

/// Texture coordinate addressing outside [0, 1].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressMode { Repeat, MirroredRepeat, ClampToEdge }

/// Parameters for [`Device::create_sampler`]. No anisotropy, compare or mip filtering in M2
/// (one mip level; LOD is clamped to 0).
#[derive(Clone, Copy, Debug)]
pub struct SamplerDesc<'a> {
    /// Magnification filter.
    pub mag_filter: Filter,
    /// Minification filter.
    pub min_filter: Filter,
    /// U addressing.
    pub address_u: AddressMode,
    /// V addressing.
    pub address_v: AddressMode,
    /// W addressing.
    pub address_w: AddressMode,
    /// Debug label.
    pub label: Option<&'a str>,
}

/// Index element type for [`CommandEncoder::bind_index_buffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndexFormat { U16, U32 }

/// Size of the push-constant block shared by every pipeline (ADR-0010 §3).
pub const PUSH_CONSTANT_BYTES: u32 = 128;
```

`Format` gains `R8Unorm`, `Rg8Unorm`, `Rgba16Float` (`D32Sfloat` already exists from M1; ADR-0010 §12 lists it only for completeness):

```rust
impl Format {
    /// Bytes per texel for tightly packed uploads: R8 1, Rg8 2, 8-bit RGBA/BGRA 4, Rgba16Float 8, D32Sfloat 4.
    pub fn bytes_per_texel(self) -> u32;
}
```
`is_srgb` is unchanged (false for the new variants); `is_depth` stays true only for `D32Sfloat`.

`TextureState` gains `DepthAttachment` (written/tested as a depth attachment). `DepthRead` and `General` are not added in M2 (nothing needs them). State is whole-texture and advisory (ADR-0010 §9); the backend only warns when `from` disagrees with its tracked state.

### 17.2 `Device` additions

```rust
/// Creates a buffer. Errors: InvalidDescriptor (size 0, empty usage), OutOfDeviceMemory,
/// OutOfHostMemory, Backend.
fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError>;
/// Destroys a buffer (deferred). Stale handles are ignored with a `warn`.
fn destroy_buffer(&self, h: BufferHandle);
/// GPU virtual address (BDA) for use in push constants / buffer references. Stable for the buffer's life.
fn buffer_address(&self, h: BufferHandle) -> Result<u64, RhiError>;
/// Copies `data` into a host-visible (`Upload`) buffer now. Errors: StaleHandle,
/// InvalidDescriptor (not `Memory::Upload`, or out of range).
fn write_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;
/// Copies from a `Readback` buffer into `out`. The caller guarantees the GPU finished writing
/// it (e.g. after `wait_idle`). Errors as `write_buffer`.
fn read_buffer(&self, h: BufferHandle, offset: u64, out: &mut [u8]) -> Result<(), RhiError>;
/// Uploads into any buffer with `TRANSFER_DST` via staging + the frame's upload command buffer.
/// Callable at any time: the copy is queued and recorded at the next submit (lead amendment, see MODULE-005 §6.3).
fn upload_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;

/// Creates a 2D texture with one default view. If `usage` has `SAMPLED` the bindless slot is
/// written now. Errors: InvalidDescriptor, Unsupported (format/usage, bindless capacity), OOM.
fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError>;
/// Destroys a texture (deferred; its bindless slot is reusable only after retirement).
/// Swapchain-owned textures are ignored with a `warn`.
fn destroy_texture(&self, h: TextureHandle);
/// Bindless slot of a `SAMPLED` texture (the handle's pool index). Errors: StaleHandle,
/// InvalidDescriptor (not SAMPLED, swapchain-owned).
fn texture_index(&self, h: TextureHandle) -> Result<u32, RhiError>;
/// Uploads mip 0 / layer 0, tightly packed (`width*height*bytes_per_texel` bytes), and leaves
/// the texture in `ShaderRead`. Needs `TRANSFER_DST`; tracked state `Undefined`
/// or `ShaderRead`.
fn upload_texture(&self, h: TextureHandle, data: &[u8]) -> Result<(), RhiError>;

/// Creates a sampler and writes its bindless slot.
fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerHandle, RhiError>;
/// Destroys a sampler (deferred, slot reuse as for textures).
fn destroy_sampler(&self, h: SamplerHandle);
/// Bindless slot of a sampler (the handle's pool index).
fn sampler_index(&self, h: SamplerHandle) -> Result<u32, RhiError>;
```

`Device::submit` is unchanged. Headless tests submit through the backend-private `VulkanDevice::frames.submit_unwindowed` as the M1 tests do (§15 resolved 3: no general `SubmitDesc` yet, not needed). No `memory_snapshot()` in M2: memory totals are part of `diagnostics()` (ADR-0010 Consequences).

### 17.3 `CommandEncoder` additions

```rust
/// Writes `data` at `offset` of the 128-byte push-constant block, visible to all graphics
/// stages. offset and len must be multiples of 4 and offset+len <= PUSH_CONSTANT_BYTES.
/// Valid inside or outside a rendering scope.
fn push_constants(&mut self, offset: u32, data: &[u8]);
/// Binds an index buffer (`INDEX` usage); `offset` must be a multiple of the index size.
fn bind_index_buffer(&mut self, b: BufferHandle, offset: u64, format: IndexFormat);
/// Indexed draw; only inside a rendering scope with a bound pipeline and index buffer.
fn draw_indexed(&mut self, index_count: u32, instance_count: u32, first_index: u32,
                vertex_offset: i32, first_instance: u32);
/// Copies `size` bytes between buffers (outside rendering). src needs TRANSFER_SRC, dst TRANSFER_DST.
fn copy_buffer_to_buffer(&mut self, src: BufferHandle, src_offset: u64,
                         dst: BufferHandle, dst_offset: u64, size: u64);
/// Copies mip 0 of a texture (state `TransferSrc`, usage TRANSFER_SRC) into a buffer
/// (TRANSFER_DST), tightly packed, outside rendering. Justified by readback verification tests.
fn copy_texture_to_buffer(&mut self, t: TextureHandle, buffer: BufferHandle, buffer_offset: u64);
```
All are infallible; misuse is recorded and returned from `finish()` (§8). `bind_pipeline` additionally binds the global descriptor set (set 0) the first time it is called in a command buffer (MODULE-007).

### 17.4 Pipelines and rendering with depth

Fields already exist (`GraphicsPipelineDesc.depth_format`, `RenderingDesc.depth`, `DepthAttachment`). M2 behavior:
- `depth_format: Some(Format::D32Sfloat)` (the only M2 depth format) enables depth test and depth write with compare `LESS`; no stencil. A non-depth format is `InvalidDescriptor`. Finer control (`DepthState`) is added when a pass needs it (M3), not now, so the triangle example keeps compiling unchanged.
- `RenderingDesc.depth` requires a texture with `DEPTH_ATTACHMENT` usage in state `DepthAttachment` (recorded error otherwise). The pipeline's `depth_format` must equal the rendering's depth format (also `None` == `None`), checked at draw like color formats; mismatch is `InvalidDescriptor` at `finish()`.
- Vulkan: `depth_attachment_format` in `PipelineRenderingCreateInfo`; layout `DEPTH_ATTACHMENT_OPTIMAL`; the `texture_barrier` image aspect comes from the texture's format (depth -> `DEPTH`), no longer fixed to `COLOR`. State pair mapping (backend, with sync2 masks): `Undefined -> DepthAttachment` (src NONE, dst `EARLY|LATE_FRAGMENT_TESTS`, `DEPTH_STENCIL_ATTACHMENT_READ|WRITE`); `DepthAttachment <-> ShaderRead` was `Unsupported` in M2 and is allowed from M3 (§18). Note: `depth_format` and "depth means LESS" in this subsection are superseded by `DepthState` (§18.1).
- The M1 "depth attachments (M2)" `Unsupported` in `begin_rendering` is removed.
- `GraphicsPipelineDesc` doc "no descriptor sets, no push constants" is replaced by: every pipeline uses the global layout (set 0 bindless + 128 B push constants, MODULE-007), no vertex input state.

### 17.5 `DeviceDiagnostics` additions (ADR-0009, ADR-0010 Consequences)

```rust
pub struct LiveObjects { /* + */ pub buffers: usize, pub samplers: usize }  // textures already counted; staging buffers are NOT counted (internal)
pub struct DeviceDiagnostics { /* + */ pub memory: MemoryDiagnostics, pub bindless: BindlessDiagnostics }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryDiagnostics {
    /// Bytes handed out to live and not-yet-retired allocations (sum of allocation sizes).
    pub allocated_bytes: u64,
    /// Bytes the allocator holds from the driver (memory blocks incl. free regions).
    pub reserved_bytes: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindlessDiagnostics {
    pub image_capacity: u32,
    /// 1 + highest texture slot ever written (0 = none).
    pub image_high_water: u32,
    pub sampler_capacity: u32,
    pub sampler_high_water: u32,
}
```
`rhi`'s test `MockDevice::diagnostics` and any struct-literal use must add the new fields (`..Default::default()` for the new structs).

### 17.6 Errors (no new variants)

| Case | Error |
|---|---|
| zero size / extent / empty usage; usage-op mismatch (write to non-`Upload`, upload without `TRANSFER_DST`, out-of-range offset+len, bad alignment); `texture_index` on non-`SAMPLED`; non-depth `depth_format` | `InvalidDescriptor(msg)` naming the field |
| format/usage not supported by the device; sampled depth; bindless capacity exceeded; anisotropy/compare (not in desc anyway) | `Unsupported{what}` |
| destroyed or never-valid handle | `StaleHandle{kind: "buffer"/"texture"/"sampler", index, generation}` |
| allocator / `vkCreate*` memory failures | `OutOfDeviceMemory` / `OutOfHostMemory`; other allocator errors `Backend{op:"gpu_allocator"}` (MODULE-002 §10) |

### 17.7 Example: `examples/textured-quad` (M2)

`examples/triangle` is **unchanged** (it keeps compiling against the extended `rhi`; hardcoded triangle, `ANVIL_EXIT_AFTER_FRAMES` verdict as in M1). New crate `examples/textured-quad` (workspace member, same structure: `build.rs` compiling `shaders/quad.{vert,frag}` with shaderc into `OUT_DIR`, no committed `.spv`; compile with target env Vulkan 1.3 / SPIR-V 1.5 via `CompileOptions::set_target_env` — verify shaderc API).

| Item | Spec |
|---|---|
| Geometry | 4 vertices `{pos: [f32;2], uv: [f32;2]}` (16 B, `repr(C)`) pulled in the vertex shader through `buffer_reference`; 6 `u16` indices `[0,1,2, 2,1,3]`; `GpuOnly` buffers (`STORAGE\|TRANSFER_DST`, `INDEX\|TRANSFER_DST`) filled with `upload_buffer` |
| Texture | 64x64 `Rgba8Unorm`, 8-texel checkerboard generated on the CPU (two colors), usage `SAMPLED\|TRANSFER_DST`, `upload_texture` |
| Sampler | `Nearest` mag/min, `ClampToEdge`; slot via `sampler_index` |
| Push constants | `#[repr(C)] struct Pc { vertex_buffer: u64, texture: u32, sampler: u32 }` (16 B, `push_constants(0, bytes)`); the GLSL block has the identical layout in both stages |
| Upload timing | `upload_*` is callable any time; the example uploads in `on_start` right after creating resources |
| Pipeline | no vertex input, `depth_format: None`, `TriangleList`, cull `None`, one color target (swapchain format) |
| Frame | `begin_frame`, `acquire`, `begin_commands`, barrier `Undefined->ColorAttachment`, `begin_rendering`, `bind_pipeline`, viewport, scissor, `push_constants`, `bind_index_buffer`, `draw_indexed(6,1,0,0,0)`, `end_rendering`, barrier `->Present`, `finish`, `submit`, `present` |
| Shaders | `#extension GL_EXT_buffer_reference : require`, `GL_EXT_nonuniform_qualifier : require`; `layout(set=0,binding=0) uniform texture2D textures[]; layout(set=0,binding=1) uniform sampler samplers[];` fragment: `texture(sampler2D(textures[nonuniformEXT(pc.texture)], samplers[nonuniformEXT(pc.sampler)]), uv)` |
| Verdict | `ANVIL_EXIT_AFTER_FRAMES=n` renders n frames, logs `diagnostics()` at exit, exits 1 if `validation.errors > 0` or if before teardown `live.buffers != 2`, `live.samplers != 1` or `bindless.image_high_water == 0`; after teardown no leak `warn` is emitted. Run: `ANVIL_SYNC_VALIDATION=1 ANVIL_EXIT_AFTER_FRAMES=120 cargo run -p textured-quad` |

### 17.8 M2 amendments to M1 text

§2 table: bind groups / descriptor layouts are replaced by the bindless model (ADR-0010); §15 Q2 is resolved: push constants and the global descriptor set first appear in M2 (ADR-0010 §1-3). Tests (§13) add the `NullBackend`-free checks listed in MODULE-005/006: `BufferUsage`/`TextureUsage` ops, `Format::bytes_per_texel`, `PUSH_CONSTANT_BYTES == 128`.

### 17.x Implementation notes (lead, post-implementation)
- `InstanceDesc.gpu_av: bool` (default from `ANVIL_GPU_AV=1`): GPU-assisted validation via `VK_EXT_layer_settings` `gpuav_enable`, fallback `VALIDATION_FEATURES GPU_ASSISTED`. Mutually exclusive with sync validation (gpu_av wins).
- `IndexFormat::size()`; `rhi_vulkan::VulkanDevice::submit_unwindowed` is public for headless tests.
- Transfer-only textures have no default view (VUID-VkImageViewCreateInfo-image-04441).
- `bind_index_buffer` is valid only inside rendering; bound state resets at begin/end rendering.

## 18. M3 additions (ADR-0011 §9)

Minimum RHI changes so the M3 renderer can use reverse-Z, HDR targets and (M3b) shadow maps.

### 18.1 Depth state
`GraphicsPipelineDesc.depth_format: Option<Format>` is replaced by `depth: Option<DepthState>` (the M2 "depth means LESS" rule is gone):

```rust
pub struct DepthState { pub format: Format, pub compare: CompareOp, pub write: bool, pub bias: Option<DepthBias> }
pub struct DepthBias { pub constant: f32, pub slope: f32, pub clamp: f32 }   // rasterizer depth bias
pub enum CompareOp { Never, Less, Equal, LessOrEqual, Greater, GreaterOrEqual, Always }
```
`format` must still be `D32Sfloat` (else `InvalidDescriptor`); `None` disables test and write. The pipeline's depth format must equal the rendering's depth format, as in §17.4. The renderer uses `GreaterOrEqual` with a depth clear of 0 (reverse-Z, ADR-0011 §1).

### 18.2 Sampled depth and comparison samplers
- `SamplerDesc.compare: Option<CompareOp>` enables a comparison sampler (shadow PCF, M3b). Existing callers pass `None`.
- `D32Sfloat` textures may now carry `TextureUsage::SAMPLED`; they get a bindless slot with a depth-aspect view (the M2 `Unsupported` is removed).
- `texture_barrier` allows `DepthAttachment <-> ShaderRead` (layouts `DEPTH_ATTACHMENT_OPTIMAL` <-> `SHADER_READ_ONLY_OPTIMAL`; the aspect comes from the texture format). Shaders declare a `samplerShadow` over the same binding-1 sampler array.

### 18.3 Formats and swapchain
- `Format::Rg16Float` (`R16G16_SFLOAT`, 4 bytes per texel) for the BRDF LUT (M3c).
- Swapchain images always get `TRANSFER_SRC` usage, and `copy_texture_to_buffer` accepts swapchain textures, for `ANVIL_SCREENSHOT` (ADR-0011 §11). shortcut: surface support for `TRANSFER_SRC` is assumed, not checked against the surface capabilities.

### 18.5 Depth-only rendering (M3b)
`GraphicsPipelineDesc.color_formats` and `RenderingDesc.color` may be empty when a depth attachment is present (shadow pass); a scope with neither is `InvalidDescriptor`. A depth-only pipeline passes an empty `blend` slice. The fragment stage is still required.

### 18.4 Tests
Unit: `depth_state_mapping`, `sampler_info_mapping` (compare), depth/shader barrier pair, `Rg16Float` mapping. GPU (`resources.rs`): `sampled_depth_and_compare_sampler` (sampled depth texture, compare sampler, `DepthAttachment <-> ShaderRead` barriers, zero validation messages).

## 19. M4b additions (ADR-0012 §12)

The only RHI-side change of M4b: `rhi-vulkan` enables the Vulkan 1.3 feature `shaderDemoteToHelperInvocation` when creating the device. Fragment-shader `discard` (alpha-tested materials) compiles to `OpDemoteToHelperInvocation` for SPIR-V 1.6, and validation rejects the module without the feature. It is mandatory in Vulkan 1.3, so no capability check was added. No public RHI API changed: `Rg16Float`, multiple color attachments of different formats, depth compare ops and premultiplied blend factors were already expressible.

## 20. M5a additions (ADR-0013 §2, §4)

### 20.1 Timestamp queries
- `Device::timestamp_info() -> Option<TimestampInfo { period_ns, valid_bits }>`: `None` when the graphics queue family reports `timestampValidBits == 0`, `timestampComputeAndGraphics` is false, or the period is zero. `period_ns` is `VkPhysicalDeviceLimits::timestampPeriod`.
- `CommandEncoder::write_timestamp(index)`: writes the GPU timestamp (after all previous commands) into query `index` of the frame slot's pool. `index < MAX_TIMESTAMPS` (128); index 0 starts a frame's sequence, further indices ascend with no gaps. No-op without support. Out of range records an `InvalidDescriptor` error returned from `finish`.
- `Device::read_timestamps(slot) -> Result<Vec<u64>>`: ticks (masked to `valid_bits`) of the last submitted frame that used frame slot `slot`, indices `0..n`. Valid after `begin_frame` returned that slot and before the new frame is submitted, i.e. `frames_in_flight` frames after the frames were recorded. Empty when none were written, the frame was never submitted or timestamps are unsupported. Convert with `delta_ticks * period_ns` (`TimestampInfo`); subtract with wrapping arithmetic when `valid_bits < 64`.
- One query pool per frame slot (so a frame in flight never overwrites results being read); the encoder resets the whole pool at `begin_commands`. The render graph uses these (LAYER-006 section 8a); other users must not mix indices with a graph in the same frame.
- Backend: LAYER-004 (Timestamps and pipeline cache, M5a). Tests: GPU `timestamps_round_trip`, graph `passes_barriers_and_pool_reuse_are_validation_clean`.

### 20.2 Pipeline cache
`DeviceDesc::pipeline_cache: Option<PathBuf>` selects the cache file (default: env `ANVIL_PIPELINE_CACHE`, else `target/anvil-pipeline-cache.bin`). The backend loads it at device creation and saves it at destruction; mismatches are ignored with a warning. See MODULE-007 section 8a.

### 20.3 M5b: compute, storage images, mips and layers, indirect draws, buffer barriers (ADR-0013 §4)
- **Compute pipelines.** `ComputePipelineDesc { label, shader: ShaderStageDesc }` and `Device::create_compute_pipeline` use the one global pipeline layout (bindless set 0 and the 128 push constant bytes; both are now visible to `ALL_GRAPHICS | COMPUTE`). The handle is a `PipelineHandle`; `destroy_pipeline` and `bind_pipeline` take both kinds. `bind_pipeline` of a compute pipeline is valid outside rendering only and binds the global set at the compute bind point (once per command buffer, tracked per bind point). `CommandEncoder::dispatch(x, y, z)` and `dispatch_indirect(buffer, offset)` (buffer needs `BufferUsage::INDIRECT`, 4-byte aligned, 12 bytes in range) require a bound compute pipeline and no active rendering.
- **Storage images.** Bindless set 0 binding 2, `STORAGE_IMAGE[]` (`GENERAL` layout), written for every texture or view with `TextureUsage::STORAGE` that is a plain 2D, single-mip image; its slot is the texture handle's index, shared with the sampled slot. `Device::storage_index(h)` returns it (error otherwise). Capacity is `min(device limit, 65536)` (`BindlessDiagnostics` unchanged); a handle index beyond it fails creation with `Unsupported` like sampled images. New `TextureState::StorageWrite` (`GENERAL`, compute stage, storage read and write). New format `Format::R32Float` (needs `STORAGE_IMAGE` format support for `STORAGE` usage). The device enables the features `shaderStorageImageArrayNonUniformIndexing`, `descriptorBindingStorageImageUpdateAfterBind`, `drawIndirectCount`, `multiDrawIndirect` and `drawIndirectFirstInstance`. shortcut: enabled without a capability check (all desktop Vulkan 1.3 drivers have them), add facts and a `Rejection` when one has to be supported.
- **Mips, layers, cube, views.** `TextureDesc` gained `mip_levels` (1..=log2(max(w, h)) + 1), `array_layers` and `cube` (6 layers, square; `TextureDesc::new_2d` builds the old shape). The default view covers the whole image (`2D`, `2D_ARRAY` or `CUBE`); only a plain 2D, one-layer default view gets bindless slots, so a mip chain is samplable (`texelFetch` with an explicit LOD) but a cube or array needs per-slice views (M5c adds `textureCube[]` bindings: shortcut). `Device::create_texture_view(parent, &TextureViewDesc { base_mip, mip_count, base_layer, layer_count, usage })` returns another `TextureHandle` that shares the parent's image: own bindless slot(s), own advisory state, usable as an attachment, as a barrier target (the barrier covers exactly the view's mip and layer range) and as a `copy_texture_to_buffer` source (the view's first mip and layer). A storage view has one mip; usage must be a subset of the parent's; no view of a view; destroy views before the parent (a view destroy defers only the `VkImageView`). `upload_texture` stays whole-texture, mip 0, one layer.
- **Buffer barriers and fills.** `BufferState { Undefined, ShaderRead, ShaderWrite, TransferSrc, TransferDst, Indirect, Vertex, Index }` and `CommandEncoder::buffer_barrier(buffer, from, to)` (whole buffer, outside rendering): the source access is the writes of `from` only, `Undefined` orders against nothing. `CommandEncoder::fill_buffer(buffer, offset, size, value)` (`TRANSFER_DST`, 4-byte multiples) resets counters. New usages `BufferUsage::INDIRECT` and `VERTEX`, `TextureUsage::STORAGE`.
- **Indirect draws.** `draw_indexed_indirect(buffer, offset, draw_count, stride)` and `draw_indexed_indirect_count(buffer, offset, count_buffer, count_offset, max_draw_count, stride)` read `VkDrawIndexedIndirectCommand` records (20 bytes; `first_instance` may be non-zero); validated for usage, alignment, range and stride (at least 20 and a multiple of 4 once more than one draw is read).
- **Tests.** GPU `crates/rhi-vulkan/tests/compute.rs` (sync validation, zero messages): a compute shader fills a buffer by `dispatch` and `dispatch_indirect` after a `fill_buffer` reset, read back through a buffer barrier and a copy; storage image writes to every mip of a 5-mip `R32Float` image through per-mip views, read back per mip, with the view validation table (bad ranges, storage view with several mips, usage superset, view of a view) and the texture description errors (too many mips, cube without 6 layers); a cube with per-face views; `draw_indexed_indirect` and `draw_indexed_indirect_count` with counts 0, 1 and 2 checked by pixels. Unit tests: the buffer barrier halves, `StorageWrite` barrier masks, indirect stride rules, the bindless set layout with binding 2.

## Observability (ADR-0009)
- `Device::diagnostics() -> DeviceDiagnostics` (plain data: `DeviceInfo`, frame / GPU-completed frame, `LiveObjects`, pending destroys, `SwapchainDiagnostics` per swapchain, `ValidationCounts`). Backends must keep it accurate; integration tests assert `validation.errors == 0`.
- `RhiError::StaleHandle` carries `kind`, `index`, `generation`; `Backend` carries `op` and the raw code.
- Backends trace every create/destroy at `debug` (handle + label), per-frame ops at `trace` inside a `frame` span, state changes at `info`.
