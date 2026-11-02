//! MODULE-005 §8 (lead amendment): queued uploads, recorded into `upload_cb` at every submit.

use ash::vk;
use gpu_allocator::vulkan::Allocation;
use rhi::{RhiError, TextureState};

use crate::commands::image_barrier;
use crate::destroy::Doomed;
use crate::device::DeviceShared;
use crate::error::map_vk;

pub(crate) enum PendingUpload {
    Buffer {
        staging: vk::Buffer,
        allocation: Allocation,
        dst: vk::Buffer,
        dst_offset: u64,
        size: u64,
    },
    Texture {
        staging: vk::Buffer,
        allocation: Allocation,
        image: vk::Image,
        extent: (u32, u32),
        bytes: u64,
        /// Tracked state when queued: `Undefined` or `ShaderRead`.
        from: TextureState,
    },
}

impl PendingUpload {
    fn bytes(&self) -> u64 {
        match self {
            PendingUpload::Buffer { size, .. } => *size,
            PendingUpload::Texture { bytes, .. } => *bytes,
        }
    }

    /// The staging buffer, to be destroyed once the submission has completed.
    pub(crate) fn into_doomed(self) -> Doomed {
        let (buffer, allocation) = match self {
            PendingUpload::Buffer {
                staging,
                allocation,
                ..
            }
            | PendingUpload::Texture {
                staging,
                allocation,
                ..
            } => (staging, allocation),
        };
        Doomed::Buffer {
            buffer,
            allocation,
            staging: true,
        }
    }
}

/// Leading global barrier (MODULE-005 U2): protects uploads against earlier submissions.
pub(crate) fn leading_barrier() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ | vk::AccessFlags2::TRANSFER_WRITE)
}

/// Trailing global barrier (MODULE-005 U4): makes copies visible to everything after.
pub(crate) fn trailing_barrier() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
}

/// Records `uploads` into `cb` (which must be in the initial state) and ends it.
pub(crate) fn record(
    s: &DeviceShared,
    cb: vk::CommandBuffer,
    uploads: &[PendingUpload],
) -> Result<(), RhiError> {
    let bi =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: `cb` belongs to the current slot whose pool was reset at begin_frame and is not
    // recording; only the submitting thread touches it.
    unsafe { s.raw.begin_command_buffer(cb, &bi) }
        .map_err(|r| map_vk("vkBeginCommandBuffer", r))?;
    let lead = [leading_barrier()];
    let dep = vk::DependencyInfo::default().memory_barriers(&lead);
    // SAFETY: `cb` is recording; `dep` outlives the call.
    unsafe { s.raw.cmd_pipeline_barrier2(cb, &dep) };
    for u in uploads {
        match u {
            PendingUpload::Buffer {
                staging,
                dst,
                dst_offset,
                size,
                ..
            } => {
                let region = [vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: *dst_offset,
                    size: *size,
                }];
                // SAFETY: staging and dst are alive (dst destruction is deferred past this
                // submission); the range was validated when queued.
                unsafe { s.raw.cmd_copy_buffer(cb, *staging, *dst, &region) };
            }
            PendingUpload::Texture {
                staging,
                image,
                extent,
                from,
                ..
            } => {
                let barrier = |a, b| -> Result<(), RhiError> {
                    let bs = [image_barrier(*image, vk::ImageAspectFlags::COLOR, a, b)?];
                    let dep = vk::DependencyInfo::default().image_memory_barriers(&bs);
                    // SAFETY: `cb` is recording; the image is alive until the submission retires.
                    unsafe { s.raw.cmd_pipeline_barrier2(cb, &dep) };
                    Ok(())
                };
                barrier(*from, TextureState::TransferDst)?;
                let region = [vk::BufferImageCopy {
                    buffer_offset: 0,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    image_offset: vk::Offset3D::default(),
                    image_extent: vk::Extent3D {
                        width: extent.0,
                        height: extent.1,
                        depth: 1,
                    },
                }];
                // SAFETY: the image is in TRANSFER_DST_OPTIMAL (barrier above) and has
                // TRANSFER_DST usage (validated when queued); staging holds w*h*bpt bytes.
                unsafe {
                    s.raw.cmd_copy_buffer_to_image(
                        cb,
                        *staging,
                        *image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &region,
                    )
                };
                barrier(TextureState::TransferDst, TextureState::ShaderRead)?;
            }
        }
    }
    let trail = [trailing_barrier()];
    let dep = vk::DependencyInfo::default().memory_barriers(&trail);
    // SAFETY: `cb` is recording.
    unsafe { s.raw.cmd_pipeline_barrier2(cb, &dep) };
    // SAFETY: `cb` is recording.
    unsafe { s.raw.end_command_buffer(cb) }.map_err(|r| map_vk("vkEndCommandBuffer", r))
}

/// Total staged bytes, for the submit trace.
pub(crate) fn total_bytes(uploads: &[PendingUpload]) -> u64 {
    uploads.iter().map(PendingUpload::bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barriers_match_spec() {
        let l = leading_barrier();
        assert_eq!(l.src_stage_mask, vk::PipelineStageFlags2::ALL_COMMANDS);
        assert_eq!(l.dst_stage_mask, vk::PipelineStageFlags2::COPY);
        assert!(l.dst_access_mask.contains(vk::AccessFlags2::TRANSFER_WRITE));
        let t = trailing_barrier();
        assert_eq!(t.src_stage_mask, vk::PipelineStageFlags2::COPY);
        assert_eq!(t.src_access_mask, vk::AccessFlags2::TRANSFER_WRITE);
        assert_eq!(t.dst_stage_mask, vk::PipelineStageFlags2::ALL_COMMANDS);
        assert!(t.dst_access_mask.contains(vk::AccessFlags2::MEMORY_READ));
    }
}
