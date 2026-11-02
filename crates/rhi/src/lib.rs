//! Backend-agnostic Rendering Hardware Interface.
//!
//! Specification: `docs/specs/layers/LAYER-003-rhi.md`.
//!
//! Higher layers are generic over [`Backend`] / [`Device`] (ADR-0005). All GPU objects are
//! owned by the backend and referenced by generational handles (ADR-0006); dropping a handle
//! does nothing, callers destroy explicitly.
//!
//! Handle tags are distinct types:
//!
//! ```compile_fail
//! fn takes_texture(_: rhi::TextureHandle) {}
//! takes_texture(rhi::BufferHandle::NULL);
//! ```

use std::sync::Arc;

pub use foundation::Handle;
pub use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};

// ---------------------------------------------------------------- constants

/// Default number of frames the CPU may record ahead of the GPU.
pub const DEFAULT_FRAMES_IN_FLIGHT: u32 = 2;

// ---------------------------------------------------------------- handles

/// Tag for [`BufferHandle`].
pub struct BufferTag;
/// Tag for [`TextureHandle`].
pub struct TextureTag;
/// Tag for [`SamplerHandle`].
pub struct SamplerTag;
/// Tag for [`ShaderModuleHandle`].
pub struct ShaderModuleTag;
/// Tag for [`PipelineHandle`].
pub struct PipelineTag;
/// Tag for [`SwapchainHandle`].
pub struct SwapchainTag;

/// Buffer handle.
pub type BufferHandle = Handle<BufferTag>;
/// Texture handle. Swapchain images are ordinary texture handles.
pub type TextureHandle = Handle<TextureTag>;
/// Sampler handle.
pub type SamplerHandle = Handle<SamplerTag>;
/// Shader module handle.
pub type ShaderModuleHandle = Handle<ShaderModuleTag>;
/// Pipeline handle (graphics pipeline in M1).
pub type PipelineHandle = Handle<PipelineTag>;
/// Swapchain handle; the swapchain lives in a backend pool.
pub type SwapchainHandle = Handle<SwapchainTag>;

// ---------------------------------------------------------------- errors

/// Errors returned by RHI calls. Swapchain out-of-date / suboptimal is a status
/// ([`Acquire::OutOfDate`], [`PresentStatus::Recreate`]), not an error.
#[derive(thiserror::Error, Debug)]
pub enum RhiError {
    /// The device was lost. Surfaced to the app, never auto-retried.
    #[error("GPU device lost")]
    DeviceLost,
    /// Device (GPU) memory is exhausted.
    #[error("out of device memory")]
    OutOfDeviceMemory,
    /// Host memory is exhausted.
    #[error("out of host memory")]
    OutOfHostMemory,
    /// No physical device satisfies the request.
    #[error("no suitable GPU: {reason}")]
    NoSuitableDevice {
        /// Why no device qualified.
        reason: String,
    },
    /// A requested feature, layer or format is not available.
    #[error("unsupported: {what}")]
    Unsupported {
        /// What is unsupported.
        what: String,
    },
    /// The surface was lost. Fatal in M1: the app shuts down cleanly.
    #[error("surface lost")]
    SurfaceLost,
    /// A handle was destroyed (generation mismatch) or never valid.
    #[error("stale or invalid {kind} handle (index {index}, generation {generation})")]
    StaleHandle {
        /// Kind of object the handle referred to.
        kind: &'static str,
        /// Slot index of the offending handle.
        index: u32,
        /// Generation of the offending handle.
        generation: u32,
    },
    /// A descriptor or call sequence is invalid.
    #[error("invalid descriptor: {0}")]
    InvalidDescriptor(String),
    /// Unclassified backend failure; `code` is the raw native result code.
    #[error("backend failure in {op}: code {code}")]
    Backend {
        /// Name of the failing native operation.
        op: &'static str,
        /// Raw native result code (no native types in the public API).
        code: i32,
    },
}

// ---------------------------------------------------------------- plain types

/// Texture / swapchain formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// 8-bit BGRA, sRGB encoded.
    Bgra8Srgb,
    /// 8-bit BGRA, linear.
    Bgra8Unorm,
    /// 8-bit RGBA, sRGB encoded.
    Rgba8Srgb,
    /// 8-bit RGBA, linear.
    Rgba8Unorm,
    /// 8-bit single channel, linear.
    R8Unorm,
    /// 8-bit two channel, linear.
    Rg8Unorm,
    /// 16-bit float RGBA.
    Rgba16Float,
    /// 32-bit float depth.
    D32Sfloat,
}

impl Format {
    /// True for sRGB-encoded color formats.
    pub fn is_srgb(self) -> bool {
        matches!(self, Format::Bgra8Srgb | Format::Rgba8Srgb)
    }

    /// True for depth formats.
    pub fn is_depth(self) -> bool {
        matches!(self, Format::D32Sfloat)
    }

    /// Bytes per texel (tightly packed upload size).
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            Format::R8Unorm => 1,
            Format::Rg8Unorm => 2,
            Format::Bgra8Srgb
            | Format::Bgra8Unorm
            | Format::Rgba8Srgb
            | Format::Rgba8Unorm
            | Format::D32Sfloat => 4,
            Format::Rgba16Float => 8,
        }
    }
}

