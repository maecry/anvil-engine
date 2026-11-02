//! MODULE-005: buffers, host access and upload staging.

use std::ptr::NonNull;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use rhi::{BufferDesc, BufferHandle, BufferUsage, IndexFormat, Memory, RhiError};

use crate::destroy::{Doomed, destroy_doomed};
use crate::device::DeviceShared;
use crate::error::{invalid, map_vk, stale};
use crate::registry::generated_name;
use crate::upload::PendingUpload;
use crate::{lock, read, write};

/// Pointer into persistently mapped memory owned by the buffer's allocation.
pub(crate) struct MappedPtr(NonNull<u8>);
// SAFETY: the pointer addresses host-visible memory owned by the `Allocation` stored next to it;
// all accesses go through bounds-checked copies, and GPU/host ordering is the caller's contract.
unsafe impl Send for MappedPtr {}
// SAFETY: as above; the pointer value itself is immutable.
unsafe impl Sync for MappedPtr {}

pub(crate) struct VkBuffer {
    pub raw: vk::Buffer,
    pub allocation: Option<Allocation>,
    pub size: u64,
    pub usage: BufferUsage,
    pub memory: Memory,
    pub address: u64,
    pub mapped: Option<MappedPtr>,
    pub coherent: bool,
    pub label: String,
}

// ---------------------------------------------------------------- pure helpers

pub(crate) fn usage_to_vk(u: BufferUsage) -> vk::BufferUsageFlags {
    let mut f = vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    for (bit, vk_bit) in [
        (BufferUsage::INDEX, vk::BufferUsageFlags::INDEX_BUFFER),
        (BufferUsage::STORAGE, vk::BufferUsageFlags::STORAGE_BUFFER),
        (BufferUsage::UNIFORM, vk::BufferUsageFlags::UNIFORM_BUFFER),
        (
            BufferUsage::TRANSFER_SRC,
            vk::BufferUsageFlags::TRANSFER_SRC,
        ),
        (
            BufferUsage::TRANSFER_DST,
            vk::BufferUsageFlags::TRANSFER_DST,
        ),
        (BufferUsage::INDIRECT, vk::BufferUsageFlags::INDIRECT_BUFFER),
        (BufferUsage::VERTEX, vk::BufferUsageFlags::VERTEX_BUFFER),
    ] {
        if u.contains(bit) {
            f |= vk_bit;
        }
    }
    f
}

pub(crate) fn memory_location(m: Memory) -> MemoryLocation {
    match m {
        Memory::GpuOnly => MemoryLocation::GpuOnly,
        Memory::Upload => MemoryLocation::CpuToGpu,
        Memory::Readback => MemoryLocation::GpuToCpu,
    }
}

/// `offset + len <= size`, overflow-safe.
pub(crate) fn check_range(what: &str, offset: u64, len: u64, size: u64) -> Result<(), RhiError> {
    match offset.checked_add(len) {
        Some(end) if end <= size => Ok(()),
        _ => Err(invalid(format!(
            "{what}: range offset {offset} + length {len} exceeds buffer size {size}"
        ))),
    }
}

pub(crate) fn check_index_alignment(
    offset: u64,
    size: u64,
    f: IndexFormat,
) -> Result<(), RhiError> {
    if !offset.is_multiple_of(f.size()) || offset >= size {
        return Err(invalid(format!(
            "bind_index_buffer: offset {offset} must be a multiple of {} and below the buffer size {size}",
            f.size()
        )));
    }
    Ok(())
}

