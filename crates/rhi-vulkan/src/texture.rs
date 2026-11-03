//! MODULE-006: textures, samplers and bindless slots.

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use rhi::{
    AddressMode, Filter, Format, RhiError, SamplerDesc, SamplerHandle, TextureDesc, TextureHandle,
    TextureState, TextureUsage, TextureViewDesc,
};

use crate::buffer::create_staging;
use crate::destroy::{Doomed, destroy_doomed};
use crate::device::DeviceShared;
use crate::error::{invalid, map_vk, stale};
use crate::registry::generated_name;
use crate::upload::PendingUpload;
use crate::{lock, read, write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextureOwner {
    Swapchain,
    Device,
    /// A view of another texture's image: no memory, destroyed as a view only.
    View,
}

pub(crate) struct VkTexture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: Format,
    /// Size of the first mip of this texture or view.
    pub width: u32,
    pub height: u32,
    pub usage: TextureUsage,
    /// First mip, mip count, first layer and layer count covered (the whole image for textures).
    pub base_mip: u32,
    pub level_count: u32,
    pub base_layer: u32,
    pub layer_count: u32,
    /// Mips and layers of the underlying image (for validating views).
    pub image_mips: u32,
    pub image_layers: u32,
    pub cube: bool,
    pub owner: TextureOwner,
    pub allocation: Option<Allocation>,
    /// Advisory whole-texture state (ADR-0010 §9).
    pub state: TextureState,
    pub label: String,
}

impl VkTexture {
    pub(crate) fn range(&self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: aspect_of(self.format),
            base_mip_level: self.base_mip,
            level_count: self.level_count,
            base_array_layer: self.base_layer,
            layer_count: self.layer_count,
        }
    }

    /// Plain 2D (one layer, not a cube): the shape of the bindless `image2D` array and of a 2D
    /// sampled slot. shortcut: array (non-cube) views get no bindless slot; a cube is sampled
    /// through the same binding 0 as `textureCube[]` (M5c).
    pub(crate) fn is_plain_2d(&self) -> bool {
        self.layer_count == 1 && !self.cube
    }

    pub(crate) fn has_sampled_slot(&self) -> bool {
        self.usage.contains(TextureUsage::SAMPLED) && (self.is_plain_2d() || self.is_cube_view())
    }

    /// A whole cube (six layers, `CUBE` view type): sampled through `textureCube[]`.
    fn is_cube_view(&self) -> bool {
        self.cube && self.layer_count == 6
    }

    pub(crate) fn has_storage_slot(&self) -> bool {
        self.usage.contains(TextureUsage::STORAGE) && self.is_plain_2d() && self.level_count == 1
    }
}

pub(crate) struct VkSampler {
    pub raw: vk::Sampler,
    pub label: String,
}

// ---------------------------------------------------------------- pure helpers

pub(crate) fn rhi_to_vk_format(f: Format) -> vk::Format {
    match f {
        Format::Bgra8Srgb => vk::Format::B8G8R8A8_SRGB,
        Format::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        Format::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        Format::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        Format::R8Unorm => vk::Format::R8_UNORM,
        Format::Rg8Unorm => vk::Format::R8G8_UNORM,
        Format::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        Format::Rg16Float => vk::Format::R16G16_SFLOAT,
        Format::R32Float => vk::Format::R32_SFLOAT,
        Format::D32Sfloat => vk::Format::D32_SFLOAT,
    }
}

pub(crate) fn vk_to_rhi_format(f: vk::Format) -> Option<Format> {
    Some(match f {
        vk::Format::B8G8R8A8_SRGB => Format::Bgra8Srgb,
        vk::Format::B8G8R8A8_UNORM => Format::Bgra8Unorm,
        vk::Format::R8G8B8A8_SRGB => Format::Rgba8Srgb,
        vk::Format::R8G8B8A8_UNORM => Format::Rgba8Unorm,
        vk::Format::R8_UNORM => Format::R8Unorm,
        vk::Format::R8G8_UNORM => Format::Rg8Unorm,
        vk::Format::R16G16B16A16_SFLOAT => Format::Rgba16Float,
        vk::Format::R16G16_SFLOAT => Format::Rg16Float,
        vk::Format::R32_SFLOAT => Format::R32Float,
        vk::Format::D32_SFLOAT => Format::D32Sfloat,
        _ => return None,
    })
}

pub(crate) fn aspect_of(f: Format) -> vk::ImageAspectFlags {
    if f.is_depth() {
        vk::ImageAspectFlags::DEPTH
    } else {
        vk::ImageAspectFlags::COLOR
    }
}

