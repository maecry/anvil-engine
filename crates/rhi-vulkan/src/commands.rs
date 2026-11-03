//! MODULE-004: frame slots, timeline sync, command encoder, barriers, shader modules, pipelines.

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use rhi::{
    BlendFactor, BlendOp, BlendState, BufferHandle, BufferState, BufferUsage, CommandEncoder,
    ComputePipelineDesc, CullMode, DepthState, Format, FrameToken, FrontFace, GraphicsPipelineDesc,
    IndexFormat, LoadOp, Memory, PipelineHandle, PrimitiveTopology, Rect2D, RenderingDesc,
    RhiError, ShaderModuleDesc, ShaderModuleHandle, StoreOp, TextureHandle, TextureState,
    TextureUsage, Viewport,
};

use crate::bindings::check_push_constants;
use crate::buffer::{check_index_alignment, check_range};
use crate::destroy::{Doomed, destroy_doomed};
use crate::device::DeviceShared;
use crate::error::{invalid, map_vk, stale};
use crate::registry::generated_name;
use crate::texture::{
    StateCheck, TextureOwner, check_tracked_state, compare_to_vk, rhi_to_vk_format,
};
use crate::upload::{self, PendingUpload};
use crate::{lock, read, write};

pub(crate) const MAX_COLOR_ATTACHMENTS: usize = 8;

// ---------------------------------------------------------------- barriers (MODULE-004 §6.3)

#[derive(Debug, Clone, Copy)]
pub(crate) struct BarrierHalf {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

const SHADER_STAGES: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::VERTEX_SHADER.as_raw()
        | vk::PipelineStageFlags2::FRAGMENT_SHADER.as_raw()
        | vk::PipelineStageFlags2::COMPUTE_SHADER.as_raw(),
);

pub(crate) fn state_as_source(s: TextureState) -> BarrierHalf {
    use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as S};
    let (layout, stage, access) = match s {
        // Undefined and Present use COLOR_ATTACHMENT_OUTPUT (not TOP_OF_PIPE): that is the stage
        // the acquire semaphore wait is chained to (MODULE-004 §6.3).
        TextureState::Undefined => (L::UNDEFINED, S::COLOR_ATTACHMENT_OUTPUT, A::NONE),
        TextureState::ColorAttachment => (
            L::COLOR_ATTACHMENT_OPTIMAL,
            S::COLOR_ATTACHMENT_OUTPUT,
            A::COLOR_ATTACHMENT_WRITE,
        ),
        TextureState::Present => (L::PRESENT_SRC_KHR, S::COLOR_ATTACHMENT_OUTPUT, A::NONE),
        TextureState::ShaderRead => (L::SHADER_READ_ONLY_OPTIMAL, SHADER_STAGES, A::NONE),
        TextureState::TransferDst => (L::TRANSFER_DST_OPTIMAL, S::ALL_TRANSFER, A::TRANSFER_WRITE),
        TextureState::TransferSrc => (L::TRANSFER_SRC_OPTIMAL, S::ALL_TRANSFER, A::NONE),
        TextureState::DepthAttachment => (
            L::DEPTH_ATTACHMENT_OPTIMAL,
            S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
            A::DEPTH_STENCIL_ATTACHMENT_WRITE,
        ),
        TextureState::StorageWrite => (L::GENERAL, S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE),
    };
    BarrierHalf {
        layout,
        stage,
        access,
    }
}

pub(crate) fn state_as_destination(s: TextureState) -> Result<BarrierHalf, RhiError> {
    use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as S};
    let (layout, stage, access) = match s {
        TextureState::Undefined => {
            return Err(invalid("Undefined is not a valid destination state"));
        }
        TextureState::ColorAttachment => (
            L::COLOR_ATTACHMENT_OPTIMAL,
            S::COLOR_ATTACHMENT_OUTPUT,
            A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE,
        ),
        TextureState::Present => (L::PRESENT_SRC_KHR, S::NONE, A::NONE),
        TextureState::ShaderRead => (
            L::SHADER_READ_ONLY_OPTIMAL,
            SHADER_STAGES,
            A::SHADER_SAMPLED_READ,
        ),
        TextureState::TransferDst => (L::TRANSFER_DST_OPTIMAL, S::ALL_TRANSFER, A::TRANSFER_WRITE),
        TextureState::TransferSrc => (L::TRANSFER_SRC_OPTIMAL, S::ALL_TRANSFER, A::TRANSFER_READ),
        TextureState::DepthAttachment => (
            L::DEPTH_ATTACHMENT_OPTIMAL,
            S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
            A::DEPTH_STENCIL_ATTACHMENT_READ | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
        ),
        TextureState::StorageWrite => (
            L::GENERAL,
            S::COMPUTE_SHADER,
            A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE,
        ),
    };
    Ok(BarrierHalf {
        layout,
        stage,
        access,
    })
}

pub(crate) fn image_barrier(
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    from: TextureState,
    to: TextureState,
) -> Result<vk::ImageMemoryBarrier2<'static>, RhiError> {
    image_barrier_range(
        image,
        vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: vk::REMAINING_MIP_LEVELS,
            base_array_layer: 0,
            layer_count: vk::REMAINING_ARRAY_LAYERS,
        },
        from,
        to,
    )
}

/// [`image_barrier`] over an explicit mip and layer range (views, per-mip tracking).
pub(crate) fn image_barrier_range(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
    from: TextureState,
    to: TextureState,
) -> Result<vk::ImageMemoryBarrier2<'static>, RhiError> {
    let aspect = range.aspect_mask;
    let (mut src, dst) = (state_as_source(from), state_as_destination(to)?);
    if from == TextureState::Undefined && aspect.contains(vk::ImageAspectFlags::DEPTH) {
        // A depth image re-transitioned from `Undefined` every frame must be ordered after the
        // previous frame's depth writes.
        src.stage = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
    }
    Ok(vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src.stage)
        .src_access_mask(src.access)
        .dst_stage_mask(dst.stage)
        .dst_access_mask(dst.access)
        .old_layout(src.layout)
        .new_layout(dst.layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range))
}

/// Pipeline stages and access of a buffer state: `(stage, access)`.
pub(crate) fn buffer_state_masks(s: BufferState) -> (vk::PipelineStageFlags2, vk::AccessFlags2) {
    use vk::{AccessFlags2 as A, PipelineStageFlags2 as S};
    match s {
        BufferState::Undefined => (S::NONE, A::NONE),
        BufferState::ShaderRead => (
            S::VERTEX_SHADER | S::FRAGMENT_SHADER | S::COMPUTE_SHADER,
            A::SHADER_STORAGE_READ,
        ),
        BufferState::ShaderWrite => (
            S::COMPUTE_SHADER,
            A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE,
        ),
        BufferState::TransferSrc => (S::ALL_TRANSFER, A::TRANSFER_READ),
        BufferState::TransferDst => (S::ALL_TRANSFER, A::TRANSFER_WRITE),
        BufferState::Indirect => (S::DRAW_INDIRECT, A::INDIRECT_COMMAND_READ),
        BufferState::Vertex => (S::VERTEX_INPUT, A::VERTEX_ATTRIBUTE_READ),
        BufferState::Index => (S::INDEX_INPUT, A::INDEX_READ),
    }
}

/// Buffer memory barrier for `from` -> `to`. The source access is only the writes of `from`
/// (reads need no availability); `Undefined` as a source orders against nothing.
pub(crate) fn buffer_barrier2(
    buffer: vk::Buffer,
    from: BufferState,
    to: BufferState,
) -> Result<vk::BufferMemoryBarrier2<'static>, RhiError> {
    if to == BufferState::Undefined {
        return Err(invalid("Undefined is not a valid destination buffer state"));
    }
    let (ss, sa) = buffer_state_masks(from);
    let (ds, da) = buffer_state_masks(to);
    Ok(vk::BufferMemoryBarrier2::default()
        .src_stage_mask(ss)
        .src_access_mask(if from.is_write() {
            sa
        } else {
            vk::AccessFlags2::NONE
        })
        .dst_stage_mask(ds)
        .dst_access_mask(da)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE))
}

// ---------------------------------------------------------------- slot tracker (pure)

pub(crate) struct SlotTracker {
    values: Vec<u64>,
    frame_counter: u64,
    last_submitted: u64,
}

impl SlotTracker {
    pub(crate) fn new(frames_in_flight: u32) -> Self {
        Self {
            values: vec![0; frames_in_flight as usize],
            frame_counter: 0,
            last_submitted: 0,
        }
    }

    /// Returns the slot and the timeline value to wait for (0 = none).
    pub(crate) fn begin_frame(&mut self) -> (usize, u64) {
        let slot = (self.frame_counter % self.values.len() as u64) as usize;
        self.frame_counter += 1;
        (slot, self.values[slot])
    }

    pub(crate) fn record_submit(&mut self, slot: usize) -> u64 {
        self.last_submitted += 1;
        self.values[slot] = self.last_submitted;
        self.last_submitted
    }

    pub(crate) fn next_value(&self) -> u64 {
        self.last_submitted + 1
    }
}

// ---------------------------------------------------------------- encoder state (pure)

#[derive(Default)]
pub(crate) struct EncoderState {
    first_error: Option<RhiError>,
    rendering: bool,
    pipeline_bound: bool,
    label_depth: u32,
    formats: [Option<Format>; MAX_COLOR_ATTACHMENTS],
    depth: Option<Format>,
    index_bound: bool,
    /// A compute pipeline is bound (cleared by binding a graphics pipeline never; compute is
    /// only valid outside rendering).
    compute_bound: bool,
    /// The global descriptor set was bound in this command buffer (graphics bind point).
    global_bound: bool,
    /// Same for the compute bind point.
    global_bound_compute: bool,
}

