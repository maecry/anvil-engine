//! MODULE-003: surface, swapchain, image views, recreation, acquire and present.

use std::sync::Arc;
use std::time::Instant;

use ash::vk;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rhi::{
    Acquire, Format, PresentStatus, RhiError, SwapchainDesc, SwapchainDiagnostics, SwapchainHandle,
    SwapchainImage, TextureHandle,
};

use crate::device::DeviceShared;
use crate::error::{invalid, map_vk, stale};
use crate::instance::InstanceShared;
use crate::registry::generated_name;
use crate::{lock, write};

// ---------------------------------------------------------------- surface

pub(crate) struct SurfaceInner {
    pub raw: vk::SurfaceKHR,
    _window: Arc<dyn Send + Sync>,
    // Declared last: dropped after the surface is destroyed and the window released.
    pub instance: Arc<InstanceShared>,
}

impl Drop for SurfaceInner {
    fn drop(&mut self) {
        if let Some(sf) = &self.instance.surface_fn {
            // SAFETY: every swapchain on this surface holds an Arc to us, so none remain; the
            // instance is alive (Arc) and the surface is destroyed exactly once.
            unsafe { sf.destroy_surface(self.raw, None) };
            tracing::debug!(surface = ?self.raw, "surface destroyed");
        }
    }
}

/// A window surface. Owns the window `Arc` and keeps the instance alive.
#[derive(Clone)]
pub struct VulkanSurface {
    pub(crate) inner: Arc<SurfaceInner>,
}

pub(crate) fn create_surface<W>(
    inst: &Arc<InstanceShared>,
    window: Arc<W>,
) -> Result<VulkanSurface, RhiError>
where
    W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
{
    if inst.surface_fn.is_none() {
        return Err(RhiError::Unsupported {
            what: "instance created without a display handle".to_owned(),
        });
    }
    let display = window
        .display_handle()
        .map_err(|e| invalid(format!("display handle: {e}")))?
        .as_raw();
    let wh = window
        .window_handle()
        .map_err(|e| invalid(format!("window handle: {e}")))?
        .as_raw();
    // SAFETY: the raw handles come from the live `window` Arc and are used only inside this call;
    // the surface is stored together with that Arc and an Arc to the instance, which was created
    // with the matching display extensions.
    let raw = unsafe { ash_window::create_surface(&inst.entry, &inst.raw, display, wh, None) }
        .map_err(|r| map_vk("vkCreateSurfaceKHR", r))?;
    let s = VulkanSurface {
        inner: Arc::new(SurfaceInner {
            raw,
            _window: window,
            instance: inst.clone(),
        }),
    };
    // ash exposes object naming only on the device loader, so surfaces stay unnamed.
    tracing::debug!(surface = ?raw, "surface created");
    Ok(s)
}

// ---------------------------------------------------------------- pure selection

pub(crate) struct SwapchainConfig {
    pub surface_format: vk::SurfaceFormatKHR,
    pub format: Format,
    pub present_mode: vk::PresentModeKHR,
    pub extent: vk::Extent2D,
    pub image_count: u32,
    pub pre_transform: vk::SurfaceTransformFlagsKHR,
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
}

pub(crate) fn vk_to_rhi_format(f: vk::Format) -> Option<Format> {
    Some(match f {
        vk::Format::B8G8R8A8_SRGB => Format::Bgra8Srgb,
        vk::Format::B8G8R8A8_UNORM => Format::Bgra8Unorm,
        vk::Format::R8G8B8A8_SRGB => Format::Rgba8Srgb,
        vk::Format::R8G8B8A8_UNORM => Format::Rgba8Unorm,
        vk::Format::D32_SFLOAT => Format::D32Sfloat,
        _ => return None,
    })
}

pub(crate) fn rhi_to_vk_format(f: Format) -> vk::Format {
    match f {
        Format::Bgra8Srgb => vk::Format::B8G8R8A8_SRGB,
        Format::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        Format::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        Format::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        Format::D32Sfloat => vk::Format::D32_SFLOAT,
    }
}

