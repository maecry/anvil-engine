//! MODULE-001: instance, validation layer, debug messenger routed to `tracing`.

use std::borrow::Cow;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use ash::{ext, khr, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rhi::{DeviceDesc, InstanceDesc, RhiError, ValidationCounts, ValidationMode};

use crate::device::VulkanDevice;
use crate::error::{invalid, map_vk};
use crate::swapchain::VulkanSurface;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// Validation message counters shared with the debug callback.
#[derive(Default)]
pub(crate) struct Counters {
    errors: AtomicU64,
    warnings: AtomicU64,
    infos: AtomicU64,
    /// `ANVIL_VALIDATION_PANIC=1`: arm `panic_pending` on every error message.
    panic_on_error: bool,
    /// Panicking inside the FFI callback is UB, so the callback only sets this flag and the next
    /// Rust-side call (`begin_frame`, `submit`) panics.
    panic_pending: AtomicBool,
}

pub(crate) struct InstanceCaps {
    pub validation_active: bool,
    pub sync_validation_active: bool,
}

pub(crate) struct InstanceShared {
    pub entry: ash::Entry,
    pub raw: ash::Instance,
    pub surface_fn: Option<khr::surface::Instance>,
    pub debug_utils: Option<ext::debug_utils::Instance>,
    messenger: Option<vk::DebugUtilsMessengerEXT>,
    // Declared after `raw`; the callback holds a raw pointer into this Arc, so it is only
    // released after the messenger is destroyed (`Drop::drop` runs before field drops).
    pub counters: Arc<Counters>,
    pub caps: InstanceCaps,
}

impl InstanceShared {
    pub(crate) fn counts(&self) -> ValidationCounts {
        let c = &self.counters;
        ValidationCounts {
            errors: c.errors.load(Ordering::Relaxed),
            warnings: c.warnings.load(Ordering::Relaxed),
            infos: c.infos.load(Ordering::Relaxed),
        }
    }

    /// Panics if `ANVIL_VALIDATION_PANIC=1` and a validation error arrived since the last check.
    pub(crate) fn check_validation_panic(&self) {
        if self.counters.panic_pending.swap(false, Ordering::AcqRel) {
            panic!(
                "validation error reported (ANVIL_VALIDATION_PANIC=1); see log target vulkan::validation"
            );
        }
    }
}

impl Drop for InstanceShared {
    fn drop(&mut self) {
        // SAFETY: every child (surface, device) holds an Arc to this struct, so none exist any
        // more; the messenger is destroyed before the instance and exactly once (`take`).
        unsafe {
            if let (Some(du), Some(m)) = (&self.debug_utils, self.messenger.take()) {
                du.destroy_debug_utils_messenger(m, None);
            }
            self.raw.destroy_instance(None);
        }
        tracing::debug!("instance destroyed");
    }
}

/// Vulkan instance; cheap to clone, keeps the native instance alive.
#[derive(Clone)]
pub struct VulkanInstance {
    pub(crate) shared: Arc<InstanceShared>,
}

// ---------------------------------------------------------------- pure planning

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ValidationPlan {
    pub enable_layer: bool,
    pub warn_missing: bool,
    pub sync: bool,
}

pub(crate) fn resolve_validation(
    mode: ValidationMode,
    debug_build: bool,
    layer_present: bool,
    sync_requested: bool,
) -> Result<ValidationPlan, RhiError> {
    let (enable_layer, warn_missing) = match (mode, layer_present) {
        (ValidationMode::Off, _) => (false, false),
        (ValidationMode::On, true) => (true, false),
        (ValidationMode::On, false) => {
            return Err(RhiError::Unsupported {
                what: "VK_LAYER_KHRONOS_validation".to_owned(),
            });
        }
        (ValidationMode::Auto, _) if !debug_build => (false, false),
        (ValidationMode::Auto, true) => (true, false),
        (ValidationMode::Auto, false) => (false, true),
    };
    Ok(ValidationPlan {
        enable_layer,
        warn_missing,
        sync: enable_layer && sync_requested,
    })
}

pub(crate) fn check_instance_version(v: Option<u32>) -> Result<u32, RhiError> {
    match v {
        Some(v) if v >= vk::API_VERSION_1_3 => Ok(v),
        _ => Err(RhiError::Unsupported {
            what: "Vulkan instance >= 1.3".to_owned(),
        }),
    }
}

/// Returns required then optional-and-available names. Names come from the input slices.
pub(crate) fn plan_extensions<'a>(
    available: &[&CStr],
    required: &[&'a CStr],
    optional: &[&'a CStr],
) -> Result<Vec<&'a CStr>, RhiError> {
    let mut out = Vec::new();
    for r in required {
        if !available.contains(r) {
            return Err(RhiError::Unsupported {
                what: r.to_string_lossy().into_owned(),
            });
        }
        out.push(*r);
    }
    for o in optional {
        if available.contains(o) && !out.contains(o) {
            out.push(*o);
        }
    }
    Ok(out)
}

pub(crate) fn severity_to_level(s: vk::DebugUtilsMessageSeverityFlagsEXT) -> tracing::Level {
    use vk::DebugUtilsMessageSeverityFlagsEXT as S;
    if s.contains(S::ERROR) {
        tracing::Level::ERROR
    } else if s.contains(S::WARNING) {
        tracing::Level::WARN
    } else if s.contains(S::INFO) {
        tracing::Level::DEBUG
    } else {
        tracing::Level::TRACE
    }
}

// ---------------------------------------------------------------- debug callback

fn cstr_lossy<'a>(p: *const c_char) -> Cow<'a, str> {
    if p.is_null() {
        Cow::Borrowed("")
    } else {
        // SAFETY: non-null pointer from the driver-owned callback data, NUL-terminated and valid
        // for the duration of the callback.
        unsafe { CStr::from_ptr(p) }.to_string_lossy()
    }
}

unsafe extern "system" fn debug_callback(
    sev: vk::DebugUtilsMessageSeverityFlagsEXT,
    _ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void,
) -> vk::Bool32 {
    // Never unwind across FFI.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if data.is_null() || user.is_null() {
            return;
        }
        // SAFETY: `user` is `Arc::as_ptr(counters)` and the messenger is destroyed before the Arc
        // is released, so it is live; `data` is non-null and valid for this call.
        let (counters, d) = unsafe { (&*user.cast::<Counters>(), &*data) };
        let level = severity_to_level(sev);
        let name = cstr_lossy(d.p_message_id_name);
        let msg = cstr_lossy(d.p_message);
        let objects = if d.p_objects.is_null() || d.object_count == 0 {
            &[]
        } else {
            // SAFETY: the driver guarantees `object_count` valid entries at `p_objects`.
            unsafe { std::slice::from_raw_parts(d.p_objects, d.object_count as usize) }
        };
        let objs = objects
            .iter()
            .map(|o| {
                if o.p_object_name.is_null() {
                    format!("{:?}:{:#x}", o.object_type, o.object_handle)
                } else {
                    cstr_lossy(o.p_object_name).into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        macro_rules! emit {
            ($lvl:ident, $sev:literal, $counter:expr) => {{
                $counter.fetch_add(1, Ordering::Relaxed);
                tracing::$lvl!(
                    target: "vulkan::validation",
                    severity = $sev,
                    message_id = %name,
                    message = %msg,
                    objects = %objs,
                );
            }};
        }
        match level {
            tracing::Level::ERROR => {
                emit!(error, "error", counters.errors);
                if counters.panic_on_error {
                    counters.panic_pending.store(true, Ordering::Release);
                }
            }
            tracing::Level::WARN => emit!(warn, "warning", counters.warnings),
            tracing::Level::DEBUG => emit!(debug, "info", counters.infos),
            _ => emit!(trace, "verbose", counters.infos),
        }
    }));
    vk::FALSE
}