impl EncoderState {
    pub(crate) fn failed(&self) -> bool {
        self.first_error.is_some()
    }

    /// Keeps the first error only.
    pub(crate) fn record(&mut self, e: RhiError) {
        self.first_error.get_or_insert(e);
    }

    pub(crate) fn begin_rendering(&mut self, formats: &[Format], depth: Option<Format>) -> bool {
        if self.rendering {
            self.record(invalid("begin_rendering while already rendering"));
            return false;
        }
        self.rendering = true;
        self.pipeline_bound = false;
        self.index_bound = false;
        self.depth = depth;
        self.formats = [None; MAX_COLOR_ATTACHMENTS];
        for (d, f) in self.formats.iter_mut().zip(formats) {
            *d = Some(*f);
        }
        true
    }

    pub(crate) fn end_rendering(&mut self) -> bool {
        if !self.rendering {
            self.record(invalid("end_rendering outside rendering"));
            return false;
        }
        self.rendering = false;
        self.pipeline_bound = false;
        self.index_bound = false;
        true
    }

    pub(crate) fn check_inside_rendering(&mut self, what: &str) -> bool {
        if !self.rendering {
            self.record(invalid(format!("{what} outside rendering")));
        }
        self.rendering
    }

    pub(crate) fn check_outside_rendering(&mut self, what: &str) -> bool {
        if self.rendering {
            self.record(invalid(format!("{what} inside rendering")));
        }
        !self.rendering
    }

    pub(crate) fn check_draw(&mut self) -> bool {
        if !self.rendering || !self.pipeline_bound {
            self.record(invalid(
                "draw requires active rendering and a bound pipeline",
            ));
            return false;
        }
        true
    }

    pub(crate) fn check_pipeline(
        &mut self,
        p: &[Option<Format>; MAX_COLOR_ATTACHMENTS],
        depth: Option<Format>,
    ) -> bool {
        if !self.check_inside_rendering("bind_pipeline") {
            return false;
        }
        if self.formats != *p {
            self.record(invalid(
                "pipeline color formats differ from the active rendering",
            ));
            return false;
        }
        if self.depth != depth {
            self.record(invalid(format!(
                "pipeline depth format {depth:?} differs from the active rendering ({:?})",
                self.depth
            )));
            return false;
        }
        self.pipeline_bound = true;
        true
    }

    pub(crate) fn check_draw_indexed(&mut self) -> bool {
        if !self.check_draw() {
            return false;
        }
        if !self.index_bound {
            self.record(invalid("draw_indexed requires a bound index buffer"));
            return false;
        }
        true
    }

    /// Returns true the first time, when the caller must bind the global descriptor set.
    pub(crate) fn take_global_unbound(&mut self) -> bool {
        !std::mem::replace(&mut self.global_bound, true)
    }

    /// Compute bind point variant of [`Self::take_global_unbound`].
    pub(crate) fn take_global_unbound_compute(&mut self) -> bool {
        !std::mem::replace(&mut self.global_bound_compute, true)
    }

    /// A compute pipeline may be bound outside rendering only.
    pub(crate) fn check_bind_compute(&mut self) -> bool {
        if !self.check_outside_rendering("bind_pipeline (compute)") {
            return false;
        }
        self.compute_bound = true;
        true
    }

    pub(crate) fn check_dispatch(&mut self) -> bool {
        if !self.check_outside_rendering("dispatch") {
            return false;
        }
        if !self.compute_bound {
            self.record(invalid("dispatch requires a bound compute pipeline"));
            return false;
        }
        true
    }

    pub(crate) fn check_draw_indirect(&mut self) -> bool {
        self.check_draw_indexed()
    }

    pub(crate) fn push_label(&mut self) {
        self.label_depth += 1;
    }

    pub(crate) fn pop_label(&mut self) -> bool {
        if self.label_depth == 0 {
            self.record(invalid("end_label without begin_label"));
            return false;
        }
        self.label_depth -= 1;
        true
    }

    pub(crate) fn finish(self) -> Result<(), RhiError> {
        if let Some(e) = self.first_error {
            return Err(e);
        }
        if self.rendering {
            return Err(invalid("finish while rendering is still active"));
        }
        if self.label_depth != 0 {
            return Err(invalid("unbalanced begin_label/end_label"));
        }
        Ok(())
    }
}

/// NUL-terminated, truncated at a char boundary, no heap.
pub(crate) fn encode_label(name: &str) -> [u8; 128] {
    let mut out = [0u8; 128];
    let mut n = name.len().min(127);
    while !name.is_char_boundary(n) {
        n -= 1;
    }
    out[..n].copy_from_slice(&name.as_bytes()[..n]);
    out
}

// ---------------------------------------------------------------- frame slots

pub(crate) struct FrameSlot {
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    /// Records the queued uploads at submit (MODULE-005 §8).
    upload_cb: vk::CommandBuffer,
    acquire_sem: vk::Semaphore,
    epoch: Arc<AtomicU64>,
    open: bool,
}

pub(crate) struct CommandManager {
    shared: Arc<DeviceShared>,
    slots: Vec<FrameSlot>,
    tracker: SlotTracker,
    current: Option<usize>,
    acquired: bool,
    /// `ANVIL_SIMULATE_DEVICE_LOST=<frame>`: the submit of that (1-based) frame fails with
    /// `DeviceLost` (fault injection, ADR-0015 §4). No real loss occurs.
    simulate_lost: Option<u64>,
    pub(crate) span: tracing::Span,
}

impl CommandManager {
    pub(crate) fn new(shared: Arc<DeviceShared>, frames_in_flight: u32) -> Result<Self, RhiError> {
        let mut cm = Self {
            shared,
            slots: Vec::new(),
            tracker: SlotTracker::new(frames_in_flight),
            current: None,
            acquired: false,
            simulate_lost: std::env::var("ANVIL_SIMULATE_DEVICE_LOST")
                .ok()
                .and_then(|v| v.parse().ok()),
            span: tracing::Span::none(),
        };
        for i in 0..frames_in_flight {
            let slot = cm.create_slot(i)?; // on error `cm` drops and `shutdown` cleans the rest
            cm.slots.push(slot);
        }
        Ok(cm)
    }

