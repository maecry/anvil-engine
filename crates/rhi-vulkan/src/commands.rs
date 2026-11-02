//! MODULE-004: frame slots, timeline sync, command encoder, barriers, shader modules, pipelines.

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use rhi::{
    BlendFactor, BlendOp, BlendState, CommandEncoder, CullMode, Format, FrameToken, FrontFace,
    GraphicsPipelineDesc, LoadOp, PipelineHandle, PrimitiveTopology, Rect2D, RenderingDesc,
    RhiError, ShaderModuleDesc, ShaderModuleHandle, StoreOp, TextureHandle, TextureState, Viewport,
};

use crate::destroy::{Doomed, destroy_doomed};
use crate::device::DeviceShared;
use crate::error::{invalid, map_vk, stale};
use crate::registry::generated_name;
use crate::swapchain::rhi_to_vk_format;
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
    let (src, dst) = (state_as_source(from), state_as_destination(to)?);
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
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: vk::REMAINING_MIP_LEVELS,
            base_array_layer: 0,
            layer_count: vk::REMAINING_ARRAY_LAYERS,
        }))
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
}

impl EncoderState {
    pub(crate) fn failed(&self) -> bool {
        self.first_error.is_some()
    }

    /// Keeps the first error only.
    pub(crate) fn record(&mut self, e: RhiError) {
        self.first_error.get_or_insert(e);
    }