fn messenger_info(user: *mut c_void) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    use vk::DebugUtilsMessageSeverityFlagsEXT as S;
    use vk::DebugUtilsMessageTypeFlagsEXT as T;
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(S::ERROR | S::WARNING | S::INFO | S::VERBOSE)
        .message_type(T::GENERAL | T::VALIDATION | T::PERFORMANCE)
        .pfn_user_callback(Some(debug_callback))
        .user_data(user)
}

fn name_set(props: &[vk::ExtensionProperties]) -> Vec<&CStr> {
    props
        .iter()
        .filter_map(|p| p.extension_name_as_c_str().ok())
        .collect()
}

// ---------------------------------------------------------------- creation

impl VulkanInstance {
    pub(crate) fn new(desc: &InstanceDesc) -> Result<Self, RhiError> {
        let started = Instant::now();
        let app_name = CString::new(desc.app_name.as_str())
            .map_err(|_| invalid("app_name contains an interior NUL"))?;

        // SAFETY: loading libvulkan runs foreign initialisers; the Entry is stored in
        // InstanceShared and dropped after the instance, so it outlives every object created.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| RhiError::Unsupported {
            what: format!("Vulkan loader (libvulkan): {e}"),
        })?;
        // SAFETY: plain query on a loaded entry.
        let version = unsafe { entry.try_enumerate_instance_version() }
            .map_err(|r| map_vk("vkEnumerateInstanceVersion", r))?;
        let api_version = check_instance_version(version)?;

        // SAFETY: plain queries on a loaded entry.
        let layers = unsafe { entry.enumerate_instance_layer_properties() }
            .map_err(|r| map_vk("vkEnumerateInstanceLayerProperties", r))?;
        let layer_present = layers
            .iter()
            .any(|l| l.layer_name_as_c_str().is_ok_and(|n| n == VALIDATION_LAYER));
        let plan = resolve_validation(
            desc.validation,
            cfg!(debug_assertions),
            layer_present,
            desc.sync_validation && !desc.gpu_av,
        )?;
        let gpu_av = desc.gpu_av && plan.enable_layer;
        if plan.warn_missing {
            tracing::warn!(
                layer = "VK_LAYER_KHRONOS_validation",
                "validation layer not installed; continuing without validation"
            );
        }

        // SAFETY: plain queries on a loaded entry; `Some(layer)` names a layer found above.
        let mut ext_props = unsafe { entry.enumerate_instance_extension_properties(None) }
            .map_err(|r| map_vk("vkEnumerateInstanceExtensionProperties", r))?;
        if plan.enable_layer {
            // SAFETY: as above.
            let more =
                unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER)) };
            if let Ok(more) = more {
                ext_props.extend(more);
            }
        }
        let available = name_set(&ext_props);

        let mut required: Vec<&'static CStr> = Vec::new();
        if let Some(display) = desc.display {
            let names = ash_window::enumerate_required_extensions(display)
                .map_err(|r| map_vk("ash_window::enumerate_required_extensions", r))?;
            for &p in names {
                // SAFETY: ash-window returns pointers to static NUL-terminated names.
                required.push(unsafe { CStr::from_ptr(p) });
            }
        }
        // VK_KHR_surface is needed by the device extension VK_KHR_swapchain even for headless devices.
        let mut optional = vec![ext::debug_utils::NAME, khr::surface::NAME];
        let want_settings = plan.sync || gpu_av;
        let use_layer_settings = want_settings && available.contains(&ext::layer_settings::NAME);
        let use_validation_features = want_settings
            && !use_layer_settings
            && available.contains(&ext::validation_features::NAME);
        if use_layer_settings {
            optional.push(ext::layer_settings::NAME);
        } else if use_validation_features {
            optional.push(ext::validation_features::NAME);
        } else if want_settings {
            tracing::warn!(
                "neither VK_EXT_layer_settings nor VK_EXT_validation_features is available; synchronization/GPU-assisted validation skipped"
            );
        }
        let enabled_exts = plan_extensions(&available, &required, &optional)?;
        let debug_utils_on = enabled_exts.contains(&ext::debug_utils::NAME);
        let ext_ptrs: Vec<*const c_char> = enabled_exts.iter().map(|c| c.as_ptr()).collect();
        let layer_ptrs: Vec<*const c_char> = if plan.enable_layer {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            Vec::new()
        };

        let counters = Arc::new(Counters {
            panic_on_error: std::env::var("ANVIL_VALIDATION_PANIC").as_deref() == Ok("1"),
            ..Counters::default()
        });
        let user = Arc::as_ptr(&counters).cast_mut().cast::<c_void>();

        let engine = c"Anvil";
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(engine)
            .api_version(vk::API_VERSION_1_3);
        let mut messenger_ci = messenger_info(user);
        let true_value = [u8::from(true), 0, 0, 0]; // VkBool32 TRUE, little-endian bytes
        let setting = |name: &'static CStr| {
            vk::LayerSettingEXT::default()
                .layer_name(VALIDATION_LAYER)
                .setting_name(name)
                .ty(vk::LayerSettingTypeEXT::BOOL32)
                .values(&true_value)
        };
        let settings: Vec<_> = if gpu_av {
            vec![setting(c"gpuav_enable")]
        } else {
            vec![setting(c"validate_sync")]
        };
        let mut layer_settings_ci = vk::LayerSettingsCreateInfoEXT::default().settings(&settings);
        let enables = if gpu_av {
            vec![vk::ValidationFeatureEnableEXT::GPU_ASSISTED]
        } else {
            vec![vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION]
        };
        let mut features_ci =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&enables);

        let mut info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layer_ptrs)
            .enabled_extension_names(&ext_ptrs);
        if debug_utils_on && plan.enable_layer {
            info = info.push_next(&mut messenger_ci);
        }
        if use_layer_settings {
            info = info.push_next(&mut layer_settings_ci);
        }
        if use_validation_features {
            info = info.push_next(&mut features_ci);
        }
        // SAFETY: every pointer in `info` (app info, name arrays, pNext chain, user data) refers
        // to locals that live until after this call returns; `counters` outlives the messenger.
        let raw = unsafe { entry.create_instance(&info, None) }
            .map_err(|r| map_vk("vkCreateInstance", r))?;

        let surface_fn = desc
            .display
            .map(|_| khr::surface::Instance::new(&entry, &raw));
        let debug_utils = debug_utils_on.then(|| ext::debug_utils::Instance::new(&entry, &raw));
        let messenger = match (&debug_utils, plan.enable_layer) {
            (Some(du), true) => {
                let ci = messenger_info(user);
                // SAFETY: `ci` is valid for the call; `user` points into `counters`, which is
                // moved into InstanceShared and released only after the messenger is destroyed.
                match unsafe { du.create_debug_utils_messenger(&ci, None) } {
                    Ok(m) => Some(m),
                    Err(r) => {
                        // SAFETY: the instance has no children yet.
                        unsafe { raw.destroy_instance(None) };
                        return Err(map_vk("vkCreateDebugUtilsMessengerEXT", r));
                    }
                }
            }
            _ => None,
        };

        let shared = Arc::new(InstanceShared {
            entry,
            raw,
            surface_fn,
            debug_utils,
            messenger,
            counters,
            caps: InstanceCaps {
                validation_active: plan.enable_layer,
                sync_validation_active: plan.sync
                    && (use_layer_settings || use_validation_features),
            },
        });
        let ext_names: Vec<_> = enabled_exts.iter().map(|c| c.to_string_lossy()).collect();
        tracing::info!(
            app_name = %desc.app_name,
            api_version = %format_args!(
                "{}.{}.{}",
                vk::api_version_major(api_version),
                vk::api_version_minor(api_version),
                vk::api_version_patch(api_version)
            ),
            layers = ?if plan.enable_layer { vec!["VK_LAYER_KHRONOS_validation"] } else { vec![] },
            extensions = ?ext_names,
            validation = shared.caps.validation_active,
            sync_validation = shared.caps.sync_validation_active,
            gpu_av = gpu_av && (use_layer_settings || use_validation_features),
            debug_utils = debug_utils_on,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "instance created"
        );
        Ok(Self { shared })
    }

    /// Validation messages seen so far (plain data, ADR-0009 §6).
    #[must_use]
    pub fn validation_counts(&self) -> ValidationCounts {
        self.shared.counts()
    }
}