pub(crate) fn choose_surface_format(
    available: &[vk::SurfaceFormatKHR],
    preferred: Format,
) -> Option<(vk::SurfaceFormatKHR, Format)> {
    let cands: Vec<_> = available
        .iter()
        .filter(|f| f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        .filter_map(|f| {
            vk_to_rhi_format(f.format)
                .filter(|r| !r.is_depth())
                .map(|r| (*f, r))
        })
        .collect();
    [preferred, Format::Bgra8Srgb, Format::Rgba8Srgb]
        .iter()
        .find_map(|want| cands.iter().find(|(_, r)| r == want))
        .or(cands.first())
        .copied()
}

pub(crate) fn choose_present_mode(
    available: &[vk::PresentModeKHR],
    vsync: bool,
) -> vk::PresentModeKHR {
    if vsync {
        return vk::PresentModeKHR::FIFO;
    }
    [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|m| available.contains(m))
        .unwrap_or(vk::PresentModeKHR::FIFO)
}

pub(crate) fn choose_extent(
    caps: &vk::SurfaceCapabilitiesKHR,
    requested: (u32, u32),
) -> vk::Extent2D {
    if caps.current_extent.width != u32::MAX {
        return caps.current_extent;
    }
    vk::Extent2D {
        width: requested
            .0
            .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
        height: requested
            .1
            .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
    }
}

pub(crate) fn choose_image_count(caps: &vk::SurfaceCapabilitiesKHR) -> u32 {
    let n = caps.min_image_count + 1;
    if caps.max_image_count != 0 {
        n.min(caps.max_image_count)
    } else {
        n
    }
}

pub(crate) fn choose_composite_alpha(
    caps: &vk::SurfaceCapabilitiesKHR,
) -> Option<vk::CompositeAlphaFlagsKHR> {
    [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ]
    .into_iter()
    .find(|a| caps.supported_composite_alpha.contains(*a))
}

pub(crate) fn choose_config(
    caps: &vk::SurfaceCapabilitiesKHR,
    formats: &[vk::SurfaceFormatKHR],
    modes: &[vk::PresentModeKHR],
    requested: (u32, u32),
    vsync: bool,
    preferred: Format,
) -> Result<SwapchainConfig, RhiError> {
    let (surface_format, format) =
        choose_surface_format(formats, preferred).ok_or_else(|| RhiError::Unsupported {
            what: "surface format".to_owned(),
        })?;
    if !format.is_srgb() {
        tracing::info!(format = ?format, "swapchain format is not sRGB");
    }
    if !caps
        .supported_usage_flags
        .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
    {
        return Err(RhiError::Unsupported {
            what: "swapchain COLOR_ATTACHMENT usage".to_owned(),
        });
    }
    let composite_alpha = choose_composite_alpha(caps).ok_or_else(|| RhiError::Unsupported {
        what: "swapchain composite alpha".to_owned(),
    })?;
    Ok(SwapchainConfig {
        surface_format,
        format,
        present_mode: choose_present_mode(modes, vsync),
        extent: choose_extent(caps, requested),
        image_count: choose_image_count(caps),
        pre_transform: caps.current_transform,
        composite_alpha,
    })
}

pub(crate) fn present_mode_name(m: vk::PresentModeKHR) -> &'static str {
    match m {
        vk::PresentModeKHR::FIFO => "FIFO",
        vk::PresentModeKHR::MAILBOX => "MAILBOX",
        vk::PresentModeKHR::IMMEDIATE => "IMMEDIATE",
        vk::PresentModeKHR::FIFO_RELAXED => "FIFO_RELAXED",
        _ => "OTHER",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AcquireOutcome {
    Image { index: u32, suboptimal: bool },
    OutOfDate,
}

pub(crate) fn classify_acquire(
    r: Result<(u32, bool), vk::Result>,
) -> Result<AcquireOutcome, RhiError> {
    match r {
        Ok((index, suboptimal)) => Ok(AcquireOutcome::Image { index, suboptimal }),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(AcquireOutcome::OutOfDate),
        Err(e) => Err(map_vk("vkAcquireNextImageKHR", e)),
    }
}

pub(crate) fn classify_present(r: Result<bool, vk::Result>) -> Result<PresentStatus, RhiError> {
    match r {
        Ok(false) => Ok(PresentStatus::Ok),
        Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(PresentStatus::Recreate),
        Err(e) => Err(map_vk("vkQueuePresentKHR", e)),
    }
}

// ---------------------------------------------------------------- objects

pub(crate) struct VkTexture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: Format,
}

pub(crate) struct VkSwapchain {
    pub raw: vk::SwapchainKHR,
    pub surface: VulkanSurface,
    pub config: SwapchainConfig,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub textures: Vec<TextureHandle>,
    pub render_done: Vec<vk::Semaphore>,
    pub vsync: bool,
    pub desired: (u32, u32),
    pub minimized: bool,
    pub broken: bool,
    pub recreations: u64,
}

/// Native objects of one swapchain generation, used for incremental build and teardown.
#[derive(Default)]
struct Built {
    raw: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    render_done: Vec<vk::Semaphore>,
    textures: Vec<TextureHandle>,
}

impl Built {
    /// Destroy order: texture entries, semaphores, views, swapchain.
    fn destroy(self, s: &DeviceShared) {
        {
            let mut t = write(&s.pools.textures);
            for h in &self.textures {
                t.remove(*h);
            }
        }
        // SAFETY: the caller made the device idle (or nothing was ever submitted); each object is
        // owned by this struct and destroyed once; views go before their swapchain.
        unsafe {
            for sem in &self.render_done {
                s.raw.destroy_semaphore(*sem, None);
            }
            for v in &self.views {
                s.raw.destroy_image_view(*v, None);
            }
            if self.raw != vk::SwapchainKHR::null() {
                s.swapchain_fn.destroy_swapchain(self.raw, None);
            }
        }
    }
}

fn query(
    s: &DeviceShared,
    surface: &VulkanSurface,
) -> Result<
    (
        vk::SurfaceCapabilitiesKHR,
        Vec<vk::SurfaceFormatKHR>,
        Vec<vk::PresentModeKHR>,
    ),
    RhiError,
> {
    let sf = surface
        .inner
        .instance
        .surface_fn
        .as_ref()
        .ok_or_else(|| RhiError::Unsupported {
            what: "instance created without a display handle".to_owned(),
        })?;
    let (pd, raw) = (s.physical, surface.inner.raw);
    // SAFETY: `pd` and `raw` belong to the same live instance (checked by callers).
    unsafe {
        Ok((
            sf.get_physical_device_surface_capabilities(pd, raw)
                .map_err(|r| map_vk("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", r))?,
            sf.get_physical_device_surface_formats(pd, raw)
                .map_err(|r| map_vk("vkGetPhysicalDeviceSurfaceFormatsKHR", r))?,
            sf.get_physical_device_surface_present_modes(pd, raw)
                .map_err(|r| map_vk("vkGetPhysicalDeviceSurfacePresentModesKHR", r))?,
        ))
    }
}

fn build(
    s: &DeviceShared,
    surface: &VulkanSurface,
    cfg: &SwapchainConfig,
    old: vk::SwapchainKHR,
) -> Result<Built, RhiError> {
    let ci = vk::SwapchainCreateInfoKHR::default()
        .surface(surface.inner.raw)
        .min_image_count(cfg.image_count)
        .image_format(cfg.surface_format.format)
        .image_color_space(cfg.surface_format.color_space)
        .image_extent(cfg.extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(cfg.pre_transform)
        .composite_alpha(cfg.composite_alpha)
        .present_mode(cfg.present_mode)
        .clipped(true)
        .old_swapchain(old);
    // SAFETY: surface and device share an instance and the queue family can present (checked by
    // callers); `old` is null or a swapchain of this device and surface; `ci` outlives the call.
    let raw = unsafe { s.swapchain_fn.create_swapchain(&ci, None) }
        .map_err(|r| map_vk("vkCreateSwapchainKHR", r))?;
    let mut b = Built {
        raw,
        ..Built::default()
    };
    let r = (|| -> Result<(), RhiError> {
        // SAFETY: `raw` was just created by this loader.
        b.images = unsafe { s.swapchain_fn.get_swapchain_images(raw) }
            .map_err(|r| map_vk("vkGetSwapchainImagesKHR", r))?;
        for &image in &b.images {
            let vi = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(cfg.surface_format.format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            // SAFETY: `image` belongs to the swapchain that is alive; views are destroyed first.
            b.views.push(
                unsafe { s.raw.create_image_view(&vi, None) }
                    .map_err(|r| map_vk("vkCreateImageView", r))?,
            );
            // SAFETY: plain object creation on a live device.
            b.render_done.push(
                unsafe {
                    s.raw
                        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                }
                .map_err(|r| map_vk("vkCreateSemaphore", r))?,
            );
        }
        Ok(())
    })();
    if let Err(e) = r {
        b.destroy(s);
        return Err(e);
    }
    let mut t = write(&s.pools.textures);
    for (&image, &view) in b.images.iter().zip(&b.views) {
        b.textures.push(t.insert(VkTexture {
            image,
            view,
            format: cfg.format,
        }));
    }
    Ok(b)
}

fn apply_names(s: &DeviceShared, h: SwapchainHandle, sc: &VkSwapchain) {
    let base = generated_name("swapchain", h);
    s.set_name(sc.raw, &base);
    for (i, ((img, view), sem)) in sc
        .images
        .iter()
        .zip(&sc.views)
        .zip(&sc.render_done)
        .enumerate()
    {
        s.set_name(*img, &format!("{base}.image{i}"));
        s.set_name(*view, &format!("{base}.view{i}"));
        s.set_name(*sem, &format!("{base}.render_done{i}"));
    }
}

fn log_state(msg: &'static str, h: SwapchainHandle, sc: &VkSwapchain, ms: f64) {
    tracing::info!(
        handle = ?h,
        width = sc.config.extent.width,
        height = sc.config.extent.height,
        format = ?sc.config.format,
        present_mode = present_mode_name(sc.config.present_mode),
        image_count = sc.images.len(),
        recreations = sc.recreations,
        elapsed_ms = ms,
        "{msg}"
    );
}

// ---------------------------------------------------------------- Device operations

pub(crate) fn create(
    s: &DeviceShared,
    surface: &VulkanSurface,
    desc: &SwapchainDesc,
) -> Result<SwapchainHandle, RhiError> {
    let started = Instant::now();
    if desc.width == 0 || desc.height == 0 {
        return Err(invalid("swapchain size must be nonzero"));
    }
    if !Arc::ptr_eq(&surface.inner.instance, &s.instance) {
        return Err(invalid("surface belongs to another instance"));
    }
    let sf = surface
        .inner
        .instance
        .surface_fn
        .as_ref()
        .ok_or_else(|| RhiError::Unsupported {
            what: "instance created without a display handle".to_owned(),
        })?;
    // SAFETY: physical device, family and surface belong to this instance.
    let can_present = unsafe {
        sf.get_physical_device_surface_support(s.physical, s.queue_family, surface.inner.raw)
    }
    .map_err(|r| map_vk("vkGetPhysicalDeviceSurfaceSupportKHR", r))?;
    if !can_present {
        return Err(RhiError::Unsupported {
            what: "queue family cannot present to this surface".to_owned(),
        });
    }
    let (caps, formats, modes) = query(s, surface)?;
    let cfg = choose_config(
        &caps,
        &formats,
        &modes,
        (desc.width, desc.height),
        desc.vsync,
        desc.preferred_format,
    )?;
    if cfg.extent.width == 0 || cfg.extent.height == 0 {
        return Err(invalid(
            "surface extent is zero (window minimized); create the swapchain when visible",
        ));
    }
    let built = build(s, surface, &cfg, vk::SwapchainKHR::null())?;
    let sc = VkSwapchain {
        raw: built.raw,
        surface: surface.clone(),
        config: cfg,
        images: built.images,
        views: built.views,
        textures: built.textures,
        render_done: built.render_done,
        vsync: desc.vsync,
        desired: (desc.width, desc.height),
        minimized: false,
        broken: false,
        recreations: 0,
    };
    let mut pool = lock(&s.pools.swapchains);
    let h = pool.insert(sc);
    let sc = pool.get(h).ok_or_else(|| stale("swapchain", h))?;
    apply_names(s, h, sc);
    log_state(
        "swapchain created",
        h,
        sc,
        started.elapsed().as_secs_f64() * 1000.0,
    );
    tracing::debug!(handle = ?h, label = %generated_name("swapchain", h), "swapchain registered");
    Ok(h)
}

pub(crate) fn resize(
    s: &DeviceShared,
    h: SwapchainHandle,
    w: u32,
    ht: u32,
) -> Result<(), RhiError> {
    let started = Instant::now();
    let mut pool = lock(&s.pools.swapchains);
    let sc = pool.get_mut(h).ok_or_else(|| stale("swapchain", h))?;
    if sc.broken {
        return Err(RhiError::Backend {
            op: "swapchain broken",
            code: 0,
        });
    }
    sc.desired = (w, ht);
    if w == 0 || ht == 0 {
        sc.minimized = true;
        tracing::debug!(handle = ?h, "swapchain minimized (zero size)");
        return Ok(());
    }
    let (caps, formats, modes) = query(s, &sc.surface)?;
    let cfg = choose_config(&caps, &formats, &modes, (w, ht), sc.vsync, sc.config.format)?;
    if cfg.extent.width == 0 || cfg.extent.height == 0 {
        sc.minimized = true;
        return Ok(());
    }
    if cfg.format != sc.config.format {
        tracing::warn!(old = ?sc.config.format, new = ?cfg.format, "swapchain format changed on recreate; pipelines must be rebuilt");
    }
    // shortcut: wait_idle on recreate, replace with retire-old-swapchain in M2
    s.wait_idle()?;
    let new = build(s, &sc.surface, &cfg, sc.raw)?;
    let old = Built {
        raw: sc.raw,
        images: std::mem::take(&mut sc.images),
        views: std::mem::take(&mut sc.views),
        render_done: std::mem::take(&mut sc.render_done),
        textures: std::mem::take(&mut sc.textures),
    };
    old.destroy(s);
    sc.raw = new.raw;
    sc.images = new.images;
    sc.views = new.views;
    sc.render_done = new.render_done;
    sc.textures = new.textures;
    sc.config = cfg;
    sc.minimized = false;
    sc.recreations += 1;
    let sc = &*sc;
    apply_names(s, h, sc);
    log_state(
        "swapchain recreated",
        h,
        sc,
        started.elapsed().as_secs_f64() * 1000.0,
    );
    Ok(())
}

pub(crate) fn format(s: &DeviceShared, h: SwapchainHandle) -> Result<Format, RhiError> {
    lock(&s.pools.swapchains)
        .get(h)
        .map(|sc| sc.config.format)
        .ok_or_else(|| stale("swapchain", h))
}

pub(crate) fn destroy(s: &DeviceShared, h: SwapchainHandle, wait: bool) {
    let Some(sc) = lock(&s.pools.swapchains).remove(h) else {
        tracing::warn!(handle = ?h, "destroy_swapchain on stale handle ignored");
        return;
    };
    if wait {
        // shortcut: wait_idle on destroy, replace with deferred destruction in M2
        if let Err(e) = s.wait_idle() {
            tracing::error!(error = %e, "wait_idle before swapchain destroy failed");
        }
    }
    Built {
        raw: sc.raw,
        images: sc.images,
        views: sc.views,
        render_done: sc.render_done,
        textures: sc.textures,
    }
    .destroy(s);
    tracing::debug!(handle = ?h, label = %generated_name("swapchain", h), "swapchain destroyed");
}

pub(crate) fn diagnostics(s: &DeviceShared) -> Vec<SwapchainDiagnostics> {
    let pool = lock(&s.pools.swapchains);
    pool.handles()
        .into_iter()
        .filter_map(|h| {
            pool.get(h).map(|sc| SwapchainDiagnostics {
                handle: h,
                extent: (sc.config.extent.width, sc.config.extent.height),
                format: sc.config.format,
                image_count: sc.images.len() as u32,
                present_mode: present_mode_name(sc.config.present_mode).to_owned(),
                recreations: sc.recreations,
            })
        })
        .collect()
}

pub(crate) fn acquire(
    s: &DeviceShared,
    h: SwapchainHandle,
    sem: vk::Semaphore,
) -> Result<Acquire, RhiError> {
    let pool = lock(&s.pools.swapchains);
    let sc = pool.get(h).ok_or_else(|| stale("swapchain", h))?;
    if sc.broken {
        return Err(RhiError::Backend {
            op: "swapchain broken",
            code: 0,
        });
    }
    if sc.minimized {
        return Ok(Acquire::Minimized);
    }
    // SAFETY: the semaphore is unsignalled with no pending wait (the slot's timeline wait in
    // begin_frame proved the previous use completed); externally synchronized by `&mut` device
    // access plus the pool lock.
    let r = unsafe {
        s.swapchain_fn
            .acquire_next_image(sc.raw, u64::MAX, sem, vk::Fence::null())
    };
    match classify_acquire(r)? {
        // The acquire semaphore was not signalled; the frame is abandoned without submit.
        AcquireOutcome::OutOfDate => Ok(Acquire::OutOfDate),
        AcquireOutcome::Image { index, suboptimal } => {
            if suboptimal {
                tracing::debug!(handle = ?h, index, "acquire reported suboptimal");
            }
            Ok(Acquire::Image(SwapchainImage {
                swapchain: h,
                texture: sc.textures[index as usize],
                index,
                extent: (sc.config.extent.width, sc.config.extent.height),
                format: sc.config.format,
            }))
        }
    }
}

pub(crate) fn render_done(
    s: &DeviceShared,
    image: &SwapchainImage,
) -> Result<vk::Semaphore, RhiError> {
    let pool = lock(&s.pools.swapchains);
    let sc = pool
        .get(image.swapchain)
        .ok_or_else(|| stale("swapchain", image.swapchain))?;
    match sc.textures.get(image.index as usize) {
        Some(t) if *t == image.texture => Ok(sc.render_done[image.index as usize]),
        _ => Err(stale("texture", image.texture)),
    }
}

pub(crate) fn present(
    s: &DeviceShared,
    h: SwapchainHandle,
    image: &SwapchainImage,
) -> Result<PresentStatus, RhiError> {
    if image.swapchain != h {
        return Err(invalid("image belongs to a different swapchain"));
    }
    let pool = lock(&s.pools.swapchains);
    let sc = pool.get(h).ok_or_else(|| stale("swapchain", h))?;
    let sem = *sc
        .render_done
        .get(image.index as usize)
        .ok_or_else(|| invalid("image index out of range"))?;
    let waits = [sem];
    let swapchains = [sc.raw];
    let indices = [image.index];
    let info = vk::PresentInfoKHR::default()
        .wait_semaphores(&waits)
        .swapchains(&swapchains)
        .image_indices(&indices);
    let q = lock(&s.queue);
    // SAFETY: queue mutex held; the wait semaphore was signalled by a queued submit; the index
    // came from the acquire of this swapchain in this frame.
    let r = unsafe { s.swapchain_fn.queue_present(*q, &info) };
    drop(q);
    classify_present(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vk::{ColorSpaceKHR as C, Format as F, PresentModeKHR as P};

    fn sf(format: F, color_space: C) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format,
            color_space,
        }
    }
    const S: C = C::SRGB_NONLINEAR;

    #[test]
    fn format_rules() {
        let all = [
            sf(F::R8G8B8A8_UNORM, S),
            sf(F::R8G8B8A8_SRGB, S),
            sf(F::B8G8R8A8_SRGB, S),
        ];
        assert_eq!(
            choose_surface_format(&all, Format::Rgba8Unorm).unwrap().1,
            Format::Rgba8Unorm
        );
        assert_eq!(
            choose_surface_format(&all, Format::Bgra8Unorm).unwrap().1,
            Format::Bgra8Srgb
        );
        assert_eq!(
            choose_surface_format(&all[..2], Format::Bgra8Unorm)
                .unwrap()
                .1,
            Format::Rgba8Srgb
        );
        assert_eq!(
            choose_surface_format(&all[..1], Format::Bgra8Srgb)
                .unwrap()
                .1,
            Format::Rgba8Unorm
        );
        let hdr = [
            sf(F::B8G8R8A8_SRGB, C::HDR10_ST2084_EXT),
            sf(F::B8G8R8A8_UNORM, S),
        ];
        assert_eq!(
            choose_surface_format(&hdr, Format::Bgra8Srgb).unwrap().1,
            Format::Bgra8Unorm
        );
        assert!(
            choose_surface_format(&[sf(F::A2B10G10R10_UNORM_PACK32, S)], Format::Bgra8Srgb)
                .is_none()
        );
    }

    #[test]
    fn present_modes() {
        let all = [P::FIFO, P::MAILBOX, P::IMMEDIATE];
        assert_eq!(choose_present_mode(&all, true), P::FIFO);
        assert_eq!(choose_present_mode(&all, false), P::MAILBOX);
        assert_eq!(
            choose_present_mode(
                &all[..1]
                    .iter()
                    .chain(&all[2..])
                    .copied()
                    .collect::<Vec<_>>(),
                false
            ),
            P::IMMEDIATE
        );
        assert_eq!(choose_present_mode(&[P::FIFO], false), P::FIFO);
        assert_eq!(present_mode_name(P::FIFO_RELAXED), "FIFO_RELAXED");
        assert_eq!(present_mode_name(P::MAILBOX), "MAILBOX");
        assert_eq!(present_mode_name(P::IMMEDIATE), "IMMEDIATE");
    }

    fn caps() -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            min_image_count: 2,
            max_image_count: 0,
            current_extent: vk::Extent2D {
                width: u32::MAX,
                height: u32::MAX,
            },
            min_image_extent: vk::Extent2D {
                width: 16,
                height: 16,
            },
            max_image_extent: vk::Extent2D {
                width: 4096,
                height: 4096,
            },
            supported_usage_flags: vk::ImageUsageFlags::COLOR_ATTACHMENT,
            supported_composite_alpha: vk::CompositeAlphaFlagsKHR::INHERIT,
            ..Default::default()
        }
    }

    #[test]
    fn counts_extent_alpha_usage() {
        let mut c = caps();
        assert_eq!(choose_image_count(&c), 3);
        c.min_image_count = 3;
        c.max_image_count = 3;
        assert_eq!(choose_image_count(&c), 3);
        let e = choose_extent(&c, (1, 99999));
        assert_eq!((e.width, e.height), (16, 4096));
        c.current_extent = vk::Extent2D {
            width: 800,
            height: 600,
        };
        assert_eq!(choose_extent(&c, (1, 1)).width, 800);
        assert_eq!(
            choose_composite_alpha(&c),
            Some(vk::CompositeAlphaFlagsKHR::INHERIT)
        );
        c.supported_composite_alpha =
            vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::INHERIT;
        assert_eq!(
            choose_composite_alpha(&c),
            Some(vk::CompositeAlphaFlagsKHR::OPAQUE)
        );
        c.supported_composite_alpha = vk::CompositeAlphaFlagsKHR::empty();
        assert_eq!(choose_composite_alpha(&c), None);
        let mut c = caps();
        c.supported_usage_flags = vk::ImageUsageFlags::SAMPLED;
        let fm = [sf(F::B8G8R8A8_SRGB, S)];
        assert!(matches!(
            choose_config(&c, &fm, &[P::FIFO], (100, 100), true, Format::Bgra8Srgb),
            Err(RhiError::Unsupported { .. })
        ));
        assert!(
            choose_config(
                &caps(),
                &fm,
                &[P::FIFO],
                (100, 100),
                true,
                Format::Bgra8Srgb
            )
            .is_ok()
        );
    }

    #[test]
    fn classify_tables() {
        use vk::Result as R;
        assert_eq!(
            classify_acquire(Ok((1, false))).unwrap(),
            AcquireOutcome::Image {
                index: 1,
                suboptimal: false
            }
        );
        assert_eq!(
            classify_acquire(Ok((2, true))).unwrap(),
            AcquireOutcome::Image {
                index: 2,
                suboptimal: true
            }
        );
        assert_eq!(
            classify_acquire(Err(R::ERROR_OUT_OF_DATE_KHR)).unwrap(),
            AcquireOutcome::OutOfDate
        );
        assert!(matches!(
            classify_acquire(Err(R::ERROR_SURFACE_LOST_KHR)),
            Err(RhiError::SurfaceLost)
        ));
        assert!(matches!(
            classify_acquire(Err(R::ERROR_DEVICE_LOST)),
            Err(RhiError::DeviceLost)
        ));
        assert!(matches!(
            classify_acquire(Err(R::TIMEOUT)),
            Err(RhiError::Backend {
                op: "vkAcquireNextImageKHR",
                ..
            })
        ));
        assert_eq!(classify_present(Ok(false)).unwrap(), PresentStatus::Ok);
        assert_eq!(classify_present(Ok(true)).unwrap(), PresentStatus::Recreate);
        assert_eq!(
            classify_present(Err(R::ERROR_OUT_OF_DATE_KHR)).unwrap(),
            PresentStatus::Recreate
        );
        assert!(matches!(
            classify_present(Err(R::ERROR_SURFACE_LOST_KHR)),
            Err(RhiError::SurfaceLost)
        ));
        assert!(matches!(
            classify_present(Err(R::ERROR_DEVICE_LOST)),
            Err(RhiError::DeviceLost)
        ));
    }

    #[test]
    fn format_mapping_roundtrip() {
        for f in [
            Format::Bgra8Srgb,
            Format::Bgra8Unorm,
            Format::Rgba8Srgb,
            Format::Rgba8Unorm,
        ] {
            assert_eq!(vk_to_rhi_format(rhi_to_vk_format(f)), Some(f));
        }
    }
}