pub(crate) fn usage_to_vk(u: TextureUsage) -> vk::ImageUsageFlags {
    let mut f = vk::ImageUsageFlags::empty();
    for (bit, vk_bit) in [
        (TextureUsage::SAMPLED, vk::ImageUsageFlags::SAMPLED),
        (
            TextureUsage::COLOR_ATTACHMENT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT,
        ),
        (
            TextureUsage::DEPTH_ATTACHMENT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        ),
        (
            TextureUsage::TRANSFER_SRC,
            vk::ImageUsageFlags::TRANSFER_SRC,
        ),
        (
            TextureUsage::TRANSFER_DST,
            vk::ImageUsageFlags::TRANSFER_DST,
        ),
        (TextureUsage::STORAGE, vk::ImageUsageFlags::STORAGE),
    ] {
        if u.contains(bit) {
            f |= vk_bit;
        }
    }
    f
}

fn required_features() -> [(TextureUsage, &'static str, vk::FormatFeatureFlags); 6] {
    [
        (
            TextureUsage::STORAGE,
            "STORAGE",
            vk::FormatFeatureFlags::STORAGE_IMAGE,
        ),
        (
            TextureUsage::SAMPLED,
            "SAMPLED",
            vk::FormatFeatureFlags::SAMPLED_IMAGE,
        ),
        (
            TextureUsage::COLOR_ATTACHMENT,
            "COLOR_ATTACHMENT",
            vk::FormatFeatureFlags::COLOR_ATTACHMENT,
        ),
        (
            TextureUsage::DEPTH_ATTACHMENT,
            "DEPTH_ATTACHMENT",
            vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT,
        ),
        (
            TextureUsage::TRANSFER_SRC,
            "TRANSFER_SRC",
            vk::FormatFeatureFlags::TRANSFER_SRC,
        ),
        (
            TextureUsage::TRANSFER_DST,
            "TRANSFER_DST",
            vk::FormatFeatureFlags::TRANSFER_DST,
        ),
    ]
}

/// Device facts needed to validate a texture description.
pub(crate) struct FormatFacts {
    pub optimal_features: vk::FormatFeatureFlags,
    pub max_dimension: u32,
}

/// Pure validation of a [`TextureDesc`] (MODULE-006 §8 step 1).
pub(crate) fn validate_texture_desc(
    desc: &TextureDesc,
    facts: &FormatFacts,
) -> Result<(), RhiError> {
    if desc.width == 0 || desc.height == 0 {
        return Err(invalid(format!(
            "create_texture: extent {}x{} must be nonzero",
            desc.width, desc.height
        )));
    }
    if desc.width > facts.max_dimension || desc.height > facts.max_dimension {
        return Err(invalid(format!(
            "create_texture: extent {}x{} exceeds the device limit {}",
            desc.width, desc.height, facts.max_dimension
        )));
    }
    if desc.usage.is_empty() {
        return Err(invalid("create_texture: usage must not be empty"));
    }
    let max_mips = desc.width.max(desc.height).ilog2() + 1;
    if desc.mip_levels == 0 || desc.mip_levels > max_mips {
        return Err(invalid(format!(
            "create_texture: mip_levels {} must be 1..={max_mips} for {}x{}",
            desc.mip_levels, desc.width, desc.height
        )));
    }
    if desc.array_layers == 0 {
        return Err(invalid("create_texture: array_layers must be nonzero"));
    }
    if desc.cube && (desc.array_layers != 6 || desc.width != desc.height) {
        return Err(invalid(
            "create_texture: a cube needs 6 layers and a square extent",
        ));
    }
    let depth = desc.format.is_depth();
    if desc.usage.contains(TextureUsage::COLOR_ATTACHMENT) && depth {
        return Err(invalid(format!(
            "create_texture: COLOR_ATTACHMENT with depth format {:?}",
            desc.format
        )));
    }
    if desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) && !depth {
        return Err(invalid(format!(
            "create_texture: DEPTH_ATTACHMENT with color format {:?}",
            desc.format
        )));
    }
    for (bit, name, feature) in required_features() {
        if desc.usage.contains(bit) && !facts.optimal_features.contains(feature) {
            return Err(RhiError::Unsupported {
                what: format!("{:?} with {name}", desc.format),
            });
        }
    }
    Ok(())
}

/// `data.len() == width * height * bytes_per_texel`, overflow-safe.
pub(crate) fn check_upload_len(
    width: u32,
    height: u32,
    format: Format,
    len: usize,
) -> Result<(), RhiError> {
    let expected = u64::from(width) * u64::from(height) * u64::from(format.bytes_per_texel());
    if len as u64 != expected {
        return Err(invalid(format!(
            "upload_texture: expected {expected} bytes ({width}x{height} {format:?}), got {len}"
        )));
    }
    Ok(())
}

/// Result of comparing the caller's `from` with the tracked state (advisory, ADR-0010 §9).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StateCheck {
    Match,
    Mismatch { tracked: TextureState },
}

pub(crate) fn check_tracked_state(tracked: TextureState, from: TextureState) -> StateCheck {
    if tracked == from {
        StateCheck::Match
    } else {
        StateCheck::Mismatch { tracked }
    }
}