    fn create_slot(&self, i: u32) -> Result<FrameSlot, RhiError> {
        let s = &self.shared;
        let pool_ci = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::TRANSIENT)
            .queue_family_index(s.queue_family);
        // SAFETY: queue family belongs to this device; the pool is destroyed in `shutdown`.
        let pool = unsafe { s.raw.create_command_pool(&pool_ci, None) }
            .map_err(|r| map_vk("vkCreateCommandPool", r))?;
        let cb_ci = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(2);
        // SAFETY: `pool` was just created on this device.
        let (cb, upload_cb) = match unsafe { s.raw.allocate_command_buffers(&cb_ci) } {
            Ok(v) => (v[0], v[1]),
            Err(r) => {
                // SAFETY: nothing else references the pool yet.
                unsafe { s.raw.destroy_command_pool(pool, None) };
                return Err(map_vk("vkAllocateCommandBuffers", r));
            }
        };
        // SAFETY: plain creation on a live device.
        let sem = match unsafe {
            s.raw
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        } {
            Ok(v) => v,
            Err(r) => {
                // SAFETY: nothing else references the pool yet.
                unsafe { s.raw.destroy_command_pool(pool, None) };
                return Err(map_vk("vkCreateSemaphore", r));
            }
        };
        s.set_name(pool, &format!("frame{i}.command_pool"));
        s.set_name(cb, &format!("frame{i}.command_buffer"));
        s.set_name(upload_cb, &format!("frame{i}.upload_command_buffer"));
        s.set_name(sem, &format!("frame{i}.acquire_semaphore"));
        tracing::debug!(slot = i, "frame slot created");
        Ok(FrameSlot {
            pool,
            cb,
            upload_cb,
            acquire_sem: sem,
            epoch: Arc::new(AtomicU64::new(0)),
            open: false,
        })
    }

    pub(crate) fn frame_counter(&self) -> u64 {
        self.tracker.frame_counter
    }

    pub(crate) fn frames_in_flight(&self) -> u32 {
        self.slots.len() as u32
    }

    pub(crate) fn begin_frame(&mut self) -> Result<FrameToken, RhiError> {
        if self.current.is_some() {
            tracing::trace!("previous frame abandoned without submit");
        }
        let (slot, wait) = self.tracker.begin_frame();
        let frame = self.tracker.frame_counter;
        self.span = tracing::debug_span!("frame", frame);
        let span = self.span.clone();
        let _g = span.enter();
        let s = &self.shared;
        tracing::trace!(slot, wait_value = wait, "begin_frame");
        if wait > 0 {
            let sems = [s.sync.timeline];
            let vals = [wait];
            let info = vk::SemaphoreWaitInfo::default()
                .semaphores(&sems)
                .values(&vals);
            // SAFETY: the timeline is alive; `wait` was queued for signal by a successful submit.
            unsafe { s.raw.wait_semaphores(&info, u64::MAX) }
                .map_err(|r| map_vk("vkWaitSemaphores", r))?;
        }
        // SAFETY: the timeline is alive.
        let completed = unsafe { s.raw.get_semaphore_counter_value(s.sync.timeline) }
            .map_err(|r| map_vk("vkGetSemaphoreCounterValue", r))?;
        loop {
            let d = lock(&s.sync.deferred).pop_ready(completed);
            let Some(d) = d else { break };
            let kind = d.kind();
            // SAFETY: tag retirement proved the GPU no longer uses the object; popped once; no
            // lock is held here.
            unsafe { destroy_doomed(s, d) };
            tracing::debug!(kind, completed, "deferred destroy retired");
        }
        let sl = &mut self.slots[slot];
        // SAFETY: the timeline wait proved this slot's last submission completed, and encoders
        // must be finished or dropped before the slot is reused (epoch rule below).
        unsafe {
            s.raw
                .reset_command_pool(sl.pool, vk::CommandPoolResetFlags::empty())
        }
        .map_err(|r| map_vk("vkResetCommandPool", r))?;
        sl.epoch.fetch_add(1, Ordering::AcqRel);
        sl.open = false;
        self.acquired = false;
        self.current = Some(slot);
        Ok(FrameToken {
            slot: slot as u32,
            frame,
        })
    }

    pub(crate) fn begin_commands(&mut self, t: &FrameToken) -> Result<VulkanCommands, RhiError> {
        if self.current != Some(t.slot as usize) {
            return Err(invalid("frame token does not match the open frame"));
        }
        let sl = &mut self.slots[t.slot as usize];
        if sl.open {
            return Err(invalid("commands already begun for this frame"));
        }
        let bi = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the pool was reset this frame and only this thread records into it.
        unsafe { self.shared.raw.begin_command_buffer(sl.cb, &bi) }
            .map_err(|r| map_vk("vkBeginCommandBuffer", r))?;
        sl.open = true;
        if let Some(ts) = &self.shared.timestamps {
            // SAFETY: open command buffer outside rendering; the slot's last frame retired, and
            // its results stay readable on the host until this reset executes at submit.
            unsafe {
                self.shared.raw.cmd_reset_query_pool(
                    sl.cb,
                    ts.pools[t.slot as usize],
                    0,
                    rhi::MAX_TIMESTAMPS,
                );
            }
        }
        Ok(VulkanCommands {
            shared: self.shared.clone(),
            cb: sl.cb,
            slot: t.slot as usize,
            epoch: sl.epoch.load(Ordering::Acquire),
            epoch_ref: sl.epoch.clone(),
            st: EncoderState::default(),
        })
    }

    pub(crate) fn acquire_semaphore(&self) -> Result<vk::Semaphore, RhiError> {
        let slot = self
            .current
            .ok_or_else(|| invalid("acquire outside a frame"))?;
        if self.acquired {
            return Err(invalid("already acquired in this frame"));
        }
        Ok(self.slots[slot].acquire_sem)
    }

    pub(crate) fn mark_acquired(&mut self) {
        self.acquired = true;
    }

    pub(crate) fn submit(
        &mut self,
        f: VulkanFinished,
        wait_acquire: bool,
        signal: Option<vk::Semaphore>,
    ) -> Result<u64, RhiError> {
        let span = self.span.clone();
        let _g = span.enter();
        let slot = self
            .current
            .filter(|c| *c == f.slot)
            .ok_or_else(|| invalid("finished commands do not belong to the open frame"))?;
        if f.epoch_ref.load(Ordering::Acquire) != f.epoch {
            return Err(invalid("encoder outlived its frame"));
        }
        if wait_acquire && !self.acquired {
            return Err(invalid("submit requires an acquired swapchain image"));
        }
        if self.simulate_lost == Some(self.tracker.frame_counter) {
            tracing::error!(frame = self.tracker.frame_counter, "simulated device loss");
            return Err(RhiError::DeviceLost);
        }
        let s = &self.shared;
        let value = self.tracker.next_value();
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.slots[slot].acquire_sem)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        // Queued uploads are recorded into this slot's upload command buffer, which runs ahead of
        // the frame commands in the same submission (MODULE-005 lead amendment).
        let uploads = std::mem::take(&mut *lock(&s.pending_uploads));
        let (upload_count, upload_bytes) = (uploads.len(), upload::total_bytes(&uploads));
        let mut cbs = Vec::with_capacity(2);
        if !uploads.is_empty() {
            let ucb = self.slots[slot].upload_cb;
            if let Err(e) = upload::record(s, ucb, &uploads) {
                defer_staging(s, uploads);
                return Err(e);
            }
            cbs.push(vk::CommandBufferSubmitInfo::default().command_buffer(ucb));
        }
        cbs.push(vk::CommandBufferSubmitInfo::default().command_buffer(f.cb));
        let timeline = vk::SemaphoreSubmitInfo::default()
            .semaphore(s.sync.timeline)
            .value(value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
        let signals = match signal {
            Some(rd) => vec![
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(rd)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
                timeline,
            ],
            None => vec![timeline],
        };
        let info = vk::SubmitInfo2::default()
            .wait_semaphore_infos(if wait_acquire { &waits } else { &[] })
            .command_buffer_infos(&cbs)
            .signal_semaphore_infos(&signals);
        let q = lock(&s.queue);
        // SAFETY: queue mutex held; all slices and handles live through the call; the acquire wait
        // is included only if an acquire is pending; render-done is unsignalled (re-acquired).
        let r = unsafe { s.raw.queue_submit2(*q, &[info], vk::Fence::null()) };
        drop(q);
        // Staging buffers retire with this submission's timeline value (`doom_tag` == `value`).
        defer_staging(s, uploads);
        r.map_err(|r| map_vk("vkQueueSubmit2", r))?;
        self.tracker.record_submit(slot);
        s.sync.last_submitted.store(value, Ordering::Release);
        self.slots[slot].open = false;
        self.acquired = false;
        tracing::trace!(slot, value, uploads = upload_count, upload_bytes, "submit");
        Ok(value)
    }

    /// Submit without swapchain semaphores (tests, headless rendering).
    pub(crate) fn submit_unwindowed(&mut self, f: VulkanFinished) -> Result<u64, RhiError> {
        self.submit(f, false, None)
    }

    /// Destroys all slots; the device must be idle.
    pub(crate) fn shutdown(&mut self) {
        let s = &self.shared;
        for sl in self.slots.drain(..) {
            // SAFETY: device idle; objects owned by the slot, destroyed once (pool frees its buffer).
            unsafe {
                s.raw.destroy_semaphore(sl.acquire_sem, None);
                s.raw.destroy_command_pool(sl.pool, None);
            }
        }
        self.current = None;
    }
}

impl Drop for CommandManager {
    fn drop(&mut self) {
        // Only non-empty if device construction failed before `VulkanDevice` took over.
        if !self.slots.is_empty() {
            let _ = self.shared.wait_idle();
            self.shutdown();
        }
    }
}

fn defer_staging(s: &DeviceShared, uploads: Vec<PendingUpload>) {
    for u in uploads {
        s.sync.defer(u.into_doomed());
    }
}

// ---------------------------------------------------------------- encoder

/// Recording command encoder; owned and `Send`. Errors surface at [`CommandEncoder::finish`].
pub struct VulkanCommands {
    shared: Arc<DeviceShared>,
    cb: vk::CommandBuffer,
    slot: usize,
    epoch: u64,
    epoch_ref: Arc<AtomicU64>,
    st: EncoderState,
}

/// Indirect command stride: a multiple of 4 and at least one record (checked when drawing more
/// than one).
pub(crate) fn check_indirect_stride(stride: u32, count: u32) -> Result<(), RhiError> {
    if !stride.is_multiple_of(4) || (count > 1 && stride < 20) {
        return Err(invalid(format!(
            "indirect stride {stride} must be a multiple of 4 and at least 20"
        )));
    }
    Ok(())
}

impl VulkanCommands {
    /// The raw buffer if it has `INDIRECT` usage and `offset..offset + len` is inside it.
    fn indirect_buffer(
        &self,
        what: &str,
        b: BufferHandle,
        offset: u64,
        len: u64,
    ) -> Result<vk::Buffer, RhiError> {
        let pool = read(&self.shared.pools.buffers);
        let buf = pool.get(b).ok_or_else(|| stale("buffer", b))?;
        if !buf.usage.contains(BufferUsage::INDIRECT) {
            return Err(invalid(format!("{what} needs BufferUsage::INDIRECT")));
        }
        if !offset.is_multiple_of(4) {
            return Err(invalid(format!("{what}: offset must be a multiple of 4")));
        }
        check_range(what, offset, len, buf.size)?;
        Ok(buf.raw)
    }

    /// Makes a preceding copy visible to host reads (`Readback` destinations).
    fn host_read_barrier(&self) {
        let mb = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)];
        let dep = vk::DependencyInfo::default().memory_barriers(&mb);
        // SAFETY: open command buffer outside rendering.
        unsafe { self.shared.raw.cmd_pipeline_barrier2(self.cb, &dep) };
    }
}

/// A finished command buffer ready for `Device::submit`.
pub struct VulkanFinished {
    cb: vk::CommandBuffer,
    slot: usize,
    epoch: u64,
    epoch_ref: Arc<AtomicU64>,
}

fn load_op(l: LoadOp) -> vk::AttachmentLoadOp {
    match l {
        LoadOp::Load => vk::AttachmentLoadOp::LOAD,
        LoadOp::Clear => vk::AttachmentLoadOp::CLEAR,
        LoadOp::DontCare => vk::AttachmentLoadOp::DONT_CARE,
    }
}