/// Image state used by [`CommandEncoder::texture_barrier`]. The backend maps pairs of
/// states to layouts and sync2 masks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureState {
    /// Contents undefined; only valid as a `from` state.
    Undefined,
    /// Written as a color attachment.
    ColorAttachment,
    /// Ready for presentation.
    Present,
    /// Sampled / read in shaders.
    ShaderRead,
    /// Destination of a transfer.
    TransferDst,
    /// Source of a transfer.
    TransferSrc,
    /// Written (and depth-tested) as a depth attachment.
    DepthAttachment,
}

/// Validation layer policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValidationMode {
    /// Debug builds: enable if the layer is present, warn if missing. Release builds: off.
    #[default]
    Auto,
    /// Error with [`RhiError::Unsupported`] if the layer is missing. Tests use this.
    On,
    /// Never enable validation.
    Off,
}

/// Where a buffer's memory lives (ADR-0010 §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Memory {
    /// Device-local, not host visible. Fill with [`Device::upload_buffer`].
    GpuOnly,
    /// Host-visible, persistently mapped, for CPU writes ([`Device::write_buffer`]).
    Upload,
    /// Host-visible, persistently mapped, for CPU reads ([`Device::read_buffer`]).
    Readback,
}

/// Buffer usage flags (device address is always enabled).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BufferUsage(u32);

impl BufferUsage {
    /// No usage (invalid for creation).
    pub const NONE: Self = Self(0);
    /// Index buffer.
    pub const INDEX: Self = Self(1);
    /// Storage buffer.
    pub const STORAGE: Self = Self(1 << 1);
    /// Uniform buffer.
    pub const UNIFORM: Self = Self(1 << 2);
    /// Transfer source.
    pub const TRANSFER_SRC: Self = Self(1 << 3);
    /// Transfer destination (required by [`Device::upload_buffer`]).
    pub const TRANSFER_DST: Self = Self(1 << 4);

    /// True when every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for BufferUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Parameters for [`Device::create_buffer`].
#[derive(Clone, Copy, Debug)]
pub struct BufferDesc<'a> {
    /// Debug label (ADR-0009).
    pub label: Option<&'a str>,
    /// Size in bytes, nonzero.
    pub size: u64,
    /// Usage flags, nonempty.
    pub usage: BufferUsage,
    /// Memory placement.
    pub memory: Memory,
}

/// Texture usage flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureUsage(u32);

impl TextureUsage {
    /// No usage (invalid for creation).
    pub const NONE: Self = Self(0);
    /// Sampled in shaders (gets a bindless slot).
    pub const SAMPLED: Self = Self(1);
    /// Color attachment.
    pub const COLOR_ATTACHMENT: Self = Self(1 << 1);
    /// Depth attachment.
    pub const DEPTH_ATTACHMENT: Self = Self(1 << 2);
    /// Transfer source.
    pub const TRANSFER_SRC: Self = Self(1 << 3);
    /// Transfer destination (required by [`Device::upload_texture`]).
    pub const TRANSFER_DST: Self = Self(1 << 4);

    /// True when every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for TextureUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Parameters for [`Device::create_texture`]: a single-mip, single-layer 2D image.
#[derive(Clone, Copy, Debug)]
pub struct TextureDesc<'a> {
    /// Debug label (ADR-0009).
    pub label: Option<&'a str>,
    /// Width in texels, nonzero.
    pub width: u32,
    /// Height in texels, nonzero.
    pub height: u32,
    /// Texel format.
    pub format: Format,
    /// Usage flags, nonempty.
    pub usage: TextureUsage,
}

/// Texture filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Nearest texel.
    Nearest,
    /// Bilinear.
    Linear,
}

/// Texture coordinate addressing outside [0, 1].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressMode {
    /// Tile.
    Repeat,
    /// Tile, mirroring every other repetition.
    MirroredRepeat,
    /// Clamp to the edge texel.
    ClampToEdge,
}

/// Parameters for [`Device::create_sampler`].
#[derive(Clone, Copy, Debug)]
pub struct SamplerDesc<'a> {
    /// Debug label (ADR-0009).
    pub label: Option<&'a str>,
    /// Magnification filter.
    pub mag_filter: Filter,
    /// Minification filter.
    pub min_filter: Filter,
    /// Addressing along u.
    pub address_u: AddressMode,
    /// Addressing along v.
    pub address_v: AddressMode,
    /// Addressing along w.
    pub address_w: AddressMode,
}

/// Index element type for [`CommandEncoder::bind_index_buffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    /// 16-bit indices.
    U16,
    /// 32-bit indices.
    U32,
}

impl IndexFormat {
    /// Size of one index in bytes.
    pub const fn size(self) -> u64 {
        match self {
            IndexFormat::U16 => 2,
            IndexFormat::U32 => 4,
        }
    }
}

/// Size of the push constant block visible to all graphics stages (ADR-0010 §3).
pub const PUSH_CONSTANT_BYTES: u32 = 128;

/// Parameters for [`Backend::create_instance`].
#[derive(Clone, Debug)]
pub struct InstanceDesc {
    /// Application name reported to the driver.
    pub app_name: String,
    /// Validation layer policy.
    pub validation: ValidationMode,
    /// Synchronization validation. `Default` reads env `ANVIL_SYNC_VALIDATION` (`1` = true).
    pub sync_validation: bool,
    /// GPU-assisted validation (shader instrumentation, descriptor-indexing checks). `Default`
    /// reads env `ANVIL_GPU_AV` (`1` = true). Replaces synchronization validation when set,
    /// because the layer does not combine them.
    pub gpu_av: bool,
    /// Display the surfaces will belong to; `None` = headless (no surface extensions).
    pub display: Option<RawDisplayHandle>,
}