/// Range for `vkFlush/InvalidateMappedMemoryRanges`: `(offset, size)` where `size` is `None`
/// for `VK_WHOLE_SIZE`. `alloc_offset/alloc_size` locate the allocation inside its memory block,
/// `off/len` the touched bytes inside the allocation.
pub(crate) fn mapped_range(
    alloc_offset: u64,
    alloc_size: u64,
    off: u64,
    len: u64,
    atom: u64,
) -> (u64, Option<u64>) {
    let atom = atom.max(1);
    let start = (alloc_offset + off) / atom * atom;
    let end = (alloc_offset + off + len).div_ceil(atom) * atom;
    if end > alloc_offset + alloc_size {
        // Rounding past the allocation might pass the end of the memory block; the spec allows
        // an unaligned size only at the end of the block, which WHOLE_SIZE expresses.
        (start, None)
    } else {
        (start, Some(end - start))
    }
}

// ---------------------------------------------------------------- raw buffers

/// Creates, allocates and binds a buffer; returns `(buffer, allocation)`. Unwinds on failure.
pub(crate) fn create_raw(
    s: &DeviceShared,
    size: u64,
    usage: vk::BufferUsageFlags,
    memory: Memory,
    name: &str,
) -> Result<(vk::Buffer, Allocation), RhiError> {
    let ci = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: `ci` outlives the call; the device is live.
    let buffer =
        unsafe { s.raw.create_buffer(&ci, None) }.map_err(|r| map_vk("vkCreateBuffer", r))?;
    // SAFETY: `buffer` was just created on this device.
    let requirements = unsafe { s.raw.get_buffer_memory_requirements(buffer) };
    let allocation = match s.alloc(&AllocationCreateDesc {
        name,
        requirements,
        location: memory_location(memory),
        linear: true,
        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
    }) {
        Ok(a) => a,
        Err(e) => {
            // SAFETY: the buffer was never bound or used.
            unsafe { s.raw.destroy_buffer(buffer, None) };
            return Err(e);
        }
    };
    // SAFETY: the buffer is unbound and the allocation satisfies its requirements.
    let bound = unsafe {
        s.raw
            .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
    };
    if let Err(r) = bound {
        s.free(allocation);
        // SAFETY: bind failed, so the buffer is unused.
        unsafe { s.raw.destroy_buffer(buffer, None) };
        return Err(map_vk("vkBindBufferMemory", r));
    }
    s.set_name(buffer, name);
    Ok((buffer, allocation))
}

fn mapped_of(a: &Allocation) -> Option<MappedPtr> {
    a.mapped_ptr().map(|p| MappedPtr(p.cast::<u8>()))
}

fn is_coherent(a: &Allocation) -> bool {
    a.memory_properties()
        .contains(vk::MemoryPropertyFlags::HOST_COHERENT)
}

/// Copies `data` into mapped memory and flushes when not coherent.
///
/// # Safety
/// `ptr` maps `alloc` (`alloc.size()` bytes), `off + data.len() <= alloc.size()`, and the GPU is
/// not accessing that range.
unsafe fn copy_in(
    s: &DeviceShared,
    alloc: &Allocation,
    ptr: &MappedPtr,
    coherent: bool,
    off: u64,
    data: &[u8],
) {
    // SAFETY: caller contract: in-bounds mapped destination, `data` is a distinct host slice.
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.0.as_ptr().add(off as usize), data.len());
    }
    if !coherent {
        flush_or_invalidate(s, alloc, off, data.len() as u64, true);
    }
}

fn flush_or_invalidate(s: &DeviceShared, alloc: &Allocation, off: u64, len: u64, flush: bool) {
    let (offset, size) = mapped_range(
        alloc.offset(),
        alloc.size(),
        off,
        len,
        s.limits.non_coherent_atom,
    );
    // SAFETY: the memory handle is only passed to Vulkan while `alloc` is alive.
    let memory = unsafe { alloc.memory() };
    let range = vk::MappedMemoryRange::default()
        .memory(memory)
        .offset(offset)
        .size(size.unwrap_or(vk::WHOLE_SIZE));
    // SAFETY: the range lies within the allocation's memory block and is atom-aligned (see
    // `mapped_range`); the memory is host visible and mapped.
    let r = unsafe {
        if flush {
            s.raw.flush_mapped_memory_ranges(&[range])
        } else {
            s.raw.invalidate_mapped_memory_ranges(&[range])
        }
    };
    if let Err(r) = r {
        tracing::warn!(target: "rhi_vulkan::buffer", result = ?r, flush, "mapped memory range operation failed");
    }
}

