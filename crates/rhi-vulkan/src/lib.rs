//! Vulkan implementation of the Anvil RHI.
//!
//! Specification: `docs/specs/layers/LAYER-004-vulkan-backend.md`.
//!
//! The public surface is [`Vulkan`] (an [`rhi::Backend`]) plus its associated types; no
//! `ash::vk` type appears in a public signature (ADR-0001, ADR-0005).
//!
//! Observability (ADR-0009): every create/destroy logs at `debug`, per-frame operations log at
//! `trace` inside a `frame` span, state changes log at `info`, validation messages go to the
//! target `vulkan::validation`, and every Vulkan object gets a debug name.

mod bindings;
mod buffer;
mod commands;
mod destroy;
mod device;
mod error;
mod instance;
mod pipeline_cache;
mod registry;
mod swapchain;
pub mod testing;
mod texture;
mod timestamps;
mod upload;

#[cfg(test)]
mod gpu_tests;

pub use commands::{VulkanCommands, VulkanFinished};
pub use device::VulkanDevice;
pub use instance::VulkanInstance;
pub use swapchain::VulkanSurface;

use rhi::{Backend, InstanceDesc, RhiError};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The Vulkan 1.3 backend marker type.
pub struct Vulkan;

impl Backend for Vulkan {
    type Instance = VulkanInstance;
    type Surface = VulkanSurface;
    type Device = VulkanDevice;

    fn create_instance(desc: &InstanceDesc) -> Result<VulkanInstance, RhiError> {
        VulkanInstance::new(desc)
    }
}

// A poisoned lock only means another thread panicked; the data is still structurally valid.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
pub(crate) fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(PoisonError::into_inner)
}
pub(crate) fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(PoisonError::into_inner)
}