/// `ANVIL_SYNC_VALIDATION` value to flag; split out so tests need not touch the process env.
fn sync_validation_from(value: Option<&str>) -> bool {
    value == Some("1")
}

impl Default for InstanceDesc {
    fn default() -> Self {
        Self {
            app_name: "anvil".to_owned(),
            validation: ValidationMode::default(),
            sync_validation: sync_validation_from(
                std::env::var("ANVIL_SYNC_VALIDATION").ok().as_deref(),
            ),
            gpu_av: sync_validation_from(std::env::var("ANVIL_GPU_AV").ok().as_deref()),
            display: None,
        }
    }
}

/// Parameters for [`Instance::create_device`].
#[derive(Clone, Debug)]
pub struct DeviceDesc {
    /// Substring of the preferred device name. The backend lets env `ANVIL_GPU` override it.
    pub preferred_gpu: Option<String>,
    /// Frames in flight, valid 1..=3.
    pub frames_in_flight: u32,
}

impl Default for DeviceDesc {
    fn default() -> Self {
        Self {
            preferred_gpu: None,
            frames_in_flight: DEFAULT_FRAMES_IN_FLIGHT,
        }
    }
}

/// Kind of physical device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuKind {
    /// Anything not covered below.
    Other,
    /// Integrated GPU.
    Integrated,
    /// Discrete GPU.
    Discrete,
    /// Virtualized GPU.
    Virtual,
    /// Software rasterizer.
    Cpu,
}

/// Description of the selected device.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// Device name.
    pub name: String,
    /// PCI vendor id.
    pub vendor_id: u32,
    /// Device kind.
    pub kind: GpuKind,
    /// API version as (major, minor, patch).
    pub api_version: (u32, u32, u32),
    /// Driver description.
    pub driver: String,
}

/// Parameters for [`Device::create_swapchain`].
#[derive(Clone, Copy, Debug)]
pub struct SwapchainDesc {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// True = FIFO present mode.
    pub vsync: bool,
    /// Preferred format; the backend falls back and reports the result via
    /// [`Device::swapchain_format`].
    pub preferred_format: Format,
}

/// Parameters for [`Device::create_shader_module`].
#[derive(Clone, Copy, Debug)]
pub struct ShaderModuleDesc<'a> {
    /// SPIR-V words.
    pub spirv: &'a [u32],
    /// Debug label.
    pub label: Option<&'a str>,
}

/// One programmable stage of a pipeline.
#[derive(Clone, Copy, Debug)]
pub struct ShaderStageDesc<'a> {
    /// Shader module containing the stage.
    pub module: ShaderModuleHandle,
    /// Entry point name.
    pub entry: &'a str,
}

/// Primitive assembly topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveTopology {
    /// Independent triangles.
    TriangleList,
    /// Triangle strip.
    TriangleStrip,
    /// Independent lines.
    LineList,
    /// Line strip.
    LineStrip,
    /// Points.
    PointList,
}

/// Face culling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullMode {
    /// No culling.
    None,
    /// Cull front faces.
    Front,
    /// Cull back faces.
    Back,
}

/// Winding order of front faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontFace {
    /// Counter-clockwise is front.
    CounterClockwise,
    /// Clockwise is front.
    Clockwise,
}

/// Blend factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendFactor {
    /// 0.
    Zero,
    /// 1.
    One,
    /// Source alpha.
    SrcAlpha,
    /// 1 - source alpha.
    OneMinusSrcAlpha,
}

/// Blend operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendOp {
    /// src + dst.
    Add,
}

/// Blend state of one color target. `Default` is opaque, write-all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendState {
    /// Blending enabled.
    pub enabled: bool,
    /// Source color factor.
    pub src_color: BlendFactor,
    /// Destination color factor.
    pub dst_color: BlendFactor,
    /// Color operation.
    pub color_op: BlendOp,
    /// Source alpha factor.
    pub src_alpha: BlendFactor,
    /// Destination alpha factor.
    pub dst_alpha: BlendFactor,
    /// Alpha operation.
    pub alpha_op: BlendOp,
    /// Channel write mask: bit 0 R, 1 G, 2 B, 3 A. `0xF` = all.
    pub write_mask: u8,
}

impl Default for BlendState {
    fn default() -> Self {
        Self {
            enabled: false,
            src_color: BlendFactor::One,
            dst_color: BlendFactor::Zero,
            color_op: BlendOp::Add,
            src_alpha: BlendFactor::One,
            dst_alpha: BlendFactor::Zero,
            alpha_op: BlendOp::Add,
            write_mask: 0xF,
        }
    }
}

/// Parameters for [`Device::create_graphics_pipeline`].
///
/// No vertex input state: shaders pull vertices through buffer device addresses. Every pipeline
/// uses the one global layout (bindless set 0 + [`PUSH_CONSTANT_BYTES`] of push constants).
/// `depth_format` may be `None` or `Some(Format::D32Sfloat)` (test + write, `LESS`).
#[derive(Clone, Copy, Debug)]
pub struct GraphicsPipelineDesc<'a> {
    /// Debug label.
    pub label: Option<&'a str>,
    /// Vertex stage.
    pub vertex: ShaderStageDesc<'a>,
    /// Fragment stage.
    pub fragment: ShaderStageDesc<'a>,
    /// Formats of the color targets (dynamic rendering, no render pass object).
    pub color_formats: &'a [Format],
    /// Depth target format, if any.
    pub depth_format: Option<Format>,
    /// Primitive topology.
    pub topology: PrimitiveTopology,
    /// Face culling.
    pub cull: CullMode,
    /// Front face winding.
    pub front_face: FrontFace,
    /// One blend state per color target.
    pub blend: &'a [BlendState],
}