impl rhi::Instance for VulkanInstance {
    type Surface = VulkanSurface;
    type Device = VulkanDevice;

    fn create_surface<W>(&self, window: Arc<W>) -> Result<VulkanSurface, RhiError>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        crate::swapchain::create_surface(&self.shared, window)
    }

    fn create_device(
        &self,
        desc: &DeviceDesc,
        compatible_surface: Option<&VulkanSurface>,
    ) -> Result<VulkanDevice, RhiError> {
        crate::device::create_device(&self.shared, desc, compatible_surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ValidationMode::{Auto, Off, On};

    fn plan(
        m: ValidationMode,
        dbg: bool,
        present: bool,
        sync: bool,
    ) -> Result<ValidationPlan, RhiError> {
        resolve_validation(m, dbg, present, sync)
    }

    #[test]
    fn validation_plan_matrix() {
        for dbg in [true, false] {
            for present in [true, false] {
                let p = plan(Off, dbg, present, true).unwrap();
                assert!(!p.enable_layer && !p.sync && !p.warn_missing);
            }
        }
        let p = plan(On, false, true, true).unwrap();
        assert!(p.enable_layer && p.sync);
        assert!(!plan(On, true, true, false).unwrap().sync);
        assert!(matches!(
            plan(On, true, false, false),
            Err(RhiError::Unsupported { what }) if what == "VK_LAYER_KHRONOS_validation"
        ));
        let p = plan(Auto, true, true, true).unwrap();
        assert!(p.enable_layer && p.sync);
        let p = plan(Auto, true, false, true).unwrap();
        assert!(!p.enable_layer && p.warn_missing && !p.sync);
    }

    #[test]
    fn validation_auto_release_is_off() {
        for present in [true, false] {
            let p = plan(Auto, false, present, true).unwrap();
            assert!(!p.enable_layer && !p.sync && !p.warn_missing);
        }
    }

    #[test]
    fn instance_version_check() {
        assert!(check_instance_version(None).is_err());
        assert!(check_instance_version(Some(vk::API_VERSION_1_0)).is_err());
        assert!(check_instance_version(Some(vk::API_VERSION_1_2)).is_err());
        assert!(check_instance_version(Some(vk::API_VERSION_1_3)).is_ok());
        assert!(check_instance_version(Some(vk::make_api_version(0, 1, 4, 0))).is_ok());
    }

    #[test]
    fn plan_extensions_required_missing() {
        let e = plan_extensions(&[c"A"], &[c"B"], &[]).unwrap_err();
        assert!(matches!(e, RhiError::Unsupported { what } if what == "B"));
    }

    #[test]
    fn plan_extensions_optional_missing() {
        let out = plan_extensions(&[c"A", c"C"], &[c"A"], &[c"B", c"C"]).unwrap();
        assert_eq!(out, vec![c"A", c"C"]);
    }

    #[test]
    fn severity_mapping() {
        use vk::DebugUtilsMessageSeverityFlagsEXT as S;
        assert_eq!(severity_to_level(S::ERROR), tracing::Level::ERROR);
        assert_eq!(severity_to_level(S::WARNING), tracing::Level::WARN);
        assert_eq!(severity_to_level(S::INFO), tracing::Level::DEBUG);
        assert_eq!(severity_to_level(S::VERBOSE), tracing::Level::TRACE);
    }

    #[test]
    fn app_name_with_nul_rejected() {
        let d = InstanceDesc {
            app_name: "a\0b".into(),
            ..InstanceDesc::default()
        };
        assert!(matches!(
            VulkanInstance::new(&d),
            Err(RhiError::InvalidDescriptor(_))
        ));
    }
}