// ---------------------------------------------------------------- public operations

pub(crate) fn create_buffer(s: &DeviceShared, desc: &BufferDesc) -> Result<BufferHandle, RhiError> {
    if desc.size == 0 {
        return Err(invalid("create_buffer: size must be nonzero"));
    }
    if desc.usage.is_empty() {
        return Err(invalid("create_buffer: usage must not be empty"));
    }
    let label = desc.label.unwrap_or("buffer");
    let (raw, allocation) = create_raw(s, desc.size, usage_to_vk(desc.usage), desc.memory, label)?;
    let info = vk::BufferDeviceAddressInfo::default().buffer(raw);
    // SAFETY: the buffer has SHADER_DEVICE_ADDRESS usage and bound memory (bufferDeviceAddress
    // is an enabled feature).
    let address = unsafe { s.raw.get_buffer_device_address(&info) };
    // UMA/software devices (lavapipe) map GpuOnly memory too; ignore that so write/read_buffer
    // behave the same on every device.
    let mapped = (desc.memory != Memory::GpuOnly)
        .then(|| mapped_of(&allocation))
        .flatten();
    debug_assert!(desc.memory == Memory::GpuOnly || mapped.is_some());
    let coherent = is_coherent(&allocation);
    let h = write(&s.pools.buffers).insert(VkBuffer {
        raw,
        allocation: Some(allocation),
        size: desc.size,
        usage: desc.usage,
        memory: desc.memory,
        address,
        mapped,
        coherent,
        label: String::new(),
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("buffer", h), str::to_owned);
    s.set_name(raw, &name);
    if let Some(b) = write(&s.pools.buffers).get_mut(h) {
        b.label.clone_from(&name);
    }
    tracing::debug!(
        target: "rhi_vulkan::buffer", handle = ?h, label = %name, size = desc.size,
        usage = ?desc.usage, memory = ?desc.memory, address, "buffer created"
    );
    Ok(h)
}

pub(crate) fn destroy_buffer(s: &DeviceShared, h: BufferHandle) {
    let removed = write(&s.pools.buffers).remove(h);
    match removed {
        Some(mut b) => {
            if let Some(allocation) = b.allocation.take() {
                s.sync.defer(Doomed::Buffer {
                    buffer: b.raw,
                    allocation,
                    staging: false,
                });
            }
            tracing::debug!(target: "rhi_vulkan::buffer", handle = ?h, label = %b.label, "buffer destroy deferred");
        }
        None => {
            tracing::warn!(target: "rhi_vulkan::buffer", handle = ?h, "destroy_buffer on stale handle ignored")
        }
    }
}

pub(crate) fn buffer_address(s: &DeviceShared, h: BufferHandle) -> Result<u64, RhiError> {
    read(&s.pools.buffers)
        .get(h)
        .map(|b| b.address)
        .ok_or_else(|| stale("buffer", h))
}

pub(crate) fn write_buffer(
    s: &DeviceShared,
    h: BufferHandle,
    offset: u64,
    data: &[u8],
) -> Result<(), RhiError> {
    let pool = read(&s.pools.buffers);
    let b = pool.get(h).ok_or_else(|| stale("buffer", h))?;
    if b.memory != Memory::Upload {
        return Err(invalid("write_buffer needs Memory::Upload"));
    }
    check_range("write_buffer", offset, data.len() as u64, b.size)?;
    if data.is_empty() {
        return Ok(());
    }
    let (Some(alloc), Some(ptr)) = (&b.allocation, &b.mapped) else {
        return Err(invalid("write_buffer: buffer is not mapped"));
    };
    // SAFETY: `ptr` maps `alloc`; the range was checked above; `data` is a caller slice that
    // cannot alias the mapping; the pool read lock keeps the buffer alive; GPU ordering is the
    // caller's contract (documented on `Device::write_buffer`).
    unsafe { copy_in(s, alloc, ptr, b.coherent, offset, data) };
    tracing::trace!(target: "rhi_vulkan::buffer", handle = ?h, offset, bytes = data.len(), "write_buffer");
    Ok(())
}

pub(crate) fn read_buffer(
    s: &DeviceShared,
    h: BufferHandle,
    offset: u64,
    out: &mut [u8],
) -> Result<(), RhiError> {
    let pool = read(&s.pools.buffers);
    let b = pool.get(h).ok_or_else(|| stale("buffer", h))?;
    if b.memory != Memory::Readback {
        return Err(invalid("read_buffer needs Memory::Readback"));
    }
    check_range("read_buffer", offset, out.len() as u64, b.size)?;
    if out.is_empty() {
        return Ok(());
    }
    let (Some(alloc), Some(ptr)) = (&b.allocation, &b.mapped) else {
        return Err(invalid("read_buffer: buffer is not mapped"));
    };
    if !b.coherent {
        flush_or_invalidate(s, alloc, offset, out.len() as u64, false);
    }
    // SAFETY: `ptr` maps `alloc`; the range was checked above; `out` is a distinct caller slice;
    // the GPU having finished writing the range is the caller's contract.
    unsafe {
        std::ptr::copy_nonoverlapping(
            ptr.0.as_ptr().add(offset as usize),
            out.as_mut_ptr(),
            out.len(),
        );
    }
    tracing::trace!(target: "rhi_vulkan::buffer", handle = ?h, offset, bytes = out.len(), "read_buffer");
    Ok(())
}

/// Creates a host-visible `TRANSFER_SRC` buffer filled with `data`.
pub(crate) fn create_staging(
    s: &DeviceShared,
    data: &[u8],
    name: &str,
) -> Result<(vk::Buffer, Allocation), RhiError> {
    let (buffer, allocation) = create_raw(
        s,
        data.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        Memory::Upload,
        name,
    )?;
    let Some(ptr) = mapped_of(&allocation) else {
        s.free(allocation);
        // SAFETY: the buffer was never submitted.
        unsafe { s.raw.destroy_buffer(buffer, None) };
        return Err(invalid("staging buffer is not host visible"));
    };
    let coherent = is_coherent(&allocation);
    // SAFETY: freshly created mapped allocation of `data.len()` bytes; nothing else uses it yet.
    unsafe { copy_in(s, &allocation, &ptr, coherent, 0, data) };
    Ok((buffer, allocation))
}

pub(crate) fn upload_buffer(
    s: &DeviceShared,
    h: BufferHandle,
    offset: u64,
    data: &[u8],
) -> Result<(), RhiError> {
    let (dst, label) = {
        let pool = read(&s.pools.buffers);
        let b = pool.get(h).ok_or_else(|| stale("buffer", h))?;
        if !b.usage.contains(BufferUsage::TRANSFER_DST) {
            return Err(invalid("upload_buffer needs BufferUsage::TRANSFER_DST"));
        }
        check_range("upload_buffer", offset, data.len() as u64, b.size)?;
        (b.raw, b.label.clone())
    };
    if data.is_empty() {
        return Ok(());
    }
    let (staging, allocation) = create_staging(s, data, &format!("staging({label})"))?;
    lock(&s.pending_uploads).push(PendingUpload::Buffer {
        staging,
        allocation,
        dst,
        dst_offset: offset,
        size: data.len() as u64,
    });
    tracing::trace!(target: "rhi_vulkan::buffer", handle = ?h, offset, bytes = data.len(), "upload_buffer queued");
    Ok(())
}

/// Shutdown: warns about and destroys buffers that were never destroyed.
pub(crate) fn destroy_leaked(s: &DeviceShared) {
    let leaked = {
        let mut p = write(&s.pools.buffers);
        let hs = p.handles();
        hs.into_iter()
            .filter_map(|h| p.remove(h).map(|v| (h, v)))
            .collect::<Vec<_>>()
    };
    for (h, mut b) in leaked {
        tracing::warn!(target: "rhi_vulkan::buffer", handle = ?h, label = %b.label, "leaked buffer destroyed at shutdown");
        if let Some(allocation) = b.allocation.take() {
            // SAFETY: device idle; removed from the pool so destroyed once.
            unsafe {
                destroy_doomed(
                    s,
                    Doomed::Buffer {
                        buffer: b.raw,
                        allocation,
                        staging: false,
                    },
                )
            };
        }
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VkBuffer>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_to_vk_flags() {
        let base = vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        assert_eq!(usage_to_vk(BufferUsage::NONE), base);
        assert_eq!(
            usage_to_vk(BufferUsage::INDEX | BufferUsage::TRANSFER_DST),
            base | vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST
        );
        let all = BufferUsage::INDEX
            | BufferUsage::STORAGE
            | BufferUsage::UNIFORM
            | BufferUsage::TRANSFER_SRC
            | BufferUsage::TRANSFER_DST;
        assert_eq!(
            usage_to_vk(all),
            base | vk::BufferUsageFlags::INDEX_BUFFER
                | vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::UNIFORM_BUFFER
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST
        );
    }

    #[test]
    fn memory_to_location() {
        assert!(matches!(
            memory_location(Memory::GpuOnly),
            MemoryLocation::GpuOnly
        ));
        assert!(matches!(
            memory_location(Memory::Upload),
            MemoryLocation::CpuToGpu
        ));
        assert!(matches!(
            memory_location(Memory::Readback),
            MemoryLocation::GpuToCpu
        ));
    }

    #[test]
    fn range_check_overflow() {
        assert!(check_range("t", 0, 16, 16).is_ok());
        assert!(check_range("t", 16, 0, 16).is_ok());
        assert!(check_range("t", 1, 16, 16).is_err());
        assert!(check_range("t", u64::MAX, 2, 16).is_err());
        assert!(check_range("t", 8, u64::MAX, 16).is_err());
    }

    #[test]
    fn index_alignment_check() {
        assert!(check_index_alignment(2, 16, IndexFormat::U16).is_ok());
        assert!(check_index_alignment(1, 16, IndexFormat::U16).is_err());
        assert!(check_index_alignment(2, 16, IndexFormat::U32).is_err());
        assert!(check_index_alignment(4, 16, IndexFormat::U32).is_ok());
        assert!(check_index_alignment(16, 16, IndexFormat::U32).is_err());
    }

    #[test]
    fn mapped_range_alignment() {
        // Inside the allocation: aligned down/up to the atom.
        assert_eq!(mapped_range(256, 1024, 10, 20, 64), (256, Some(64)));
        assert_eq!(mapped_range(256, 1024, 60, 10, 64), (256, Some(128)));
        // Rounding past the allocation end falls back to WHOLE_SIZE.
        assert_eq!(mapped_range(256, 100, 90, 10, 64), (320, None));
        assert_eq!(mapped_range(0, 128, 0, 128, 64), (0, Some(128)));
        assert_eq!(mapped_range(0, 100, 0, 100, 1), (0, Some(100)));
    }

    #[test]
    fn vk_buffer_is_send_sync() {
        // Compile-time assertion above; this keeps the check visible in the test list.
        fn needs<T: Send + Sync>() {}
        needs::<VkBuffer>();
    }

    #[test]
    fn generated_names() {
        let mut r: crate::registry::Registry<rhi::BufferTag, u8> =
            crate::registry::Registry::new(|| rhi::BufferTag);
        let h = r.insert(0);
        assert_eq!(generated_name("buffer", h), "buffer#0v1");
    }
}