/// What happens to an attachment's contents at the start of rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOp {
    /// Keep existing contents.
    Load,
    /// Clear to the clear value.
    Clear,
    /// Contents are undefined.
    DontCare,
}

/// What happens to an attachment's contents at the end of rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    /// Write results back.
    Store,
    /// Results may be discarded.
    DontCare,
}

/// A color attachment of a rendering scope.
#[derive(Clone, Copy, Debug)]
pub struct ColorAttachment {
    /// Target texture (must be in [`TextureState::ColorAttachment`]).
    pub texture: TextureHandle,
    /// Load operation.
    pub load: LoadOp,
    /// Store operation.
    pub store: StoreOp,
    /// Clear color, used when `load` is [`LoadOp::Clear`].
    pub clear: [f32; 4],
}

/// A depth attachment of a rendering scope. Handled by the backend from M2.
#[derive(Clone, Copy, Debug)]
pub struct DepthAttachment {
    /// Target texture.
    pub texture: TextureHandle,
    /// Load operation.
    pub load: LoadOp,
    /// Store operation.
    pub store: StoreOp,
    /// Clear depth, used when `load` is [`LoadOp::Clear`].
    pub clear: f32,
}

/// Parameters for [`CommandEncoder::begin_rendering`].
#[derive(Clone, Copy, Debug)]
pub struct RenderingDesc<'a> {
    /// Color attachments.
    pub color: &'a [ColorAttachment],
    /// Depth attachment, if any.
    pub depth: Option<DepthAttachment>,
    /// Render area width.
    pub width: u32,
    /// Render area height.
    pub height: u32,
}

/// Viewport. M1 uses native Vulkan conventions (Y down, positive height, no flip).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Minimum depth.
    pub min_depth: f32,
    /// Maximum depth.
    pub max_depth: f32,
}

/// Integer rectangle (scissor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect2D {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// An acquired swapchain image. Passed to [`Device::submit`] (to wire the semaphores) and
/// consumed by [`Device::present`].
#[derive(Clone, Debug)]
pub struct SwapchainImage {
    /// Swapchain the image belongs to.
    pub swapchain: SwapchainHandle,
    /// The image as a texture handle; stale after the swapchain is resized.
    pub texture: TextureHandle,
    /// Image index within the swapchain.
    pub index: u32,
    /// Extent in pixels.
    pub extent: (u32, u32),
    /// Image format.
    pub format: Format,
}

/// Result of [`Device::acquire`].
#[derive(Clone, Debug)]
pub enum Acquire {
    /// An image is ready to render to.
    Image(SwapchainImage),
    /// The swapchain must be resized/recreated; skip the frame.
    OutOfDate,
    /// Extent is 0; skip the frame.
    Minimized,
}

/// Result of [`Device::present`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentStatus {
    /// Presented.
    Ok,
    /// Out of date or suboptimal: call [`Device::resize_swapchain`].
    Recreate,
}

/// Proof that [`Device::begin_frame`] ran for the current frame. Filled in by the backend;
/// callers only pass it to [`Device::begin_commands`].
#[derive(Debug)]
pub struct FrameToken {
    /// Frame slot index.
    pub slot: u32,
    /// Monotonic frame counter (timeline value).
    pub frame: u64,
}

// ---------------------------------------------------------------- diagnostics (ADR-0009)

/// Read-only snapshot of device state for logs, tests and AI agents (ADR-0009).
#[derive(Clone, Debug)]
pub struct DeviceDiagnostics {
    /// The selected GPU.
    pub info: DeviceInfo,
    /// Frames begun so far (timeline value of the latest frame).
    pub frame: u64,
    /// Last timeline value the GPU has completed.
    pub gpu_completed_frame: u64,
    /// Frames-in-flight setting.
    pub frames_in_flight: u32,
    /// Live object counts per kind.
    pub live: LiveObjects,
    /// Objects waiting for the GPU before native destruction.
    pub pending_destroys: usize,
    /// State of every live swapchain.
    pub swapchains: Vec<SwapchainDiagnostics>,
    /// Validation-layer messages seen so far (all zero when validation is off).
    pub validation: ValidationCounts,
    /// GPU memory usage.
    pub memory: MemoryDiagnostics,
    /// Bindless descriptor slot usage.
    pub bindless: BindlessDiagnostics,
}

/// GPU memory usage (ADR-0010 Consequences).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryDiagnostics {
    /// Bytes handed out to live (and not yet retired) allocations.
    pub allocated_bytes: u64,
    /// Bytes reserved from the driver by the allocator.
    pub reserved_bytes: u64,
}

/// Bindless descriptor array capacities and usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindlessDiagnostics {
    /// Capacity of the sampled image array.
    pub image_capacity: u32,
    /// Capacity of the sampler array.
    pub sampler_capacity: u32,
    /// Highest sampled image slot written so far, plus one.
    pub image_high_water: u32,
    /// Highest sampler slot written so far, plus one.
    pub sampler_high_water: u32,
}

/// Live object counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveObjects {
    /// Swapchains.
    pub swapchains: usize,
    /// Textures, including swapchain images.
    pub textures: usize,
    /// Shader modules.
    pub shader_modules: usize,
    /// Graphics pipelines.
    pub pipelines: usize,
    /// Buffers.
    pub buffers: usize,
    /// Samplers.
    pub samplers: usize,
}