pub(crate) fn filter_to_vk(f: Filter) -> vk::Filter {
    match f {
        Filter::Nearest => vk::Filter::NEAREST,
        Filter::Linear => vk::Filter::LINEAR,
    }
}

pub(crate) fn address_to_vk(a: AddressMode) -> vk::SamplerAddressMode {
    match a {
        AddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        AddressMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        AddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
    }
}

pub(crate) fn compare_to_vk(c: rhi::CompareOp) -> vk::CompareOp {
    use rhi::CompareOp as C;
    match c {
        C::Never => vk::CompareOp::NEVER,
        C::Less => vk::CompareOp::LESS,
        C::Equal => vk::CompareOp::EQUAL,
        C::LessOrEqual => vk::CompareOp::LESS_OR_EQUAL,
        C::Greater => vk::CompareOp::GREATER,
        C::GreaterOrEqual => vk::CompareOp::GREATER_OR_EQUAL,
        C::Always => vk::CompareOp::ALWAYS,
    }
}

pub(crate) fn sampler_info(d: &SamplerDesc) -> vk::SamplerCreateInfo<'static> {
    vk::SamplerCreateInfo::default()
        .mag_filter(filter_to_vk(d.mag_filter))
        .min_filter(filter_to_vk(d.min_filter))
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(address_to_vk(d.address_u))
        .address_mode_v(address_to_vk(d.address_v))
        .address_mode_w(address_to_vk(d.address_w))
        .anisotropy_enable(false)
        .compare_enable(d.compare.is_some())
        .compare_op(d.compare.map_or(vk::CompareOp::ALWAYS, compare_to_vk))
        .min_lod(0.0)
        .max_lod(0.0)
        .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK)
        .unnormalized_coordinates(false)
}

// ---------------------------------------------------------------- textures

fn destroy_image_now(s: &DeviceShared, image: vk::Image, view: vk::ImageView, a: Allocation) {
    // SAFETY: the texture was never submitted (failure paths during creation).
    unsafe {
        s.raw.destroy_image_view(view, None);
        s.raw.destroy_image(image, None);
    }
    s.free(a);
}