    pub(crate) fn begin_rendering(&mut self, formats: &[Format]) -> bool {
        if self.rendering {
            self.record(invalid("begin_rendering while already rendering"));
            return false;
        }
        self.rendering = true;
        self.pipeline_bound = false;
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

    pub(crate) fn check_pipeline(&mut self, p: &[Option<Format>; MAX_COLOR_ATTACHMENTS]) -> bool {
        if !self.check_inside_rendering("bind_pipeline") {
            return false;
        }
        if self.formats != *p {
            self.record(invalid(
                "pipeline color formats differ from the active rendering",
            ));
            return false;
        }
        self.pipeline_bound = true;
        true
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
            .command_buffer_count(1);
        // SAFETY: `pool` was just created on this device.
        let cb = match unsafe { s.raw.allocate_command_buffers(&cb_ci) } {
            Ok(v) => v[0],
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
        s.set_name(sem, &format!("frame{i}.acquire_semaphore"));
        tracing::debug!(slot = i, "frame slot created");
        Ok(FrameSlot {
            pool,
            cb,
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
            let kind = match &d {
                Doomed::ShaderModule(_) => "shader_module",
                Doomed::Pipeline { .. } => "pipeline",
            };
            // SAFETY: tag retirement proved the GPU no longer uses the object; popped once.
            unsafe { destroy_doomed(&s.raw, d) };
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
        let s = &self.shared;
        let value = self.tracker.next_value();
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.slots[slot].acquire_sem)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(f.cb)];
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
        r.map_err(|r| map_vk("vkQueueSubmit2", r))?;
        self.tracker.record_submit(slot);
        s.sync.last_submitted.store(value, Ordering::Release);
        self.slots[slot].open = false;
        self.acquired = false;
        tracing::trace!(slot, value, "submit");
        Ok(value)
    }

    /// Submit without swapchain semaphores (tests, later headless rendering).
    #[allow(dead_code)]
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
        let image = match read(&self.shared.pools.textures).get(t) {
            Some(tex) => tex.image,
            None => return self.st.record(stale("texture", t)),
        };
        match image_barrier(image, vk::ImageAspectFlags::COLOR, from, to) {
            Ok(b) => {
                let barriers = [b];
                let dep = vk::DependencyInfo::default().image_memory_barriers(&barriers);
                // SAFETY: `cb` is open on this encoder's slot (pool reset this frame, single
                // thread); the image was resolved under the pool lock and outlives the submission.
                unsafe { self.shared.raw.cmd_pipeline_barrier2(self.cb, &dep) };
            }
            Err(e) => self.st.record(e),
        }
    }

    fn begin_rendering(&mut self, desc: &RenderingDesc) {
        if self.st.failed() {
            return;
        }
        if desc.depth.is_some() {
            return self.st.record(RhiError::Unsupported {
                what: "depth attachments (M2)".to_owned(),
            });
        }
        if desc.color.is_empty() || desc.color.len() > MAX_COLOR_ATTACHMENTS {
            return self
                .st
                .record(invalid("color attachment count must be 1..=8"));
        }
        if desc.width == 0 || desc.height == 0 {
            return self.st.record(invalid("rendering area must be nonzero"));
        }
        let mut atts = [vk::RenderingAttachmentInfo::default(); MAX_COLOR_ATTACHMENTS];
        let mut formats = [Format::Bgra8Srgb; MAX_COLOR_ATTACHMENTS];
        {
            let tex = read(&self.shared.pools.textures);
            for (i, c) in desc.color.iter().enumerate() {
                let Some(t) = tex.get(c.texture) else {
                    return self.st.record(stale("texture", c.texture));
                };
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
        }
        let n = desc.color.len();
        if !self.st.begin_rendering(&formats[..n]) {
            return;
        }
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: desc.width,
                height: desc.height,
            },
        };
        let info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&atts[..n]);
        let vp = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: desc.width as f32,
            height: desc.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        // SAFETY: `cb` is open on this encoder's slot; views were resolved under the pool lock
        // and are owned by live swapchains; `info` and `atts` outlive the call.
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
        let (pipeline, formats) = match read(&self.shared.pools.pipelines).get(p) {
            Some(v) => (v.pipeline, v.color_formats),
            None => return self.st.record(stale("pipeline", p)),
        };
        if !self.st.check_pipeline(&formats) {
            return;
        }
        // SAFETY: open command buffer; the pipeline is alive: stale handles are rejected above and
        // a later destroy is deferred past the submission recording it (doom_tag).
        unsafe {
            self.shared
                .raw
                .cmd_bind_pipeline(self.cb, vk::PipelineBindPoint::GRAPHICS, pipeline)
        };
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
    pub layout: vk::PipelineLayout,
    pub color_formats: [Option<Format>; MAX_COLOR_ATTACHMENTS],
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
    if desc.depth_format.is_some() {
        return Err(RhiError::Unsupported {
            what: "depth attachments (M2)".to_owned(),
        });
    }
    let n = desc.color_formats.len();
    if n == 0 || n > MAX_COLOR_ATTACHMENTS {
        return Err(invalid("color format count must be 1..=8"));
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
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(cull)
        .front_face(front)
        .line_width(1.0);
    let ms = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blends);
    let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
    let mut rendering =
        vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&vk_formats);

    // SAFETY: plain creation of an empty layout on a live device.
    let layout = unsafe {
        s.raw
            .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default(), None)
    }
    .map_err(|r| map_vk("vkCreatePipelineLayout", r))?;
    let ci = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_asm)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&ms)
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
            // SAFETY: partially created pipelines and the layout are owned here and unused.
            unsafe {
                for p in made.into_iter().filter(|p| *p != vk::Pipeline::null()) {
                    s.raw.destroy_pipeline(p, None);
                }
                s.raw.destroy_pipeline_layout(layout, None);
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
        layout,
        color_formats,
    });
    let name = desc
        .label
        .map_or_else(|| generated_name("pipeline", h), str::to_owned);
    s.set_name(pipeline, &name);
    s.set_name(layout, &format!("{name}.layout"));
    tracing::debug!(handle = ?h, label = %name, color_formats = ?desc.color_formats, topology = ?desc.topology, "pipeline created");
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
                layout: p.layout,
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

    const ALL: [TextureState; 6] = [
        TextureState::Undefined,
        TextureState::ColorAttachment,
        TextureState::Present,
        TextureState::ShaderRead,
        TextureState::TransferDst,
        TextureState::TransferSrc,
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
    fn barrier_table_total_and_rules() {
        for from in ALL {
            for to in &ALL[1..] {
                let b = image_barrier(vk::Image::null(), vk::ImageAspectFlags::COLOR, from, *to)
                    .unwrap();
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
        assert!(s.begin_rendering(&[Format::Bgra8Srgb]));
        assert!(!s.begin_rendering(&[Format::Bgra8Srgb]));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        assert!(!s.check_draw());
        assert!(s.begin_rendering(&[Format::Bgra8Srgb]));
        assert!(s.end_rendering());
        assert!(is_invalid(s.finish())); // first error (draw outside) wins

        let mut s = EncoderState::default();
        assert!(s.begin_rendering(&[Format::Bgra8Srgb]));
        assert!(is_invalid(s.finish())); // unbalanced rendering

        let mut s = EncoderState::default();
        s.record(RhiError::DeviceLost);
        s.record(invalid("second"));
        assert!(matches!(s.finish(), Err(RhiError::DeviceLost)));
    }

    #[test]
    fn encoder_pipeline_format_and_labels() {
        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Rgba8Unorm]);
        let mut p = [None; MAX_COLOR_ATTACHMENTS];
        p[0] = Some(Format::Bgra8Srgb);
        assert!(!s.check_pipeline(&p));
        assert!(is_invalid(s.finish()));

        let mut s = EncoderState::default();
        s.begin_rendering(&[Format::Bgra8Srgb]);
        assert!(s.check_pipeline(&p));
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
            depth_format: None,
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
        d.depth_format = Some(Format::D32Sfloat);
        assert!(matches!(
            validate_pipeline_desc(&d),
            Err(RhiError::Unsupported { .. })
        ));
        d.depth_format = None;
        d.vertex = mk("ma\0in");
        assert!(matches!(
            validate_pipeline_desc(&d),
            Err(RhiError::InvalidDescriptor(_))
        ));
    }
}