/// Swapchain state.
#[derive(Clone, Debug)]
pub struct SwapchainDiagnostics {
    /// Handle.
    pub handle: SwapchainHandle,
    /// Current extent in pixels.
    pub extent: (u32, u32),
    /// Image format.
    pub format: Format,
    /// Number of images.
    pub image_count: u32,
    /// Present mode name (e.g. `"FIFO"`, `"MAILBOX"`).
    pub present_mode: String,
    /// Times the swapchain was recreated.
    pub recreations: u64,
}

/// Validation message counts by severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationCounts {
    /// Errors. The M1 exit criterion is zero.
    pub errors: u64,
    /// Warnings.
    pub warnings: u64,
    /// Info and verbose messages.
    pub infos: u64,
}

// ---------------------------------------------------------------- traits

/// A graphics backend: a family of associated instance, surface and device types.
pub trait Backend: 'static {
    /// Instance type.
    type Instance: Instance<Surface = Self::Surface, Device = Self::Device>;
    /// Surface type; owns its window.
    type Surface: Send + Sync + 'static;
    /// Device type.
    type Device: Device<Surface = Self::Surface>;
    /// Creates the instance. Errors: [`RhiError::Unsupported`] (missing layer with
    /// [`ValidationMode::On`], missing extension), [`RhiError::Backend`].
    fn create_instance(desc: &InstanceDesc) -> Result<Self::Instance, RhiError>;
}

/// Entry point for surfaces and devices.
pub trait Instance: Sized + Send + Sync {
    /// Surface type.
    type Surface;
    /// Device type.
    type Device;
    /// The returned Surface owns `window`, so the window outlives the native surface.
    fn create_surface<W>(&self, window: Arc<W>) -> Result<Self::Surface, RhiError>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static;
    /// `compatible_surface`: the selected queue family must be able to present to it.
    /// `None` = headless. Errors: [`RhiError::NoSuitableDevice`], [`RhiError::InvalidDescriptor`]
    /// (e.g. `frames_in_flight` outside 1..=3).
    fn create_device(
        &self,
        desc: &DeviceDesc,
        compatible_surface: Option<&Self::Surface>,
    ) -> Result<Self::Device, RhiError>;
}

/// The GPU device. Must be `Send` (can move to a render thread); not required to be `Sync`.
///
/// Resource creation takes `&self` (backend pools use interior mutability); frame operations
/// take `&mut self`. `destroy_*` is deferred: handles are stale immediately, the native object
/// is destroyed once the GPU is past the last submission that may use it. Stale handles
/// yield [`RhiError::StaleHandle`] from fallible calls.
pub trait Device: Sized + Send {
    /// Surface type.
    type Surface;
    /// Command encoder type.
    type Commands: CommandEncoder;

    /// Information about the selected GPU.
    fn info(&self) -> &DeviceInfo;
    /// Snapshot of device state (ADR-0009). Cheap enough to call every frame.
    fn diagnostics(&self) -> DeviceDiagnostics;

    /// Creates a swapchain for `surface`. Each swapchain keeps its surface alive.
    fn create_swapchain(
        &self,
        surface: &Self::Surface,
        desc: &SwapchainDesc,
    ) -> Result<SwapchainHandle, RhiError>;
    /// Recreates the swapchain at a new size; old image texture handles become stale.
    fn resize_swapchain(
        &self,
        sc: SwapchainHandle,
        width: u32,
        height: u32,
    ) -> Result<(), RhiError>;
    /// The format the swapchain actually uses.
    fn swapchain_format(&self, sc: SwapchainHandle) -> Result<Format, RhiError>;
    /// Destroys the swapchain (deferred).
    fn destroy_swapchain(&self, sc: SwapchainHandle);

    /// Creates a shader module from SPIR-V words.
    fn create_shader_module(&self, desc: &ShaderModuleDesc)
    -> Result<ShaderModuleHandle, RhiError>;
    /// Creates a graphics pipeline for dynamic rendering.
    fn create_graphics_pipeline(
        &self,
        desc: &GraphicsPipelineDesc,
    ) -> Result<PipelineHandle, RhiError>;
    /// Destroys a shader module (deferred).
    fn destroy_shader_module(&self, h: ShaderModuleHandle);
    /// Destroys a pipeline (deferred).
    fn destroy_pipeline(&self, h: PipelineHandle);

    /// Creates a buffer. Errors: [`RhiError::InvalidDescriptor`] (zero size, empty usage),
    /// [`RhiError::OutOfDeviceMemory`].
    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError>;
    /// Destroys a buffer (deferred). Stale handles are ignored with a warning.
    fn destroy_buffer(&self, h: BufferHandle);
    /// GPU virtual address of the buffer, for `buffer_reference` in shaders.
    fn buffer_address(&self, h: BufferHandle) -> Result<u64, RhiError>;
    /// Copies `data` into a [`Memory::Upload`] buffer at `offset`. The caller guarantees the GPU
    /// is not reading that range.
    fn write_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;
    /// Copies from a [`Memory::Readback`] buffer into `out`. The caller guarantees the GPU
    /// finished writing the range (e.g. after [`Device::wait_idle`]).
    fn read_buffer(&self, h: BufferHandle, offset: u64, out: &mut [u8]) -> Result<(), RhiError>;
    /// Queues a copy of `data` into any buffer with [`BufferUsage::TRANSFER_DST`]. Callable at
    /// any time; the copy runs ahead of the commands of the next submit, in the same
    /// submission. Overlapping ranges within one submit are unspecified.
    fn upload_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;