fn store_op(s: StoreOp) -> vk::AttachmentStoreOp {
    match s {
        StoreOp::Store => vk::AttachmentStoreOp::STORE,
        StoreOp::DontCare => vk::AttachmentStoreOp::DONT_CARE,
    }
}

impl CommandEncoder for VulkanCommands {
    type Finished = VulkanFinished;

    fn texture_barrier(&mut self, t: TextureHandle, from: TextureState, to: TextureState) {
        if self.st.failed() || !self.st.check_outside_rendering("texture_barrier") {
            return;
        }
        let mut pool = write(&self.shared.pools.textures);
        let Some(tex) = pool.get_mut(t) else {
            drop(pool);
            return self.st.record(stale("texture", t));
        };
        let device_owned = tex.owner != TextureOwner::Swapchain;
        if device_owned && (from == TextureState::Present || to == TextureState::Present) {
            return self.st.record(RhiError::Unsupported {
                what: "Present state on a non-swapchain texture".to_owned(),
            });
        }
        let depth = tex.format.is_depth();
        if (to == TextureState::DepthAttachment && !depth)
            || (to == TextureState::ColorAttachment && depth)
        {
            return self.st.record(invalid(format!(
                "texture_barrier to {to:?} on a {:?} texture",
                tex.format
            )));
        }
        match image_barrier_range(tex.image, tex.range(), from, to) {
            Ok(b) => {
                if device_owned
                    && let StateCheck::Mismatch { tracked } = check_tracked_state(tex.state, from)
                {
                    tracing::warn!(handle = ?t, ?tracked, ?from, "texture_barrier from-state mismatch");
                }
                if device_owned {
                    tex.state = to;
                }
                let barriers = [b];
                let dep = vk::DependencyInfo::default().image_memory_barriers(&barriers);
                // SAFETY: `cb` is open on this encoder's slot (pool reset this frame, single
                // thread); the image was resolved under the pool lock and outlives the submission.
                unsafe { self.shared.raw.cmd_pipeline_barrier2(self.cb, &dep) };
            }
            Err(e) => self.st.record(e),
        }
    }

    fn buffer_barrier(&mut self, b: BufferHandle, from: BufferState, to: BufferState) {
        if self.st.failed() || !self.st.check_outside_rendering("buffer_barrier") {
            return;
        }
        let raw = match read(&self.shared.pools.buffers).get(b) {
            Some(v) => v.raw,
            None => return self.st.record(stale("buffer", b)),
        };
        match buffer_barrier2(raw, from, to) {
            Ok(bb) => {
                let barriers = [bb];
                let dep = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
                // SAFETY: open command buffer outside rendering; the buffer is alive (stale
                // handles rejected above, a later destroy is deferred past this submission).
                unsafe { self.shared.raw.cmd_pipeline_barrier2(self.cb, &dep) };
            }
            Err(e) => self.st.record(e),
        }
    }

    fn fill_buffer(&mut self, b: BufferHandle, offset: u64, size: u64, value: u32) {
        if self.st.failed() || !self.st.check_outside_rendering("fill_buffer") {
            return;
        }
        let raw = {
            let pool = read(&self.shared.pools.buffers);
            let Some(buf) = pool.get(b) else {
                drop(pool);
                return self.st.record(stale("buffer", b));
            };
            if !buf.usage.contains(BufferUsage::TRANSFER_DST) {
                return self
                    .st
                    .record(invalid("fill_buffer needs BufferUsage::TRANSFER_DST"));
            }
            if !offset.is_multiple_of(4) || !size.is_multiple_of(4) {
                return self.st.record(invalid(
                    "fill_buffer offset and size must be multiples of 4",
                ));
            }
            if let Err(e) = check_range("fill_buffer", offset, size, buf.size) {
                return self.st.record(e);
            }
            buf.raw
        };
        if size == 0 {
            return;
        }
        // SAFETY: open command buffer outside rendering; buffer alive; range and usage validated.
        unsafe {
            self.shared
                .raw
                .cmd_fill_buffer(self.cb, raw, offset, size, value)
        };
    }

    fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        if self.st.failed() || !self.st.check_dispatch() {
            return;
        }
        // SAFETY: open command buffer outside rendering with a bound compute pipeline (checked).
        unsafe { self.shared.raw.cmd_dispatch(self.cb, x, y, z) };
    }

    fn dispatch_indirect(&mut self, b: BufferHandle, offset: u64) {
        if self.st.failed() || !self.st.check_dispatch() {
            return;
        }
        let raw = match self.indirect_buffer("dispatch_indirect", b, offset, 12) {
            Ok(r) => r,
            Err(e) => return self.st.record(e),
        };
        // SAFETY: as dispatch; the buffer has INDIRECT usage and holds 12 bytes at `offset`.
        unsafe { self.shared.raw.cmd_dispatch_indirect(self.cb, raw, offset) };
    }

    fn draw_indexed_indirect(
        &mut self,
        b: BufferHandle,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) {
        if self.st.failed() || !self.st.check_draw_indirect() {
            return;
        }
        if let Err(e) = check_indirect_stride(stride, draw_count) {
            return self.st.record(e);
        }
        let len = u64::from(stride) * u64::from(draw_count.saturating_sub(1)) + 20;
        let raw = match self.indirect_buffer("draw_indexed_indirect", b, offset, len) {
            Ok(r) => r,
            Err(e) => return self.st.record(e),
        };
        // SAFETY: open command buffer inside rendering with pipeline and index buffer bound
        // (checked); the buffer has INDIRECT usage and holds `draw_count` records at `offset`.
        unsafe {
            self.shared
                .raw
                .cmd_draw_indexed_indirect(self.cb, raw, offset, draw_count, stride)
        };
    }

    fn draw_indexed_indirect_count(
        &mut self,
        b: BufferHandle,
        offset: u64,
        count: BufferHandle,
        count_offset: u64,
        max_draw_count: u32,
        stride: u32,
    ) {
        if self.st.failed() || !self.st.check_draw_indirect() {
            return;
        }
        if let Err(e) = check_indirect_stride(stride, max_draw_count) {
            return self.st.record(e);
        }
        let len = u64::from(stride) * u64::from(max_draw_count.saturating_sub(1)) + 20;
        let raw = match self.indirect_buffer("draw_indexed_indirect_count", b, offset, len) {
            Ok(r) => r,
            Err(e) => return self.st.record(e),
        };
        let count_raw = match self.indirect_buffer(
            "draw_indexed_indirect_count (count)",
            count,
            count_offset,
            4,
        ) {
            Ok(r) => r,
            Err(e) => return self.st.record(e),
        };
        if !count_offset.is_multiple_of(4) {
            return self
                .st
                .record(invalid("indirect count offset must be a multiple of 4"));
        }
        // SAFETY: as draw_indexed_indirect; the count buffer has INDIRECT usage and holds a u32 at
        // `count_offset`; the drawIndirectCount feature is enabled at device creation.
        unsafe {
            self.shared.raw.cmd_draw_indexed_indirect_count(
                self.cb,
                raw,
                offset,
                count_raw,
                count_offset,
                max_draw_count,
                stride,
            )
        };
    }

    fn begin_rendering(&mut self, desc: &RenderingDesc) {
        if self.st.failed() {
            return;
        }
        if desc.color.len() > MAX_COLOR_ATTACHMENTS
            || (desc.color.is_empty() && desc.depth.is_none())
        {
            return self.st.record(invalid(
                "need 1..=8 color attachments, or a depth attachment alone (depth-only pass)",
            ));
        }
        if desc.width == 0 || desc.height == 0 {
            return self.st.record(invalid("rendering area must be nonzero"));
        }
        let mut atts = [vk::RenderingAttachmentInfo::default(); MAX_COLOR_ATTACHMENTS];
        let mut formats = [Format::Bgra8Srgb; MAX_COLOR_ATTACHMENTS];
        let mut depth_att = vk::RenderingAttachmentInfo::default();
        let mut depth_format = None;
        {
            let tex = read(&self.shared.pools.textures);
            for (i, c) in desc.color.iter().enumerate() {
                let Some(t) = tex.get(c.texture) else {
                    return self.st.record(stale("texture", c.texture));
                };
                if !t.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
                    return self.st.record(invalid(
                        "color attachment texture lacks TextureUsage::COLOR_ATTACHMENT",
                    ));
                }
                formats[i] = t.format;
                atts[i] = vk::RenderingAttachmentInfo::default()
                    .image_view(t.view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(load_op(c.load))
                    .store_op(store_op(c.store))
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: c.clear },
                    });
            }
            if let Some(d) = &desc.depth {
                let Some(t) = tex.get(d.texture) else {
                    return self.st.record(stale("texture", d.texture));
                };
                if !t.format.is_depth() || !t.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
                    return self.st.record(invalid(
                        "depth attachment needs a depth-format texture with TextureUsage::DEPTH_ATTACHMENT",
                    ));
                }
                depth_format = Some(t.format);
                depth_att = vk::RenderingAttachmentInfo::default()
                    .image_view(t.view)
                    .image_layout(vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL)
                    .load_op(load_op(d.load))
                    .store_op(store_op(d.store))
                    .clear_value(vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: d.clear,
                            stencil: 0,
                        },
                    });
            }
        }
        let n = desc.color.len();
        if !self.st.begin_rendering(&formats[..n], depth_format) {
            return;
        }
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: desc.width,
                height: desc.height,
            },
        };
        let mut info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&atts[..n]);
        if depth_format.is_some() {
            info = info.depth_attachment(&depth_att);
        }
        let vp = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: desc.width as f32,
            height: desc.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        // SAFETY: `cb` is open on this encoder's slot; views were resolved under the pool lock
        // and their textures outlive the submission (destroy is deferred); `info`, `atts` and
        // `depth_att` outlive the call.
        unsafe {
            self.shared.raw.cmd_begin_rendering(self.cb, &info);
            self.shared.raw.cmd_set_viewport(self.cb, 0, &[vp]);
            self.shared.raw.cmd_set_scissor(self.cb, 0, &[area]);
        }
    }

    fn end_rendering(&mut self) {
        if self.st.failed() || !self.st.end_rendering() {
            return;
        }
        // SAFETY: rendering is active on this open command buffer.
        unsafe { self.shared.raw.cmd_end_rendering(self.cb) };
    }

    fn bind_pipeline(&mut self, p: PipelineHandle) {
        if self.st.failed() {
            return;
        }
        let (pipeline, formats, depth, compute) = match read(&self.shared.pools.pipelines).get(p) {
            Some(v) => (v.pipeline, v.color_formats, v.depth_format, v.compute),
            None => return self.st.record(stale("pipeline", p)),
        };
        let (ok, first, point) = if compute {
            (
                self.st.check_bind_compute(),
                self.st.take_global_unbound_compute(),
                vk::PipelineBindPoint::COMPUTE,
            )
        } else {
            (
                self.st.check_pipeline(&formats, depth),
                self.st.take_global_unbound(),
                vk::PipelineBindPoint::GRAPHICS,
            )
        };
        if !ok {
            return;
        }
        if first {
            let b = &self.shared.bindings;
            // SAFETY: open command buffer; the set and layout live as long as the device. The
            // set is bound once per bind point and command buffer: every pipeline shares the
            // global layout.
            unsafe {
                self.shared.raw.cmd_bind_descriptor_sets(
                    self.cb,
                    point,
                    b.pipeline_layout,
                    0,
                    &[b.set],
                    &[],
                )
            };
            tracing::trace!(target: "rhi_vulkan::bindings", "global descriptor set bound");
        }
        // SAFETY: open command buffer; the pipeline is alive: stale handles are rejected above and
        // a later destroy is deferred past the submission recording it (doom_tag).
        unsafe { self.shared.raw.cmd_bind_pipeline(self.cb, point, pipeline) };
    }

    fn set_viewport(&mut self, v: Viewport) {
        if self.st.failed() || !self.st.check_inside_rendering("set_viewport") {
            return;
        }
        let vp = vk::Viewport {
            x: v.x,
            y: v.y,
            width: v.width,
            height: v.height,
            min_depth: v.min_depth,
            max_depth: v.max_depth,
        };
        // SAFETY: open command buffer.
        unsafe { self.shared.raw.cmd_set_viewport(self.cb, 0, &[vp]) };
    }

    fn set_scissor(&mut self, r: Rect2D) {
        if self.st.failed() || !self.st.check_inside_rendering("set_scissor") {
            return;
        }
        let rect = vk::Rect2D {
            offset: vk::Offset2D { x: r.x, y: r.y },
            extent: vk::Extent2D {
                width: r.width,
                height: r.height,
            },
        };
        // SAFETY: open command buffer.
        unsafe { self.shared.raw.cmd_set_scissor(self.cb, 0, &[rect]) };
    }

    fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        if self.st.failed() || !self.st.check_draw() {
            return;
        }
        // SAFETY: open command buffer inside rendering with a bound pipeline (checked).
        unsafe {
            self.shared.raw.cmd_draw(
                self.cb,
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            )
        };
    }

    fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    ) {
        if self.st.failed() || !self.st.check_draw_indexed() {
            return;
        }
        // SAFETY: open command buffer inside rendering with a bound pipeline and index buffer
        // (checked).
        unsafe {
            self.shared.raw.cmd_draw_indexed(
                self.cb,
                index_count,
                instance_count,
                first_index,
                vertex_offset,
                first_instance,
            )
        };
    }

    fn bind_index_buffer(&mut self, b: BufferHandle, offset: u64, format: IndexFormat) {
        if self.st.failed() || !self.st.check_inside_rendering("bind_index_buffer") {
            return;
        }
        let raw = {
            let pool = read(&self.shared.pools.buffers);
            let Some(buf) = pool.get(b) else {
                drop(pool);
                return self.st.record(stale("buffer", b));
            };
            if !buf.usage.contains(BufferUsage::INDEX) {
                return self
                    .st
                    .record(invalid("bind_index_buffer needs BufferUsage::INDEX"));
            }
            if let Err(e) = check_index_alignment(offset, buf.size, format) {
                return self.st.record(e);
            }
            buf.raw
        };
        let ty = match format {
            IndexFormat::U16 => vk::IndexType::UINT16,
            IndexFormat::U32 => vk::IndexType::UINT32,
        };
        // SAFETY: open command buffer; the buffer is alive (stale handles rejected above, later
        // destroy deferred past this submission); offset validated.
        unsafe {
            self.shared
                .raw
                .cmd_bind_index_buffer(self.cb, raw, offset, ty)
        };
        self.st.index_bound = true;
    }

    fn push_constants(&mut self, offset: u32, data: &[u8]) {
        if self.st.failed() {
            return;
        }
        if let Err(e) = check_push_constants(offset, data.len()) {
            return self.st.record(e);
        }
        if data.is_empty() {
            return;
        }
        // SAFETY: open command buffer; range validated against the layout's 128 byte block.
        unsafe {
            self.shared.raw.cmd_push_constants(
                self.cb,
                self.shared.bindings.pipeline_layout,
                crate::bindings::ALL_STAGES,
                offset,
                data,
            )
        };
        tracing::trace!(target: "rhi_vulkan::bindings", offset, bytes = data.len(), "push_constants");
    }

    fn copy_buffer_to_buffer(
        &mut self,
        src: BufferHandle,
        src_offset: u64,
        dst: BufferHandle,
        dst_offset: u64,
        size: u64,
    ) {
        if self.st.failed() || !self.st.check_outside_rendering("copy_buffer_to_buffer") {
            return;
        }
        let (sraw, draw, host_read) = {
            let pool = read(&self.shared.pools.buffers);
            let Some(sb) = pool.get(src) else {
                drop(pool);
                return self.st.record(stale("buffer", src));
            };
            let Some(db) = pool.get(dst) else {
                drop(pool);
                return self.st.record(stale("buffer", dst));
            };
            if !sb.usage.contains(BufferUsage::TRANSFER_SRC) {
                return self.st.record(invalid(
                    "copy_buffer_to_buffer: source needs BufferUsage::TRANSFER_SRC",
                ));
            }
            if !db.usage.contains(BufferUsage::TRANSFER_DST) {
                return self.st.record(invalid(
                    "copy_buffer_to_buffer: destination needs BufferUsage::TRANSFER_DST",
                ));
            }
            for r in [
                check_range("copy_buffer_to_buffer source", src_offset, size, sb.size),
                check_range(
                    "copy_buffer_to_buffer destination",
                    dst_offset,
                    size,
                    db.size,
                ),
            ] {
                if let Err(e) = r {
                    return self.st.record(e);
                }
            }
            if src == dst && src_offset < dst_offset + size && dst_offset < src_offset + size {
                return self.st.record(invalid(
                    "copy_buffer_to_buffer: overlapping ranges in the same buffer",
                ));
            }
            (sb.raw, db.raw, db.memory == Memory::Readback)
        };
        if size == 0 {
            return;
        }
        let region = [vk::BufferCopy2::default()
            .src_offset(src_offset)
            .dst_offset(dst_offset)
            .size(size)];
        let info = vk::CopyBufferInfo2::default()
            .src_buffer(sraw)
            .dst_buffer(draw)
            .regions(&region);
        // SAFETY: open command buffer outside rendering; buffers alive (see bind_index_buffer);
        // usages and ranges validated.
        unsafe { self.shared.raw.cmd_copy_buffer2(self.cb, &info) };
        if host_read {
            self.host_read_barrier();
        }
    }

    fn copy_texture_to_buffer(&mut self, t: TextureHandle, dst: BufferHandle, dst_offset: u64) {
        if self.st.failed() || !self.st.check_outside_rendering("copy_texture_to_buffer") {
            return;
        }
        let (image, extent, bytes, tracked, mip, layer) = {
            let pool = read(&self.shared.pools.textures);
            let Some(tex) = pool.get(t) else {
                drop(pool);
                return self.st.record(stale("texture", t));
            };
            if !tex.usage.contains(TextureUsage::TRANSFER_SRC) || tex.format.is_depth() {
                return self.st.record(invalid(
                    "copy_texture_to_buffer needs a color texture with TextureUsage::TRANSFER_SRC",
                ));
            }
            let bytes = u64::from(tex.width)
                * u64::from(tex.height)
                * u64::from(tex.format.bytes_per_texel());
            (
                tex.image,
                (tex.width, tex.height),
                bytes,
                // swapchain textures are not state-tracked
                if tex.owner == TextureOwner::Swapchain {
                    TextureState::TransferSrc
                } else {
                    tex.state
                },
                tex.base_mip,
                tex.base_layer,
            )
        };
        let (draw, host_read) = {
            let pool = read(&self.shared.pools.buffers);
            let Some(db) = pool.get(dst) else {
                drop(pool);
                return self.st.record(stale("buffer", dst));
            };
            if !db.usage.contains(BufferUsage::TRANSFER_DST) {
                return self.st.record(invalid(
                    "copy_texture_to_buffer: destination needs BufferUsage::TRANSFER_DST",
                ));
            }
            if let Err(e) = check_range("copy_texture_to_buffer", dst_offset, bytes, db.size) {
                return self.st.record(e);
            }
            (db.raw, db.memory == Memory::Readback)
        };
        if tracked != TextureState::TransferSrc {
            tracing::warn!(handle = ?t, ?tracked, "copy_texture_to_buffer from a texture not tracked as TransferSrc");
        }
        let region = [vk::BufferImageCopy {
            buffer_offset: dst_offset,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: mip,
                base_array_layer: layer,
                layer_count: 1,
            },
            image_offset: vk::Offset3D::default(),
            image_extent: vk::Extent3D {
                width: extent.0,
                height: extent.1,
                depth: 1,
            },
        }];
        // SAFETY: open command buffer outside rendering; image and buffer alive; the caller put
        // the image in TRANSFER_SRC_OPTIMAL with texture_barrier (advisory, warned above).
        unsafe {
            self.shared.raw.cmd_copy_image_to_buffer(
                self.cb,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                draw,
                &region,
            )
        };
        if host_read {
            self.host_read_barrier();
        }
    }

    fn write_timestamp(&mut self, index: u32) {
        let Some(ts) = &self.shared.timestamps else {
            return;
        };
        if self.st.failed() {
            return;
        }
        if index >= rhi::MAX_TIMESTAMPS {
            self.st
                .record(invalid(format!("timestamp index {index} out of range")));
            return;
        }
        // SAFETY: open command buffer; the pool belongs to this slot and was reset at
        // `begin_commands`; each index is written once per frame.
        unsafe {
            self.shared.raw.cmd_write_timestamp2(
                self.cb,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                ts.pools[self.slot],
                index,
            );
        }
        // Index 0 starts a new frame's count; until then the slot still holds the last frame's.
        if index == 0 {
            ts.written[self.slot].store(1, Ordering::Release);
        } else {
            ts.written[self.slot].fetch_max(index + 1, Ordering::AcqRel);
        }
    }

    fn begin_label(&mut self, name: &str, color: [f32; 4]) {
        self.st.push_label();
        if self.st.failed() {
            return;
        }
        if let Some(du) = &self.shared.debug_fn {
            let buf = encode_label(name);
            if let Ok(c) = std::ffi::CStr::from_bytes_until_nul(&buf) {
                let l = vk::DebugUtilsLabelEXT::default().label_name(c).color(color);
                // SAFETY: open command buffer; `l` and `buf` outlive the call.
                unsafe { du.cmd_begin_debug_utils_label(self.cb, &l) };
            }
        }
    }

    fn end_label(&mut self) {
        if !self.st.pop_label() || self.st.failed() {
            return;
        }
        if let Some(du) = &self.shared.debug_fn {
            // SAFETY: open command buffer with a balanced begin (depth checked).
            unsafe { du.cmd_end_debug_utils_label(self.cb) };
        }
    }

    fn finish(self) -> Result<VulkanFinished, RhiError> {
        let Self {
            shared,
            cb,
            slot,
            epoch,
            epoch_ref,
            st,
        } = self;
        st.finish()?;
        if epoch_ref.load(Ordering::Acquire) != epoch {
            return Err(invalid("encoder outlived its frame"));
        }
        // SAFETY: the buffer is in the recording state on its slot's reset pool.
        unsafe { shared.raw.end_command_buffer(cb) }
            .map_err(|r| map_vk("vkEndCommandBuffer", r))?;
        Ok(VulkanFinished {
            cb,
            slot,
            epoch,
            epoch_ref,
        })
    }
}