pub(crate) fn create_texture(
    s: &DeviceShared,
    desc: &TextureDesc,
) -> Result<TextureHandle, RhiError> {
    // SAFETY: the physical device belongs to the live instance.
    let props = unsafe {
        s.instance
            .raw
            .get_physical_device_format_properties(s.physical, rhi_to_vk_format(desc.format))
    };
    let facts = FormatFacts {
        optimal_features: props.optimal_tiling_features,
        max_dimension: s.limits.max_image_dimension_2d,
    };
    if let Err(e) = validate_texture_desc(desc, &facts) {
        if matches!(e, RhiError::Unsupported { .. }) {
            tracing::warn!(target: "rhi_vulkan::texture", format = ?desc.format, usage = ?desc.usage, "unsupported texture description");
        }
        return Err(e);
    }
    let label = desc.label.unwrap_or("texture");
    let vk_format = rhi_to_vk_format(desc.format);
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk_format)
        .extent(vk::Extent3D {
            width: desc.width,
            height: desc.height,
            depth: 1,
        })
        .flags(if desc.cube {
            vk::ImageCreateFlags::CUBE_COMPATIBLE
        } else {
            vk::ImageCreateFlags::empty()
        })
        .mip_levels(desc.mip_levels)
        .array_layers(desc.array_layers)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage_to_vk(desc.usage))
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: `ci` outlives the call; the device is live.
    let image = unsafe { s.raw.create_image(&ci, None) }.map_err(|r| map_vk("vkCreateImage", r))?;
    // SAFETY: `image` was just created on this device.
    let requirements = unsafe { s.raw.get_image_memory_requirements(image) };
    let attachment = desc.usage.contains(TextureUsage::COLOR_ATTACHMENT)
        || desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT);
    let allocation = match s.alloc(&AllocationCreateDesc {
        name: label,
        requirements,
        location: MemoryLocation::GpuOnly,
        linear: false,
        allocation_scheme: if attachment {
            AllocationScheme::DedicatedImage(image)
        } else {
            AllocationScheme::GpuAllocatorManaged
        },
    }) {
        Ok(a) => a,
        Err(e) => {
            // SAFETY: the image was never bound or used.
            unsafe { s.raw.destroy_image(image, None) };
            return Err(e);
        }
    };
    // SAFETY: the image is unbound and the allocation satisfies its requirements.
    if let Err(r) = unsafe {
        s.raw
            .bind_image_memory(image, allocation.memory(), allocation.offset())
    } {
        s.free(allocation);
        // SAFETY: bind failed, so the image is unused.
        unsafe { s.raw.destroy_image(image, None) };
        return Err(map_vk("vkBindImageMemory", r));
    }
    // Views need an image usage that allows them; transfer-only images have none.
    let view = if desc.usage.contains(TextureUsage::SAMPLED)
        || desc.usage.contains(TextureUsage::COLOR_ATTACHMENT)
        || desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT)
    {
        let view_type = if desc.cube {
            vk::ImageViewType::CUBE
        } else if desc.array_layers > 1 {
            vk::ImageViewType::TYPE_2D_ARRAY
        } else {
            vk::ImageViewType::TYPE_2D
        };
        let view_ci = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(view_type)
            .format(vk_format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect_of(desc.format),
                base_mip_level: 0,
                level_count: desc.mip_levels,
                base_array_layer: 0,
                layer_count: desc.array_layers,
            });
        // SAFETY: the image is bound; the view format/aspect match the image.
        match unsafe { s.raw.create_image_view(&view_ci, None) } {
            Ok(v) => v,
            Err(r) => {
                s.free(allocation);
                // SAFETY: the image is unused.
                unsafe { s.raw.destroy_image(image, None) };
                return Err(map_vk("vkCreateImageView", r));
            }
        }
    } else {
        vk::ImageView::null()
    };
    let h = write(&s.pools.textures).insert(VkTexture {
        image,
        view,
        format: desc.format,
        width: desc.width,
        height: desc.height,
        usage: desc.usage,
        base_mip: 0,
        level_count: desc.mip_levels,
        base_layer: 0,
        layer_count: desc.array_layers,
        image_mips: desc.mip_levels,
        image_layers: desc.array_layers,
        cube: desc.cube,
        owner: TextureOwner::Device,
        allocation: Some(allocation),
        state: TextureState::Undefined,
        label: String::new(),
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("texture", h), str::to_owned);
    s.set_name(image, &name);
    if view != vk::ImageView::null() {
        s.set_name(view, &format!("{name}.view"));
    }
    if let Some(t) = write(&s.pools.textures).get_mut(h) {
        t.label.clone_from(&name);
    }
    let slot = h.index();
    let (sampled, storage) = {
        let pool = read(&s.pools.textures);
        pool.get(h).map_or((false, false), |t| {
            (t.has_sampled_slot(), t.has_storage_slot())
        })
    };
    if let Err(e) = write_slots(s, slot, view, sampled, storage) {
        let t = write(&s.pools.textures).remove(h);
        if let Some(mut t) = t
            && let Some(a) = t.allocation.take()
        {
            destroy_image_now(s, t.image, t.view, a);
        }
        return Err(e);
    }
    tracing::debug!(
        target: "rhi_vulkan::texture", handle = ?h, label = %name, width = desc.width,
        height = desc.height, format = ?desc.format, usage = ?desc.usage, slot, "texture created"
    );
    Ok(h)
}

/// Writes the bindless descriptors a texture or view is entitled to; `Unsupported` when `slot`
/// is beyond a binding's capacity (the caller undoes the creation).
fn write_slots(
    s: &DeviceShared,
    slot: u32,
    view: vk::ImageView,
    sampled: bool,
    storage: bool,
) -> Result<(), RhiError> {
    for (wanted, cap, what) in [
        (sampled, s.bindings.sampled_image_capacity, "sampled image"),
        (storage, s.bindings.storage_image_capacity, "storage image"),
    ] {
        if wanted && slot >= cap {
            tracing::warn!(target: "rhi_vulkan::texture", slot, capacity = cap, what, "bindless capacity exceeded");
            return Err(RhiError::Unsupported {
                what: format!("bindless {what} capacity ({cap}) exceeded"),
            });
        }
    }
    if sampled {
        s.bindings.write_sampled_image(&s.raw, slot, view);
    }
    if storage {
        s.bindings.write_storage_image(&s.raw, slot, view);
    }
    Ok(())
}