    /// Creates a texture (2D, one mip, one layer). Samplable textures get a bindless slot.
    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError>;
    /// Destroys a texture (deferred). Stale or swapchain-owned handles are ignored with a warning.
    fn destroy_texture(&self, h: TextureHandle);
    /// Bindless slot of a [`TextureUsage::SAMPLED`] texture in the global image array.
    fn texture_index(&self, h: TextureHandle) -> Result<u32, RhiError>;
    /// Queues an upload of tightly packed texels (mip 0, layer 0) into a texture with
    /// [`TextureUsage::TRANSFER_DST`]. Callable at any time, like [`Device::upload_buffer`].
    /// The texture ends in [`TextureState::ShaderRead`].
    fn upload_texture(&self, h: TextureHandle, data: &[u8]) -> Result<(), RhiError>;

    /// Creates a sampler and writes it into the bindless sampler array.
    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerHandle, RhiError>;
    /// Destroys a sampler (deferred). Stale handles are ignored with a warning.
    fn destroy_sampler(&self, h: SamplerHandle);
    /// Bindless slot of a sampler in the global sampler array.
    fn sampler_index(&self, h: SamplerHandle) -> Result<u32, RhiError>;

    /// Starts a frame: waits for the slot's previous work and runs deferred destroys.
    fn begin_frame(&mut self) -> Result<FrameToken, RhiError>;
    /// Acquires the next swapchain image.
    fn acquire(&mut self, sc: SwapchainHandle) -> Result<Acquire, RhiError>;
    /// Returns an owned encoder (no borrow of the device while recording).
    fn begin_commands(&mut self, frame: &FrameToken) -> Result<Self::Commands, RhiError>;
    /// M1: `image` identifies the swapchain and image whose acquire/render-done semaphores
    /// are wired. A general SubmitDesc arrives in M2 only when needed.
    fn submit(
        &mut self,
        cmds: <Self::Commands as CommandEncoder>::Finished,
        image: &SwapchainImage,
    ) -> Result<(), RhiError>;
    /// Presents `image`. Out-of-date / suboptimal is [`PresentStatus::Recreate`], not an error.
    fn present(
        &mut self,
        sc: SwapchainHandle,
        image: SwapchainImage,
    ) -> Result<PresentStatus, RhiError>;
    /// Blocks until the GPU is idle.
    fn wait_idle(&self) -> Result<(), RhiError>;
}