// ---------------------------------------------------------------- shader modules and pipelines

pub(crate) struct VkShaderModule {
    pub raw: vk::ShaderModule,
}

pub(crate) struct VkPipeline {
    pub pipeline: vk::Pipeline,
    pub compute: bool,
    pub color_formats: [Option<Format>; MAX_COLOR_ATTACHMENTS],
    pub depth_format: Option<Format>,
}

/// Depth/stencil state for a pipeline (MODULE-007 §6.4): test off without a depth state.
pub(crate) fn depth_state(
    depth: Option<DepthState>,
) -> vk::PipelineDepthStencilStateCreateInfo<'static> {
    vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(depth.is_some())
        .depth_write_enable(depth.is_some_and(|d| d.write))
        .depth_compare_op(depth.map_or(vk::CompareOp::ALWAYS, |d| compare_to_vk(d.compare)))
}

pub(crate) fn check_spirv(words: &[u32]) -> Result<(), RhiError> {
    if words.len() >= 5 && words[0] == 0x0723_0203 {
        Ok(())
    } else {
        Err(invalid("not SPIR-V"))
    }
}

pub(crate) fn validate_pipeline_desc(
    desc: &GraphicsPipelineDesc,
) -> Result<(CString, CString), RhiError> {
    if let Some(d) = desc.depth
        && d.format != Format::D32Sfloat
    {
        return Err(invalid(format!(
            "pipeline depth format must be D32Sfloat, got {:?}",
            d.format
        )));
    }
    if desc.color_formats.iter().any(|f| f.is_depth()) {
        return Err(invalid("a depth format is not a color target format"));
    }
    let n = desc.color_formats.len();
    // Zero color formats is a depth-only pipeline (shadow pass).
    if n > MAX_COLOR_ATTACHMENTS {
        return Err(invalid("color format count must be 0..=8"));
    }
    if !desc.blend.is_empty() && desc.blend.len() != n {
        return Err(invalid(
            "blend state count must be 0 or equal the color format count",
        ));
    }
    let entry = |s: &str| CString::new(s).map_err(|_| invalid("shader entry name contains NUL"));
    Ok((entry(desc.vertex.entry)?, entry(desc.fragment.entry)?))
}

pub(crate) fn create_shader_module(
    s: &DeviceShared,
    desc: &ShaderModuleDesc,
) -> Result<ShaderModuleHandle, RhiError> {
    check_spirv(desc.spirv)?;
    let ci = vk::ShaderModuleCreateInfo::default().code(desc.spirv);
    // SAFETY: `ci.code` is aligned u32 words that live through the call.
    let raw = unsafe { s.raw.create_shader_module(&ci, None) }
        .map_err(|r| map_vk("vkCreateShaderModule", r))?;
    let h = write(&s.pools.shader_modules).insert(VkShaderModule { raw });
    let name = desc
        .label
        .map_or_else(|| generated_name("shader_module", h), str::to_owned);
    s.set_name(raw, &name);
    tracing::debug!(handle = ?h, label = %name, words = desc.spirv.len(), "shader module created");
    Ok(h)
}

fn factor(f: BlendFactor) -> vk::BlendFactor {
    match f {
        BlendFactor::Zero => vk::BlendFactor::ZERO,
        BlendFactor::One => vk::BlendFactor::ONE,
        BlendFactor::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        BlendFactor::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
    }
}

fn blend_attachment(b: Option<&BlendState>) -> vk::PipelineColorBlendAttachmentState {
    let b = b.copied().unwrap_or_default();
    let BlendOp::Add = b.color_op;
    let BlendOp::Add = b.alpha_op;
    vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(b.enabled)
        .src_color_blend_factor(factor(b.src_color))
        .dst_color_blend_factor(factor(b.dst_color))
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(factor(b.src_alpha))
        .dst_alpha_blend_factor(factor(b.dst_alpha))
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::from_raw(
            u32::from(b.write_mask) & 0xF,
        ))
}