/// A view of `parent`'s image over a mip and layer range, as a texture of its own.
pub(crate) fn create_texture_view(
    s: &DeviceShared,
    parent: TextureHandle,
    desc: &TextureViewDesc,
) -> Result<TextureHandle, RhiError> {
    let p = {
        let pool = read(&s.pools.textures);
        let t = pool.get(parent).ok_or_else(|| stale("texture", parent))?;
        if t.owner != TextureOwner::Device {
            return Err(invalid(
                "create_texture_view: the parent must be a device texture, not a view or swapchain image",
            ));
        }
        (
            t.image,
            t.format,
            t.width,
            t.height,
            t.usage,
            t.image_mips,
            t.image_layers,
        )
    };
    let (image, format, pw, ph, pusage, mips, layers) = p;
    let end_mip = desc.base_mip.checked_add(desc.mip_count);
    let end_layer = desc.base_layer.checked_add(desc.layer_count);
    if desc.mip_count == 0
        || desc.layer_count == 0
        || end_mip.is_none_or(|e| e > mips)
        || end_layer.is_none_or(|e| e > layers)
    {
        return Err(invalid(format!(
            "create_texture_view: mips {}+{} / layers {}+{} outside the parent's {mips} mips / {layers} layers",
            desc.base_mip, desc.mip_count, desc.base_layer, desc.layer_count
        )));
    }
    if desc.usage.is_empty() || !pusage.contains(desc.usage) {
        return Err(invalid(format!(
            "create_texture_view: usage {:?} is not a subset of the parent's {pusage:?}",
            desc.usage
        )));
    }
    if desc.usage.contains(TextureUsage::STORAGE) && desc.mip_count != 1 {
        return Err(invalid("create_texture_view: a storage view has one mip"));
    }
    let view_ci = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(if desc.layer_count > 1 {
            vk::ImageViewType::TYPE_2D_ARRAY
        } else {
            vk::ImageViewType::TYPE_2D
        })
        .format(rhi_to_vk_format(format))
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: aspect_of(format),
            base_mip_level: desc.base_mip,
            level_count: desc.mip_count,
            base_array_layer: desc.base_layer,
            layer_count: desc.layer_count,
        });
    // SAFETY: the parent image is alive (checked above, destruction is deferred and the caller
    // keeps the parent past its views); the range was validated against its mips and layers.
    let view = unsafe { s.raw.create_image_view(&view_ci, None) }
        .map_err(|r| map_vk("vkCreateImageView", r))?;
    let h = write(&s.pools.textures).insert(VkTexture {
        image,
        view,
        format,
        width: (pw >> desc.base_mip).max(1),
        height: (ph >> desc.base_mip).max(1),
        usage: desc.usage,
        base_mip: desc.base_mip,
        level_count: desc.mip_count,
        base_layer: desc.base_layer,
        layer_count: desc.layer_count,
        image_mips: mips,
        image_layers: layers,
        cube: false,
        owner: TextureOwner::View,
        allocation: None,
        state: TextureState::Undefined,
        label: String::new(),
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("texture_view", h), str::to_owned);
    s.set_name(view, &name);
    let (sampled, storage) = {
        let mut pool = write(&s.pools.textures);
        pool.get_mut(h).map_or((false, false), |t| {
            t.label.clone_from(&name);
            (t.has_sampled_slot(), t.has_storage_slot())
        })
    };
    if let Err(e) = write_slots(s, h.index(), view, sampled, storage) {
        write(&s.pools.textures).remove(h);
        // SAFETY: the view was never used by a submission.
        unsafe { s.raw.destroy_image_view(view, None) };
        return Err(e);
    }
    tracing::debug!(
        target: "rhi_vulkan::texture", handle = ?h, parent = ?parent, label = %name,
        mips = ?(desc.base_mip, desc.mip_count), layers = ?(desc.base_layer, desc.layer_count),
        usage = ?desc.usage, slot = h.index(), "texture view created"
    );
    Ok(h)
}

/// Bindless storage slot of a single-mip 2D texture or view with `STORAGE` usage.
pub(crate) fn storage_index(s: &DeviceShared, h: TextureHandle) -> Result<u32, RhiError> {
    let pool = read(&s.pools.textures);
    let t = pool.get(h).ok_or_else(|| stale("texture", h))?;
    if !t.has_storage_slot() {
        return Err(invalid(
            "storage_index: needs a plain 2D single-mip texture or view with TextureUsage::STORAGE",
        ));
    }
    Ok(h.index())
}

pub(crate) fn destroy_texture(s: &DeviceShared, h: TextureHandle) {
    let taken = {
        let mut pool = write(&s.pools.textures);
        match pool.get(h) {
            None => None,
            Some(t) if t.owner == TextureOwner::Swapchain => {
                tracing::warn!(target: "rhi_vulkan::texture", handle = ?h, "destroy_texture on swapchain-owned texture ignored");
                return;
            }
            Some(_) => pool.take(h),
        }
    };
    let Some(mut t) = taken else {
        tracing::warn!(target: "rhi_vulkan::texture", handle = ?h, "destroy_texture on stale handle ignored");
        return;
    };
    if t.owner == TextureOwner::View {
        s.sync.defer(Doomed::View {
            view: t.view,
            slot: h,
        });
        tracing::debug!(target: "rhi_vulkan::texture", handle = ?h, label = %t.label, "texture view destroy deferred");
        return;
    }
    let Some(allocation) = t.allocation.take() else {
        return;
    };
    s.sync.defer(Doomed::Image {
        image: t.image,
        view: t.view,
        allocation,
        slot: h,
    });
    tracing::debug!(target: "rhi_vulkan::texture", handle = ?h, label = %t.label, "texture destroy deferred");
}