/// Command recording. Methods are infallible: the first error (stale handle, invalid state)
/// is recorded and returned from [`CommandEncoder::finish`].
pub trait CommandEncoder {
    /// Finished command buffer, consumed by [`Device::submit`].
    type Finished;
    /// Transitions a texture between states.
    fn texture_barrier(&mut self, t: TextureHandle, from: TextureState, to: TextureState);
    /// Begins a dynamic rendering scope.
    fn begin_rendering(&mut self, desc: &RenderingDesc);
    /// Ends the rendering scope.
    fn end_rendering(&mut self);
    /// Binds a graphics pipeline.
    fn bind_pipeline(&mut self, p: PipelineHandle);
    /// Sets the viewport.
    fn set_viewport(&mut self, v: Viewport);
    /// Sets the scissor rectangle.
    fn set_scissor(&mut self, r: Rect2D);
    /// Non-indexed draw.
    fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    );
    /// Indexed draw. Requires a bound pipeline and index buffer.
    fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    );
    /// Binds an index buffer (inside rendering). `offset` must be a multiple of the index size.
    fn bind_index_buffer(&mut self, b: BufferHandle, offset: u64, format: IndexFormat);
    /// Writes push constants visible to all graphics stages. `offset` and `data.len()` must be
    /// multiples of 4 and `offset + len <= PUSH_CONSTANT_BYTES`.
    fn push_constants(&mut self, offset: u32, data: &[u8]);
    /// Copies between buffers (outside rendering). A copy into a [`Memory::Readback`] buffer is
    /// followed by a host-read barrier.
    fn copy_buffer_to_buffer(
        &mut self,
        src: BufferHandle,
        src_offset: u64,
        dst: BufferHandle,
        dst_offset: u64,
        size: u64,
    );
    /// Copies a whole texture (mip 0) into a buffer (outside rendering). The texture must be
    /// in [`TextureState::TransferSrc`].
    fn copy_texture_to_buffer(&mut self, t: TextureHandle, dst: BufferHandle, dst_offset: u64);
    /// Opens a debug label (no-op if unsupported).
    fn begin_label(&mut self, name: &str, color: [f32; 4]);
    /// Closes the innermost debug label.
    fn end_label(&mut self);
    /// Ends recording; surfaces the first recorded error.
    fn finish(self) -> Result<Self::Finished, RhiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(ValidationMode::default(), ValidationMode::Auto);
        let d = DeviceDesc::default();
        assert_eq!(d.frames_in_flight, DEFAULT_FRAMES_IN_FLIGHT);
        assert!(d.preferred_gpu.is_none());
        let b = BlendState::default();
        assert!(!b.enabled);
        assert_eq!(b.write_mask, 0xF);
        let i = InstanceDesc::default();
        assert_eq!(i.validation, ValidationMode::Auto);
        assert!(i.display.is_none());
    }

    #[test]
    fn sync_validation_env_parsing() {
        assert!(sync_validation_from(Some("1")));
        assert!(!sync_validation_from(Some("0")));
        assert!(!sync_validation_from(Some("true")));
        assert!(!sync_validation_from(None));
    }

    #[test]
    fn format_helpers() {
        assert!(Format::Bgra8Srgb.is_srgb() && Format::Rgba8Srgb.is_srgb());
        assert!(!Format::Bgra8Unorm.is_srgb() && !Format::D32Sfloat.is_srgb());
        assert!(Format::D32Sfloat.is_depth() && !Format::Rgba8Unorm.is_depth());
    }

    #[test]
    fn usage_flag_ops() {
        let u = BufferUsage::INDEX | BufferUsage::TRANSFER_DST;
        assert!(u.contains(BufferUsage::INDEX) && u.contains(BufferUsage::NONE));
        assert!(!u.contains(BufferUsage::STORAGE | BufferUsage::INDEX));
        assert!(BufferUsage::NONE.is_empty() && !u.is_empty());
        let t = TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST;
        assert!(t.contains(TextureUsage::SAMPLED) && !t.contains(TextureUsage::DEPTH_ATTACHMENT));
        assert!(TextureUsage::NONE.is_empty() && !t.is_empty());
    }

    #[test]
    fn format_bytes_per_texel() {
        assert_eq!(Format::R8Unorm.bytes_per_texel(), 1);
        assert_eq!(Format::Rg8Unorm.bytes_per_texel(), 2);
        assert_eq!(Format::Rgba8Unorm.bytes_per_texel(), 4);
        assert_eq!(Format::Bgra8Srgb.bytes_per_texel(), 4);
        assert_eq!(Format::Rgba16Float.bytes_per_texel(), 8);
        assert_eq!(Format::D32Sfloat.bytes_per_texel(), 4);
        assert_eq!(IndexFormat::U16.size() + IndexFormat::U32.size(), 6);
    }

    #[test]
    fn error_display() {
        assert_eq!(RhiError::DeviceLost.to_string(), "GPU device lost");
        assert_eq!(RhiError::SurfaceLost.to_string(), "surface lost");
        assert_eq!(
            RhiError::StaleHandle {
                kind: "texture",
                index: 3,
                generation: 1
            }
            .to_string(),
            "stale or invalid texture handle (index 3, generation 1)"
        );
        assert_eq!(
            RhiError::Backend {
                op: "vkQueueSubmit",
                code: -4
            }
            .to_string(),
            "backend failure in vkQueueSubmit: code -4"
        );
        assert_eq!(
            RhiError::Unsupported { what: "x".into() }.to_string(),
            "unsupported: x"
        );
        assert_eq!(
            RhiError::InvalidDescriptor("bad".into()).to_string(),
            "invalid descriptor: bad"
        );
        assert_eq!(
            RhiError::NoSuitableDevice {
                reason: "none".into()
            }
            .to_string(),
            "no suitable GPU: none"
        );
    }

    #[test]
    fn handles_are_distinct_types() {
        use std::any::TypeId;
        let ids = [
            TypeId::of::<BufferHandle>(),
            TypeId::of::<TextureHandle>(),
            TypeId::of::<SamplerHandle>(),
            TypeId::of::<ShaderModuleHandle>(),
            TypeId::of::<PipelineHandle>(),
            TypeId::of::<SwapchainHandle>(),
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    /// Minimal backend proving the trait set is implementable and usable generically.
    struct Mock;
    struct MockSurface;
    struct MockInstance;
    struct MockDevice(DeviceInfo);
    struct MockCommands(u32);

    impl Backend for Mock {
        type Instance = MockInstance;
        type Surface = MockSurface;
        type Device = MockDevice;
        fn create_instance(_: &InstanceDesc) -> Result<MockInstance, RhiError> {
            Ok(MockInstance)
        }
    }
    impl Instance for MockInstance {
        type Surface = MockSurface;
        type Device = MockDevice;
        fn create_surface<W>(&self, _: Arc<W>) -> Result<MockSurface, RhiError>
        where
            W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        {
            Ok(MockSurface)
        }
        fn create_device(
            &self,
            _: &DeviceDesc,
            _: Option<&MockSurface>,
        ) -> Result<MockDevice, RhiError> {
            Ok(MockDevice(DeviceInfo {
                name: "mock".into(),
                vendor_id: 0,
                kind: GpuKind::Other,
                api_version: (1, 3, 0),
                driver: String::new(),
            }))
        }
    }
    impl Device for MockDevice {
        type Surface = MockSurface;
        type Commands = MockCommands;
        fn info(&self) -> &DeviceInfo {
            &self.0
        }
        fn diagnostics(&self) -> DeviceDiagnostics {
            DeviceDiagnostics {
                info: self.0.clone(),
                frame: 0,
                gpu_completed_frame: 0,
                frames_in_flight: DEFAULT_FRAMES_IN_FLIGHT,
                live: LiveObjects::default(),
                pending_destroys: 0,
                swapchains: Vec::new(),
                validation: ValidationCounts::default(),
                memory: MemoryDiagnostics::default(),
                bindless: BindlessDiagnostics::default(),
            }
        }
        fn create_buffer(&self, _: &BufferDesc) -> Result<BufferHandle, RhiError> {
            Ok(BufferHandle::NULL)
        }
        fn destroy_buffer(&self, _: BufferHandle) {}
        fn buffer_address(&self, _: BufferHandle) -> Result<u64, RhiError> {
            Ok(0x1000)
        }
        fn write_buffer(&self, _: BufferHandle, _: u64, _: &[u8]) -> Result<(), RhiError> {
            Ok(())
        }
        fn read_buffer(&self, _: BufferHandle, _: u64, _: &mut [u8]) -> Result<(), RhiError> {
            Ok(())
        }
        fn upload_buffer(&self, _: BufferHandle, _: u64, _: &[u8]) -> Result<(), RhiError> {
            Ok(())
        }
        fn create_texture(&self, _: &TextureDesc) -> Result<TextureHandle, RhiError> {
            Ok(TextureHandle::NULL)
        }
        fn destroy_texture(&self, _: TextureHandle) {}
        fn texture_index(&self, _: TextureHandle) -> Result<u32, RhiError> {
            Ok(0)
        }
        fn upload_texture(&self, _: TextureHandle, _: &[u8]) -> Result<(), RhiError> {
            Ok(())
        }
        fn create_sampler(&self, _: &SamplerDesc) -> Result<SamplerHandle, RhiError> {
            Ok(SamplerHandle::NULL)
        }
        fn destroy_sampler(&self, _: SamplerHandle) {}
        fn sampler_index(&self, _: SamplerHandle) -> Result<u32, RhiError> {
            Ok(0)
        }
        fn create_swapchain(
            &self,
            _: &MockSurface,
            _: &SwapchainDesc,
        ) -> Result<SwapchainHandle, RhiError> {
            Ok(SwapchainHandle::NULL)
        }
        fn resize_swapchain(&self, _: SwapchainHandle, _: u32, _: u32) -> Result<(), RhiError> {
            Ok(())
        }
        fn swapchain_format(&self, _: SwapchainHandle) -> Result<Format, RhiError> {
            Ok(Format::Bgra8Srgb)
        }
        fn destroy_swapchain(&self, _: SwapchainHandle) {}
        fn create_shader_module(
            &self,
            _: &ShaderModuleDesc,
        ) -> Result<ShaderModuleHandle, RhiError> {
            Ok(ShaderModuleHandle::NULL)
        }
        fn create_graphics_pipeline(
            &self,
            _: &GraphicsPipelineDesc,
        ) -> Result<PipelineHandle, RhiError> {
            Ok(PipelineHandle::NULL)
        }
        fn destroy_shader_module(&self, _: ShaderModuleHandle) {}
        fn destroy_pipeline(&self, _: PipelineHandle) {}
        fn begin_frame(&mut self) -> Result<FrameToken, RhiError> {
            Ok(FrameToken { slot: 0, frame: 1 })
        }
        fn acquire(&mut self, _: SwapchainHandle) -> Result<Acquire, RhiError> {
            Ok(Acquire::Minimized)
        }
        fn begin_commands(&mut self, _: &FrameToken) -> Result<MockCommands, RhiError> {
            Ok(MockCommands(0))
        }
        fn submit(&mut self, _: u32, _: &SwapchainImage) -> Result<(), RhiError> {
            Ok(())
        }
        fn present(
            &mut self,
            _: SwapchainHandle,
            _: SwapchainImage,
        ) -> Result<PresentStatus, RhiError> {
            Ok(PresentStatus::Ok)
        }
        fn wait_idle(&self) -> Result<(), RhiError> {
            Ok(())
        }
    }
    impl CommandEncoder for MockCommands {
        type Finished = u32;
        fn texture_barrier(&mut self, _: TextureHandle, _: TextureState, _: TextureState) {}
        fn begin_rendering(&mut self, _: &RenderingDesc) {}
        fn end_rendering(&mut self) {}
        fn bind_pipeline(&mut self, _: PipelineHandle) {}
        fn set_viewport(&mut self, _: Viewport) {}
        fn set_scissor(&mut self, _: Rect2D) {}
        fn draw(&mut self, n: u32, _: u32, _: u32, _: u32) {
            self.0 += n;
        }
        fn draw_indexed(&mut self, n: u32, _: u32, _: u32, _: i32, _: u32) {
            self.0 += n;
        }
        fn bind_index_buffer(&mut self, _: BufferHandle, _: u64, _: IndexFormat) {}
        fn push_constants(&mut self, _: u32, _: &[u8]) {}
        fn copy_buffer_to_buffer(
            &mut self,
            _: BufferHandle,
            _: u64,
            _: BufferHandle,
            _: u64,
            _: u64,
        ) {
        }
        fn copy_texture_to_buffer(&mut self, _: TextureHandle, _: BufferHandle, _: u64) {}
        fn begin_label(&mut self, _: &str, _: [f32; 4]) {}
        fn end_label(&mut self) {}
        fn finish(self) -> Result<u32, RhiError> {
            Ok(self.0)
        }
    }

    /// Frame loop written only against the traits.
    fn one_frame<B: Backend>() -> Result<(), RhiError> {
        let inst = B::create_instance(&InstanceDesc::default())?;
        let mut dev = inst.create_device(&DeviceDesc::default(), None)?;
        let frame = dev.begin_frame()?;
        let mut cmd = dev.begin_commands(&frame)?;
        cmd.draw(3, 1, 0, 0);
        let fin = cmd.finish()?;
        let img = SwapchainImage {
            swapchain: SwapchainHandle::NULL,
            texture: TextureHandle::NULL,
            index: 0,
            extent: (1, 1),
            format: Format::Bgra8Srgb,
        };
        dev.submit(fin, &img)?;
        assert_eq!(dev.present(img.swapchain, img)?, PresentStatus::Ok);
        dev.wait_idle()
    }

    #[test]
    fn mock_backend_implements_traits() {
        one_frame::<Mock>().unwrap();
    }
}