pub(crate) fn create_graphics_pipeline(
    s: &DeviceShared,
    desc: &GraphicsPipelineDesc,
) -> Result<PipelineHandle, RhiError> {
    let (vname, fname) = validate_pipeline_desc(desc)?;
    let (vmod, fmod) = {
        let m = read(&s.pools.shader_modules);
        let v = m
            .get(desc.vertex.module)
            .ok_or_else(|| stale("shader_module", desc.vertex.module))?
            .raw;
        let f = m
            .get(desc.fragment.module)
            .ok_or_else(|| stale("shader_module", desc.fragment.module))?
            .raw;
        (v, f)
    };
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vmod)
            .name(&vname),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fmod)
            .name(&fname),
    ];
    let topology = match desc.topology {
        PrimitiveTopology::TriangleList => vk::PrimitiveTopology::TRIANGLE_LIST,
        PrimitiveTopology::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
        PrimitiveTopology::LineList => vk::PrimitiveTopology::LINE_LIST,
        PrimitiveTopology::LineStrip => vk::PrimitiveTopology::LINE_STRIP,
        PrimitiveTopology::PointList => vk::PrimitiveTopology::POINT_LIST,
    };
    let cull = match desc.cull {
        CullMode::None => vk::CullModeFlags::NONE,
        CullMode::Front => vk::CullModeFlags::FRONT,
        CullMode::Back => vk::CullModeFlags::BACK,
    };
    let front = match desc.front_face {
        FrontFace::CounterClockwise => vk::FrontFace::COUNTER_CLOCKWISE,
        FrontFace::Clockwise => vk::FrontFace::CLOCKWISE,
    };
    let vk_formats: Vec<vk::Format> = desc
        .color_formats
        .iter()
        .map(|f| rhi_to_vk_format(*f))
        .collect();
    let blends: Vec<_> = (0..vk_formats.len())
        .map(|i| blend_attachment(desc.blend.get(i)))
        .collect();
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(topology);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let bias = desc.depth.and_then(|d| d.bias);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .depth_bias_enable(bias.is_some())
        .depth_bias_constant_factor(bias.map_or(0.0, |b| b.constant))
        .depth_bias_slope_factor(bias.map_or(0.0, |b| b.slope))
        .depth_bias_clamp(bias.map_or(0.0, |b| b.clamp))
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(cull)
        .front_face(front)
        .line_width(1.0);
    let ms = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blends);
    let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
    let depth_stencil = depth_state(desc.depth);
    let mut rendering =
        vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&vk_formats);
    if let Some(d) = desc.depth {
        rendering = rendering.depth_attachment_format(rhi_to_vk_format(d.format));
    }
    let layout = s.bindings.pipeline_layout;
    let ci = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_asm)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&ms)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);
    // SAFETY: all create-info pointers (stages, entry names, state structs, format array) outlive
    // the call; modules and layout are alive; no render pass is used (dynamic rendering).
    let pipeline = match unsafe {
        s.raw
            .create_graphics_pipelines(s.pipeline_cache, &[ci], None)
    } {
        Ok(v) => v[0],
        Err((made, r)) => {
            // SAFETY: partially created pipelines are owned here and unused.
            unsafe {
                for p in made.into_iter().filter(|p| *p != vk::Pipeline::null()) {
                    s.raw.destroy_pipeline(p, None);
                }
            }
            return Err(map_vk("vkCreateGraphicsPipelines", r));
        }
    };
    let mut color_formats = [None; MAX_COLOR_ATTACHMENTS];
    for (d, f) in color_formats.iter_mut().zip(desc.color_formats) {
        *d = Some(*f);
    }
    let h = write(&s.pools.pipelines).insert(VkPipeline {
        pipeline,
        compute: false,
        color_formats,
        depth_format: desc.depth.map(|d| d.format),
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("pipeline", h), str::to_owned);
    s.set_name(pipeline, &name);
    tracing::debug!(handle = ?h, label = %name, color_formats = ?desc.color_formats, depth = ?desc.depth, topology = ?desc.topology, "pipeline created");
    Ok(h)
}

pub(crate) fn create_compute_pipeline(
    s: &DeviceShared,
    desc: &ComputePipelineDesc,
) -> Result<PipelineHandle, RhiError> {
    let name =
        CString::new(desc.shader.entry).map_err(|_| invalid("shader entry name contains NUL"))?;
    let module = read(&s.pools.shader_modules)
        .get(desc.shader.module)
        .ok_or_else(|| stale("shader_module", desc.shader.module))?
        .raw;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(&name);
    let ci = vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(s.bindings.pipeline_layout);
    // SAFETY: `ci`, the entry name and the module outlive the call; the layout is alive.
    let pipeline = match unsafe {
        s.raw
            .create_compute_pipelines(s.pipeline_cache, &[ci], None)
    } {
        Ok(v) => v[0],
        Err((made, r)) => {
            // SAFETY: partially created pipelines are owned here and unused.
            unsafe {
                for p in made.into_iter().filter(|p| *p != vk::Pipeline::null()) {
                    s.raw.destroy_pipeline(p, None);
                }
            }
            return Err(map_vk("vkCreateComputePipelines", r));
        }
    };
    let h = write(&s.pools.pipelines).insert(VkPipeline {
        pipeline,
        compute: true,
        color_formats: [None; MAX_COLOR_ATTACHMENTS],
        depth_format: None,
    });
    let label = desc
        .label
        .map_or_else(|| generated_name("compute_pipeline", h), str::to_owned);
    s.set_name(pipeline, &label);
    tracing::debug!(handle = ?h, label = %label, "compute pipeline created");
    Ok(h)
}

pub(crate) fn destroy_shader_module(s: &DeviceShared, h: ShaderModuleHandle) {
    match write(&s.pools.shader_modules).remove(h) {
        Some(m) => {
            s.sync.defer(Doomed::ShaderModule(m.raw));
            tracing::debug!(handle = ?h, "shader module destroy deferred");
        }
        None => tracing::warn!(handle = ?h, "destroy_shader_module on stale handle ignored"),
    }
}