pub(crate) fn texture_index(s: &DeviceShared, h: TextureHandle) -> Result<u32, RhiError> {
    let pool = read(&s.pools.textures);
    let t = pool.get(h).ok_or_else(|| stale("texture", h))?;
    if t.owner == TextureOwner::Swapchain {
        return Err(invalid(
            "texture_index: swapchain textures have no bindless slot",
        ));
    }
    if !t.has_sampled_slot() {
        return Err(invalid(
            "texture_index: needs a 2D or cube (not array) texture or view with TextureUsage::SAMPLED",
        ));
    }
    Ok(h.index())
}

pub(crate) fn upload_texture(
    s: &DeviceShared,
    h: TextureHandle,
    data: &[u8],
) -> Result<(), RhiError> {
    upload_texture_mips(s, h, &[data], false)
}

/// Queues the upload of `mips` (mip 0 first, each tightly packed). With `all` the texture's whole
/// chain must be given; otherwise only mip 0 is written.
pub(crate) fn upload_texture_mips(
    s: &DeviceShared,
    h: TextureHandle,
    mips: &[&[u8]],
    all: bool,
) -> Result<(), RhiError> {
    let mut pool = write(&s.pools.textures);
    let t = pool.get_mut(h).ok_or_else(|| stale("texture", h))?;
    if t.owner != TextureOwner::Device {
        return Err(invalid(
            "upload_texture: only device textures (not views or swapchain images) can be uploaded to",
        ));
    }
    if t.image_layers != 1 {
        return Err(invalid(
            "upload_texture: layered textures are not supported",
        ));
    }
    if !t.usage.contains(TextureUsage::TRANSFER_DST) {
        return Err(invalid("upload_texture needs TextureUsage::TRANSFER_DST"));
    }
    if t.format.is_depth() {
        return Err(invalid(
            "upload_texture: depth textures cannot be uploaded to",
        ));
    }
    if all && mips.len() as u32 != t.image_mips {
        return Err(invalid(format!(
            "upload_texture_mips: {} levels given, the texture has {}",
            mips.len(),
            t.image_mips
        )));
    }
    let mut regions = Vec::with_capacity(mips.len());
    let mut staged = Vec::new();
    for (level, data) in mips.iter().enumerate() {
        let (w, h2) = ((t.width >> level).max(1), (t.height >> level).max(1));
        check_upload_len(w, h2, t.format, data.len())?;
        regions.push((staged.len() as u64, w, h2));
        staged.extend_from_slice(data);
        // Offsets of a buffer-to-image copy must be a multiple of the texel size (4 for RGBA8).
        staged.resize(staged.len().next_multiple_of(16), 0);
    }
    if !matches!(t.state, TextureState::Undefined | TextureState::ShaderRead) {
        return Err(invalid(format!("upload_texture from state {:?}", t.state)));
    }
    let from = t.state;
    let (staging, allocation) = create_staging(s, &staged, &format!("staging({})", t.label))?;
    lock(&s.pending_uploads).push(PendingUpload::Texture {
        staging,
        allocation,
        image: t.image,
        mips: regions,
        bytes: staged.len() as u64,
        from,
    });
    t.state = TextureState::ShaderRead;
    tracing::trace!(target: "rhi_vulkan::texture", handle = ?h, bytes = staged.len(), levels = mips.len(), from_state = ?from, "upload_texture queued");
    Ok(())
}

// ---------------------------------------------------------------- samplers

pub(crate) fn create_sampler(
    s: &DeviceShared,
    desc: &SamplerDesc,
) -> Result<SamplerHandle, RhiError> {
    let ci = sampler_info(desc);
    // SAFETY: `ci` outlives the call; the device is live.
    let raw =
        unsafe { s.raw.create_sampler(&ci, None) }.map_err(|r| map_vk("vkCreateSampler", r))?;
    let h = lock(&s.pools.samplers).insert(VkSampler {
        raw,
        label: String::new(),
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("sampler", h), str::to_owned);
    s.set_name(raw, &name);
    if let Some(v) = lock(&s.pools.samplers).get_mut(h) {
        v.label.clone_from(&name);
    }
    let slot = h.index();
    if slot >= s.bindings.sampler_capacity {
        tracing::warn!(target: "rhi_vulkan::texture", slot, capacity = s.bindings.sampler_capacity, "bindless sampler capacity exceeded");
        lock(&s.pools.samplers).remove(h);
        // SAFETY: never submitted.
        unsafe { s.raw.destroy_sampler(raw, None) };
        return Err(RhiError::Unsupported {
            what: format!(
                "bindless sampler capacity ({}) exceeded",
                s.bindings.sampler_capacity
            ),
        });
    }
    s.bindings.write_sampler(&s.raw, slot, raw);
    tracing::debug!(
        target: "rhi_vulkan::texture", handle = ?h, label = %name,
        filters = ?(desc.mag_filter, desc.min_filter),
        address = ?(desc.address_u, desc.address_v, desc.address_w), slot, "sampler created"
    );
    Ok(h)
}

pub(crate) fn destroy_sampler(s: &DeviceShared, h: SamplerHandle) {
    let taken = lock(&s.pools.samplers).take(h);
    match taken {
        Some(v) => {
            s.sync.defer(Doomed::Sampler {
                sampler: v.raw,
                slot: h,
            });
            tracing::debug!(target: "rhi_vulkan::texture", handle = ?h, label = %v.label, "sampler destroy deferred");
        }
        None => {
            tracing::warn!(target: "rhi_vulkan::texture", handle = ?h, "destroy_sampler on stale handle ignored");
        }
    }
}

pub(crate) fn sampler_index(s: &DeviceShared, h: SamplerHandle) -> Result<u32, RhiError> {
    lock(&s.pools.samplers)
        .get(h)
        .map(|_| h.index())
        .ok_or_else(|| stale("sampler", h))
}

/// Shutdown: warns about and destroys textures and samplers that were never destroyed.
pub(crate) fn destroy_leaked(s: &DeviceShared) {
    let textures = {
        let mut p = write(&s.pools.textures);
        let hs = p.handles();
        hs.into_iter()
            .filter_map(|h| p.remove(h).map(|v| (h, v)))
            .filter(|(_, t)| t.owner != TextureOwner::Swapchain)
            .collect::<Vec<_>>()
    };
    let (views, textures): (Vec<_>, Vec<_>) = textures
        .into_iter()
        .partition(|(_, t)| t.owner == TextureOwner::View);
    for (h, t) in views.into_iter().chain(textures) {
        let mut t = t;
        tracing::warn!(target: "rhi_vulkan::texture", handle = ?h, label = %t.label, "leaked texture destroyed at shutdown");
        if t.owner == TextureOwner::View {
            // SAFETY: device idle; removed from the pool so destroyed once.
            unsafe { s.raw.destroy_image_view(t.view, None) };
            continue;
        }
        if let Some(allocation) = t.allocation.take() {
            // SAFETY: device idle; removed from the pool so destroyed once. The Doomed release
            // of the (already removed) slot is a no-op.
            unsafe {
                destroy_doomed(
                    s,
                    Doomed::Image {
                        image: t.image,
                        view: t.view,
                        allocation,
                        slot: h,
                    },
                )
            };
        }
    }
    let samplers = {
        let mut p = lock(&s.pools.samplers);
        let hs = p.handles();
        hs.into_iter()
            .filter_map(|h| p.remove(h).map(|v| (h, v)))
            .collect::<Vec<_>>()
    };
    for (h, v) in samplers {
        tracing::warn!(target: "rhi_vulkan::texture", handle = ?h, label = %v.label, "leaked sampler destroyed at shutdown");
        // SAFETY: device idle; removed from the pool so destroyed once.
        unsafe { s.raw.destroy_sampler(v.raw, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(f: vk::FormatFeatureFlags) -> FormatFacts {
        FormatFacts {
            optimal_features: f,
            max_dimension: 4096,
        }
    }

    fn all() -> vk::FormatFeatureFlags {
        vk::FormatFeatureFlags::SAMPLED_IMAGE
            | vk::FormatFeatureFlags::COLOR_ATTACHMENT
            | vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT
            | vk::FormatFeatureFlags::TRANSFER_SRC
            | vk::FormatFeatureFlags::TRANSFER_DST
    }

    fn desc(w: u32, h: u32, format: Format, usage: TextureUsage) -> TextureDesc<'static> {
        TextureDesc {
            label: None,
            width: w,
            height: h,
            format,
            usage,
            mip_levels: 1,
            array_layers: 1,
            cube: false,
        }
    }

    #[test]
    fn format_to_vk_and_aspect() {
        for f in [
            Format::Bgra8Srgb,
            Format::Bgra8Unorm,
            Format::Rgba8Srgb,
            Format::Rgba8Unorm,
            Format::R8Unorm,
            Format::Rg8Unorm,
            Format::Rgba16Float,
            Format::D32Sfloat,
        ] {
            assert_eq!(vk_to_rhi_format(rhi_to_vk_format(f)), Some(f));
            let want = if f == Format::D32Sfloat {
                vk::ImageAspectFlags::DEPTH
            } else {
                vk::ImageAspectFlags::COLOR
            };
            assert_eq!(aspect_of(f), want);
        }
        assert_eq!(rhi_to_vk_format(Format::R8Unorm), vk::Format::R8_UNORM);
        assert_eq!(
            rhi_to_vk_format(Format::Rgba16Float),
            vk::Format::R16G16B16A16_SFLOAT
        );
        assert_eq!(vk_to_rhi_format(vk::Format::UNDEFINED), None);
    }

    #[test]
    fn usage_to_vk_image_usage() {
        use vk::ImageUsageFlags as U;
        assert_eq!(usage_to_vk(TextureUsage::NONE), U::empty());
        assert_eq!(
            usage_to_vk(TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST),
            U::SAMPLED | U::TRANSFER_DST
        );
        assert_eq!(
            usage_to_vk(TextureUsage::DEPTH_ATTACHMENT | TextureUsage::TRANSFER_SRC),
            U::DEPTH_STENCIL_ATTACHMENT | U::TRANSFER_SRC
        );
        assert_eq!(
            usage_to_vk(TextureUsage::COLOR_ATTACHMENT),
            U::COLOR_ATTACHMENT
        );
    }

    #[test]
    fn create_validation_table() {
        let f = facts(all());
        let ok = desc(4, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED);
        assert!(validate_texture_desc(&ok, &f).is_ok());
        let invalid_cases = [
            desc(0, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED),
            desc(4, 0, Format::Rgba8Unorm, TextureUsage::SAMPLED),
            desc(4097, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED),
            desc(4, 4, Format::Rgba8Unorm, TextureUsage::NONE),
            desc(4, 4, Format::D32Sfloat, TextureUsage::COLOR_ATTACHMENT),
            desc(4, 4, Format::Rgba8Unorm, TextureUsage::DEPTH_ATTACHMENT),
        ];
        for d in &invalid_cases {
            assert!(
                matches!(
                    validate_texture_desc(d, &f),
                    Err(RhiError::InvalidDescriptor(_))
                ),
                "{d:?}"
            );
        }
        let sampled_depth = desc(4, 4, Format::D32Sfloat, TextureUsage::SAMPLED);
        assert!(validate_texture_desc(&sampled_depth, &f).is_ok());
        let no_sampled = facts(vk::FormatFeatureFlags::COLOR_ATTACHMENT);
        assert!(matches!(
            validate_texture_desc(&ok, &no_sampled),
            Err(RhiError::Unsupported { .. })
        ));
        let depth = desc(4, 4, Format::D32Sfloat, TextureUsage::DEPTH_ATTACHMENT);
        assert!(validate_texture_desc(&depth, &f).is_ok());
    }

    #[test]
    fn upload_len_check() {
        assert!(check_upload_len(4, 4, Format::Rgba8Unorm, 64).is_ok());
        assert!(check_upload_len(4, 4, Format::Rgba8Unorm, 63).is_err());
        assert!(check_upload_len(4, 4, Format::Rgba16Float, 128).is_ok());
        assert!(check_upload_len(3, 5, Format::R8Unorm, 15).is_ok());
        // 65536 * 65536 * 4 overflows u32 but not u64 and must simply mismatch.
        assert!(check_upload_len(65536, 65536, Format::Rgba8Unorm, 16).is_err());
    }

    #[test]
    fn tracked_state_mismatch_is_warning() {
        assert_eq!(
            check_tracked_state(TextureState::ShaderRead, TextureState::ShaderRead),
            StateCheck::Match
        );
        assert_eq!(
            check_tracked_state(TextureState::ShaderRead, TextureState::Undefined),
            StateCheck::Mismatch {
                tracked: TextureState::ShaderRead
            }
        );
    }

    #[test]
    fn sampler_info_mapping() {
        let d = SamplerDesc {
            label: None,
            mag_filter: Filter::Linear,
            min_filter: Filter::Nearest,
            address_u: AddressMode::Repeat,
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::MirroredRepeat,
            compare: Some(rhi::CompareOp::GreaterOrEqual),
        };
        let i = sampler_info(&d);
        assert_eq!(i.compare_enable, vk::TRUE);
        assert_eq!(i.compare_op, vk::CompareOp::GREATER_OR_EQUAL);
        assert_eq!(i.mag_filter, vk::Filter::LINEAR);
        assert_eq!(i.min_filter, vk::Filter::NEAREST);
        assert_eq!(i.address_mode_u, vk::SamplerAddressMode::REPEAT);
        assert_eq!(i.address_mode_v, vk::SamplerAddressMode::CLAMP_TO_EDGE);
        assert_eq!(i.address_mode_w, vk::SamplerAddressMode::MIRRORED_REPEAT);
        assert_eq!((i.min_lod, i.max_lod), (0.0, 0.0));
        assert_eq!(i.anisotropy_enable, vk::FALSE);
    }

    #[test]
    fn slot_equals_handle_index() {
        let mut r: crate::registry::Registry<rhi::TextureTag, u8> =
            crate::registry::Registry::new(|| rhi::TextureTag);
        let a = r.insert(0);
        let b = r.insert(0);
        assert_eq!((a.index(), b.index()), (0, 1));
    }

    #[test]
    fn high_water_monotonic() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let hw = AtomicU32::new(0);
        for slot in [3u32, 1, 7, 2] {
            hw.fetch_max(slot + 1, Ordering::Relaxed);
        }
        assert_eq!(hw.load(Ordering::Relaxed), 8);
    }
}