pub(crate) fn destroy_pipeline(s: &DeviceShared, h: PipelineHandle) {
    match write(&s.pools.pipelines).remove(h) {
        Some(p) => {
            s.sync.defer(Doomed::Pipeline {
                pipeline: p.pipeline,
            });
            tracing::debug!(handle = ?h, "pipeline destroy deferred");
        }
        None => tracing::warn!(handle = ?h, "destroy_pipeline on stale handle ignored"),
    }
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<VulkanCommands>();
    assert_send::<VulkanFinished>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use vk::{AccessFlags2 as A, ImageLayout as L, PipelineStageFlags2 as S};

    const ALL: [TextureState; 7] = [
        TextureState::Undefined,
        TextureState::ColorAttachment,
        TextureState::Present,
        TextureState::ShaderRead,
        TextureState::TransferDst,
        TextureState::TransferSrc,
        TextureState::DepthAttachment,
    ];

    #[test]
    fn tracker_rotation_and_waits() {
        for n in 1..=3u32 {
            let mut t = SlotTracker::new(n);
            for f in 0..10u64 {
                assert_eq!(t.begin_frame().0 as u64, f % u64::from(n));
            }
        }
        let mut t = SlotTracker::new(2);
        assert_eq!(t.begin_frame(), (0, 0));
        assert_eq!(t.record_submit(0), 1);
        assert_eq!(t.begin_frame(), (1, 0));
        assert_eq!(t.begin_frame(), (0, 1)); // frame 2 waits for frame 0's submit
        assert_eq!(t.begin_frame(), (1, 0)); // abandoned frames keep values
        assert_eq!(t.record_submit(1), 2);
        assert_eq!(t.next_value(), 3);
    }

    #[test]
    fn state_pair_mapping() {
        let b = image_barrier(
            vk::Image::null(),
            vk::ImageAspectFlags::DEPTH,
            TextureState::Undefined,
            TextureState::DepthAttachment,
        )
        .unwrap();
        assert_eq!(b.new_layout, L::DEPTH_ATTACHMENT_OPTIMAL);
        assert_eq!(b.subresource_range.aspect_mask, vk::ImageAspectFlags::DEPTH);
        assert!(
            b.dst_stage_mask
                .contains(S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS)
        );
        assert!(
            b.dst_access_mask
                .contains(A::DEPTH_STENCIL_ATTACHMENT_WRITE)
        );
        for (a, c) in [
            (TextureState::DepthAttachment, TextureState::ShaderRead),
            (TextureState::ShaderRead, TextureState::DepthAttachment),
        ] {
            assert!(image_barrier(vk::Image::null(), vk::ImageAspectFlags::DEPTH, a, c).is_ok());
        }
        let up = image_barrier(
            vk::Image::null(),
            vk::ImageAspectFlags::COLOR,
            TextureState::Undefined,
            TextureState::TransferDst,
        )
        .unwrap();
        assert_eq!(up.old_layout, L::UNDEFINED);
        assert_eq!(up.dst_access_mask, A::TRANSFER_WRITE);
        let done = image_barrier(
            vk::Image::null(),
            vk::ImageAspectFlags::COLOR,
            TextureState::TransferDst,
            TextureState::ShaderRead,
        )
        .unwrap();
        assert_eq!(done.src_access_mask, A::TRANSFER_WRITE);
        assert_eq!(done.dst_access_mask, A::SHADER_SAMPLED_READ);
        assert_eq!(done.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
    }

    #[test]
    fn depth_state_mapping() {
        let off = depth_state(None);
        assert_eq!(
            (off.depth_test_enable, off.depth_write_enable),
            (vk::FALSE, vk::FALSE)
        );
        let mut st = DepthState {
            format: Format::D32Sfloat,
            compare: rhi::CompareOp::GreaterOrEqual,
            write: true,
            bias: None,
        };
        let on = depth_state(Some(st));
        assert_eq!(
            (on.depth_test_enable, on.depth_write_enable),
            (vk::TRUE, vk::TRUE)
        );
        assert_eq!(on.depth_compare_op, vk::CompareOp::GREATER_OR_EQUAL);
        st.write = false;
        assert_eq!(depth_state(Some(st)).depth_write_enable, vk::FALSE);
        assert_eq!(on.stencil_test_enable, vk::FALSE);
    }

    #[test]
    fn encoder_depth_and_index_checks() {
        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Bgra8Srgb], Some(Format::D32Sfloat));
        let mut p = [None; MAX_COLOR_ATTACHMENTS];
        p[0] = Some(Format::Bgra8Srgb);
        assert!(!s.check_pipeline(&p, None));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Bgra8Srgb], None);
        assert!(!s.check_pipeline(&p, Some(Format::D32Sfloat)));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Bgra8Srgb], Some(Format::D32Sfloat));
        assert!(s.check_pipeline(&p, Some(Format::D32Sfloat)));
        assert!(!s.check_draw_indexed()); // no index buffer
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        assert!(s.take_global_unbound());
        assert!(!s.take_global_unbound());
        s.begin_rendering(&[Format::Bgra8Srgb], None);
        s.check_pipeline(&p, None);
        s.index_bound = true;
        assert!(s.check_draw_indexed());
        s.end_rendering();
        assert!(s.finish().is_ok());
    }

    #[test]
    fn barrier_table_total_and_rules() {
        for from in ALL {
            for to in &ALL[1..] {
                let Ok(b) =
                    image_barrier(vk::Image::null(), vk::ImageAspectFlags::COLOR, from, *to)
                else {
                    panic!("{from:?} -> {to:?} must be a valid transition");
                };
                assert_eq!(b.old_layout, state_as_source(from).layout);
                assert_eq!(b.new_layout, state_as_destination(*to).unwrap().layout);
                assert!(
                    !b.src_stage_mask.contains(S::ALL_COMMANDS)
                        && !b.dst_stage_mask.contains(S::ALL_COMMANDS)
                );
            }
            let src = state_as_source(from).access;
            let reads = A::COLOR_ATTACHMENT_READ
                | A::SHADER_SAMPLED_READ
                | A::TRANSFER_READ
                | A::MEMORY_READ;
            assert!(
                !src.intersects(reads),
                "{from:?} source access has read bits"
            );
            assert!(!src.intersects(A::MEMORY_WRITE | A::MEMORY_READ));
        }
        assert!(matches!(
            image_barrier(
                vk::Image::null(),
                vk::ImageAspectFlags::COLOR,
                TextureState::Present,
                TextureState::Undefined
            ),
            Err(RhiError::InvalidDescriptor(_))
        ));
    }

    #[test]
    fn barrier_swapchain_sequence() {
        let b = image_barrier(
            vk::Image::null(),
            vk::ImageAspectFlags::COLOR,
            TextureState::Undefined,
            TextureState::ColorAttachment,
        )
        .unwrap();
        assert_eq!(b.src_stage_mask, S::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(b.src_access_mask, A::NONE);
        assert_eq!(b.dst_stage_mask, S::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(
            b.dst_access_mask,
            A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE
        );
        assert_eq!(
            (b.old_layout, b.new_layout),
            (L::UNDEFINED, L::COLOR_ATTACHMENT_OPTIMAL)
        );
        let b = image_barrier(
            vk::Image::null(),
            vk::ImageAspectFlags::COLOR,
            TextureState::ColorAttachment,
            TextureState::Present,
        )
        .unwrap();
        assert_eq!(b.src_stage_mask, S::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(b.src_access_mask, A::COLOR_ATTACHMENT_WRITE);
        assert_eq!((b.dst_stage_mask, b.dst_access_mask), (S::NONE, A::NONE));
        assert_eq!(
            (b.old_layout, b.new_layout),
            (L::COLOR_ATTACHMENT_OPTIMAL, L::PRESENT_SRC_KHR)
        );
        assert_eq!(b.subresource_range.level_count, vk::REMAINING_MIP_LEVELS);
        assert_eq!(b.subresource_range.layer_count, vk::REMAINING_ARRAY_LAYERS);
    }

    fn is_invalid(e: Result<(), RhiError>) -> bool {
        matches!(e, Err(RhiError::InvalidDescriptor(_)))
    }

    #[test]
    fn encoder_state_machine() {
        let mut s = EncoderState::default();
        assert!(!s.end_rendering());
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        assert!(s.begin_rendering(&[Format::Bgra8Srgb], None));
        assert!(!s.begin_rendering(&[Format::Bgra8Srgb], None));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        assert!(!s.check_draw());
        assert!(s.begin_rendering(&[Format::Bgra8Srgb], None));
        assert!(s.end_rendering());
        assert!(is_invalid(s.finish())); // first error (draw outside) wins

        let mut s = EncoderState::default();
        assert!(s.begin_rendering(&[Format::Bgra8Srgb], None));
        assert!(is_invalid(s.finish())); // unbalanced rendering

        let mut s = EncoderState::default();
        s.record(RhiError::DeviceLost);
        s.record(invalid("second"));
        assert!(matches!(s.finish(), Err(RhiError::DeviceLost)));
    }

    #[test]
    fn encoder_pipeline_format_and_labels() {
        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Rgba8Unorm], None);
        let mut p = [None; MAX_COLOR_ATTACHMENTS];
        p[0] = Some(Format::Bgra8Srgb);
        assert!(!s.check_pipeline(&p, None));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Bgra8Srgb], None);
        assert!(s.check_pipeline(&p, None));
        assert!(s.check_draw());
        s.end_rendering();
        s.push_label();
        assert!(s.pop_label());
        assert!(s.finish().is_ok());

        let mut s = EncoderState::default();
        assert!(!s.pop_label());
        assert!(is_invalid(s.finish()));
        let mut s = EncoderState::default();
        s.push_label();
        assert!(is_invalid(s.finish()));
    }

    #[test]
    fn label_truncation() {
        let name = format!("{}é", "a".repeat(126)); // é straddles byte 127
        let buf = encode_label(&name);
        assert_eq!(buf.len(), 128);
        assert_eq!(buf[125], b'a');
        assert_eq!(buf[126], 0);
        assert_eq!(buf[127], 0);
        assert_eq!(buf[..126], *"a".repeat(126).as_bytes());
        assert_eq!(&encode_label("hi")[..3], b"hi\0");
    }

    #[test]
    fn spirv_and_pipeline_validation() {
        assert!(check_spirv(&[]).is_err());
        assert!(check_spirv(&[0x0723_0203, 0, 0, 0]).is_err());
        assert!(check_spirv(&[1, 0, 0, 0, 0]).is_err());
        assert!(check_spirv(&[0x0723_0203, 0, 0, 0, 0]).is_ok());

        let m = ShaderModuleHandle::NULL;
        let mk = |entry: &'static str| rhi::ShaderStageDesc { module: m, entry };
        let formats = [Format::Bgra8Srgb];
        let mut d = GraphicsPipelineDesc {
            label: None,
            vertex: mk("main"),
            fragment: mk("main"),
            color_formats: &formats,
            depth: None,
            topology: PrimitiveTopology::TriangleList,
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            blend: &[],
        };
        assert!(validate_pipeline_desc(&d).is_ok());
        let two = [BlendState::default(), BlendState::default()];
        d.blend = &two;
        assert!(matches!(
            validate_pipeline_desc(&d),
            Err(RhiError::InvalidDescriptor(_))
        ));
        d.blend = &[];
        let ds = |format| {
            Some(DepthState {
                format,
                compare: rhi::CompareOp::Less,
                write: true,
                bias: None,
            })
        };
        d.depth = ds(Format::D32Sfloat);
        assert!(validate_pipeline_desc(&d).is_ok());
        d.depth = ds(Format::Rgba8Unorm);
        assert!(matches!(
            validate_pipeline_desc(&d),
            Err(RhiError::InvalidDescriptor(_))
        ));
        d.depth = None;
        d.vertex = mk("ma\0in");
        assert!(matches!(
            validate_pipeline_desc(&d),
            Err(RhiError::InvalidDescriptor(_))
        ));
    }

    #[test]
    fn buffer_barrier_halves() {
        use BufferState as B;
        use vk::{AccessFlags2 as A, PipelineStageFlags2 as S};
        // A write chains to a read: only the write's access is the source access.
        let b = buffer_barrier2(vk::Buffer::null(), B::ShaderWrite, B::Indirect).unwrap();
        assert_eq!(b.src_stage_mask, S::COMPUTE_SHADER);
        assert!(b.src_access_mask.contains(A::SHADER_STORAGE_WRITE));
        assert_eq!(b.dst_stage_mask, S::DRAW_INDIRECT);
        assert_eq!(b.dst_access_mask, A::INDIRECT_COMMAND_READ);
        // A read as the source has no availability to make: only the execution dependency.
        let b = buffer_barrier2(vk::Buffer::null(), B::ShaderRead, B::TransferDst).unwrap();
        assert_eq!(b.src_access_mask, A::NONE);
        assert_eq!(b.dst_access_mask, A::TRANSFER_WRITE);
        // Undefined orders against nothing, and is not a destination.
        let b = buffer_barrier2(vk::Buffer::null(), B::Undefined, B::ShaderWrite).unwrap();
        assert_eq!((b.src_stage_mask, b.src_access_mask), (S::NONE, A::NONE));
        assert!(buffer_barrier2(vk::Buffer::null(), B::ShaderRead, B::Undefined).is_err());
        for s in [
            B::ShaderRead,
            B::ShaderWrite,
            B::TransferSrc,
            B::TransferDst,
            B::Indirect,
            B::Vertex,
            B::Index,
        ] {
            let (stage, access) = buffer_state_masks(s);
            assert!(!stage.is_empty() && !access.is_empty(), "{s:?}");
        }
        // The storage-write texture state is GENERAL, compute only.
        let h = state_as_destination(TextureState::StorageWrite).unwrap();
        assert_eq!(
            (h.layout, h.stage),
            (vk::ImageLayout::GENERAL, S::COMPUTE_SHADER)
        );
        assert!(
            state_as_source(TextureState::StorageWrite)
                .access
                .contains(A::SHADER_STORAGE_WRITE)
        );
    }

    #[test]
    fn indirect_stride_rules() {
        assert!(check_indirect_stride(20, 100).is_ok());
        assert!(check_indirect_stride(32, 2).is_ok());
        assert!(check_indirect_stride(16, 2).is_err(), "records overlap");
        assert!(
            check_indirect_stride(0, 1).is_ok(),
            "a single draw has no stride"
        );
        assert!(check_indirect_stride(22, 1).is_err(), "unaligned");
    }
}
