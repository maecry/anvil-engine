//! Frame render graph over the Anvil RHI (ADR-0012, `docs/specs/layers/LAYER-006-render-graph.md`).
//!
//! Every frame the app declares passes with their texture reads and attachments on a
//! [`GraphBuilder`], compiles it with [`RenderGraph::compile`] (validate, cull, order, plan
//! barriers and transient slots) and records it with [`RenderGraph::execute`]. The graph emits
//! all barriers, `begin_rendering`/`end_rendering`, viewport and scissor; pass bodies only bind
//! and draw. The persistent [`RenderGraph`] remembers the state of imported textures and pools
//! transient textures across frames. Knows nothing about meshes or materials.

mod describe;
mod plan;
mod pool;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rhi::{
    BufferDesc, BufferHandle, BufferState, ColorAttachment, CommandEncoder, DepthAttachment,
    Device, FrameToken, Memory, Rect2D, RenderingDesc, RhiError, TextureDesc, TextureHandle,
    TextureState, Viewport,
};

pub use describe::{BarrierDesc, GraphDescription, PassDesc, ResourceDesc};
pub use plan::{
    BufId, ColorTarget, DepthTarget, GraphError, Pass, TexId, TransientBufferDesc, TransientDesc,
};

use plan::{Decls, Plan, St, pass_extent};
use pool::TransientPool;

/// Counters of one frame's graph (ADR-0012 §5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphStats {
    /// Declared passes.
    pub passes: u32,
    /// Declared passes dropped because no root depends on them.
    pub culled: u32,
    /// Planned barriers.
    pub barriers: u32,
    /// Declared transient textures that a live pass uses.
    pub transients: u32,
    /// Physical textures needed after lifetime-based sharing.
    pub slots: u32,
    /// CPU time of [`RenderGraph::compile`].
    pub compile_time: Duration,
    /// Device textures created by [`RenderGraph::execute`] (set there).
    pub textures_created: u32,
    /// Pool entries alive after [`RenderGraph::execute`] (set there).
    pub pool_size: u32,
}

/// Per-pass GPU times of one frame, from timestamps (ADR-0013 §2). A pass's time includes the
/// barriers planned before it; the frame total ends after the last export barrier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuTimings {
    /// Frame counter of the measured frame.
    pub frame: u64,
    /// Live passes in execution order with their GPU milliseconds.
    pub passes: Vec<(Cow<'static, str>, f64)>,
    /// First to last timestamp, in milliseconds.
    pub total_ms: f64,
}

/// Body of a pass: binds and draws. Errors are attributed to the pass.
type Body<'a, D> = Box<dyn FnOnce(&mut PassContext<'_, D>) -> Result<(), RhiError> + 'a>;

/// What a pass body can use: the encoder (inside the pass's rendering scope for raster
/// passes), the device, and the physical textures behind graph ids.
pub struct PassContext<'c, D: Device> {
    /// Command encoder.
    pub cmd: &'c mut D::Commands,
    /// The device the graph runs on.
    pub device: &'c D,
    textures: &'c [TextureHandle],
    buffers: &'c [BufferHandle],
}

impl<D: Device> PassContext<'_, D> {
    /// The buffer behind `id`. Only valid for buffers the pass declared.
    pub fn buffer(&self, id: BufId) -> BufferHandle {
        self.buffers[id.0 as usize]
    }

    /// Device address of the buffer behind `id`.
    ///
    /// # Errors
    /// [`RhiError`] from the device.
    pub fn buffer_address(&self, id: BufId) -> Result<u64, RhiError> {
        self.device.buffer_address(self.buffer(id))
    }

    /// The texture behind `id`. Only valid for textures the pass declared.
    pub fn texture(&self, id: TexId) -> TextureHandle {
        self.textures[id.0 as usize]
    }

    /// Bindless slot of the texture behind `id`.
    ///
    /// # Errors
    /// [`RhiError`] from the device (e.g. a texture without `SAMPLED` usage).
    pub fn texture_index(&self, id: TexId) -> Result<u32, RhiError> {
        self.device.texture_index(self.texture(id))
    }
}

/// One frame's declarations: textures, passes with bodies, exports. Bodies are boxed once per
/// pass (the bounded `dyn` exception of ADR-0012 §7) and may borrow frame data for `'a`.
pub struct GraphBuilder<'a, D: Device> {
    decls: Decls,
    bodies: Vec<Body<'a, D>>,
}

impl<'a, D: Device> Default for GraphBuilder<'a, D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, D: Device> GraphBuilder<'a, D> {
    /// An empty frame.
    pub fn new() -> Self {
        Self {
            decls: Decls::default(),
            bodies: Vec::new(),
        }
    }

    /// Declares a transient texture, allocated from the pool when a live pass uses it.
    pub fn transient(&mut self, name: impl Into<Cow<'static, str>>, desc: TransientDesc) -> TexId {
        self.decls.transient(name.into(), desc)
    }

    /// Imports a persistent texture of `extent`. `initial` overrides the state the graph
    /// remembers (use `Some(Undefined)` for a swapchain image); `None` uses the remembered
    /// state and fails compilation if there is none (see [`RenderGraph::track`]). Only imports
    /// with `None` have their final state remembered.
    pub fn import(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        texture: TextureHandle,
        extent: (u32, u32),
        initial: Option<TextureState>,
    ) -> TexId {
        self.decls.import(name.into(), texture, extent, initial)
    }

    /// Declares a transient GPU buffer, allocated from the pool when a live pass uses it. Its
    /// contents are undefined at the start of the frame: a pass must write it before any read.
    pub fn transient_buffer(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        desc: TransientBufferDesc,
    ) -> BufId {
        self.decls.transient_buffer(name.into(), desc)
    }

    /// Imports a persistent buffer. `initial` overrides the state the graph remembers; with
    /// `None` the remembered state is used (`Undefined` the first time) and the final state is
    /// remembered for the next frame.
    pub fn import_buffer(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        buffer: BufferHandle,
        initial: Option<BufferState>,
    ) -> BufId {
        self.decls.import_buffer(name.into(), buffer, initial)
    }

    /// `tex` must end the frame in `state` (e.g. `Present`). Exports are roots.
    pub fn export(&mut self, tex: TexId, state: TextureState) {
        self.decls.exports.push((tex, state));
    }

    /// Adds a pass. Passes run in declaration order; one that reaches no root is culled and
    /// its body dropped unrun.
    pub fn pass(
        &mut self,
        pass: Pass,
        body: impl FnOnce(&mut PassContext<'_, D>) -> Result<(), RhiError> + 'a,
    ) {
        self.decls.passes.push(pass);
        self.bodies.push(Box::new(body));
    }
}

/// A validated, planned frame ready for [`RenderGraph::execute`].
pub struct CompiledGraph<'a, D: Device> {
    decls: Decls,
    plan: Plan,
    bodies: Vec<Option<Body<'a, D>>>,
    stats: GraphStats,
}

impl<D: Device> CompiledGraph<'_, D> {
    /// Compile-time counters.
    pub fn stats(&self) -> GraphStats {
        self.stats
    }

    /// The frame structure as plain data.
    pub fn describe(&self) -> GraphDescription {
        describe::describe(&self.decls, &self.plan)
    }
}

/// Persistent part of the graph: remembered states of imported textures and the transient pool.
pub struct RenderGraph {
    states: HashMap<TextureHandle, TextureState>,
    buffer_states: HashMap<BufferHandle, BufferState>,
    pool: TransientPool<TransientDesc, TextureHandle, TextureState>,
    buffer_pool: TransientPool<TransientBufferDesc, BufferHandle, BufferState>,
    /// Per frame slot: the frame number and pass names its timestamps belong to.
    timed: Vec<Option<(u64, Vec<Cow<'static, str>>)>>,
}

/// Writes the first compiled graph to `ANVIL_GRAPH_DUMP` (`.json`, otherwise DOT).
fn dump_once(desc: &GraphDescription) {
    static DONE: AtomicBool = AtomicBool::new(false);
    let Ok(path) = std::env::var("ANVIL_GRAPH_DUMP") else {
        return;
    };
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let text = if path.ends_with(".json") {
        desc.to_json()
    } else {
        desc.to_dot()
    };
    match std::fs::write(&path, text) {
        Ok(()) => tracing::info!(path, "render graph dumped"),
        Err(e) => tracing::warn!(path, error = %e, "render graph dump failed"),
    }
}

impl RenderGraph {
    /// `frames_in_flight` is the device's setting; it decides when a pooled texture has retired.
    pub fn new(frames_in_flight: u32) -> Self {
        Self {
            states: HashMap::new(),
            buffer_states: HashMap::new(),
            pool: TransientPool::new(frames_in_flight),
            buffer_pool: TransientPool::new(frames_in_flight),
            timed: vec![None; frames_in_flight as usize],
        }
    }

    /// Starts remembering `texture` (e.g. a just-created IBL map) in `state`.
    pub fn track(&mut self, texture: TextureHandle, state: TextureState) {
        self.states.insert(texture, state);
    }

    /// Like [`track`](Self::track) but keeps the state already remembered for `texture`.
    pub fn track_if_new(&mut self, texture: TextureHandle, state: TextureState) {
        self.states.entry(texture).or_insert(state);
    }

    /// Stops remembering `texture`; call before destroying an imported texture.
    pub fn forget(&mut self, texture: TextureHandle) {
        self.states.remove(&texture);
    }

    /// Stops remembering `buffer`; call before destroying an imported buffer.
    pub fn forget_buffer(&mut self, buffer: BufferHandle) {
        self.buffer_states.remove(&buffer);
    }

    /// Validates, culls, orders and plans the frame.
    ///
    /// # Errors
    /// A [`GraphError`] naming the offending pass and texture.
    pub fn compile<'a, D: Device>(
        &self,
        builder: GraphBuilder<'a, D>,
    ) -> Result<CompiledGraph<'a, D>, GraphError> {
        let t0 = Instant::now();
        let GraphBuilder { decls, bodies } = builder;
        let plan = plan::plan(&decls, &self.states, &self.buffer_states)?;
        let culled = plan.live.iter().filter(|l| !**l).count() as u32;
        let stats = GraphStats {
            passes: decls.passes.len() as u32,
            culled,
            barriers: plan.barriers.len() as u32,
            transients: plan.slot_of.iter().filter(|s| s.is_some()).count() as u32,
            slots: plan.slots.len() as u32,
            compile_time: t0.elapsed(),
            ..GraphStats::default()
        };
        tracing::debug!(
            passes = stats.passes,
            culled,
            barriers = stats.barriers,
            transients = stats.transients,
            slots = stats.slots,
            compile_us = stats.compile_time.as_micros() as u64,
            "render graph compiled"
        );
        let compiled = CompiledGraph {
            decls,
            plan,
            bodies: bodies.into_iter().map(Some).collect(),
            stats,
        };
        if tracing::enabled!(tracing::Level::TRACE)
            || std::env::var_os("ANVIL_GRAPH_DUMP").is_some()
        {
            let desc = compiled.describe();
            tracing::trace!(graph = ?desc, "render graph");
            dump_once(&desc);
        }
        Ok(compiled)
    }

    /// Records the compiled frame into `cmd`: per live pass its barriers, label, rendering
    /// scope and body; then the export barriers. `frame` is the token the commands were
    /// begun with. Returns the counters including pool activity.
    ///
    /// # Errors
    /// [`GraphError::Rhi`] from texture creation or a pass body.
    pub fn execute<D: Device>(
        &mut self,
        compiled: CompiledGraph<'_, D>,
        device: &D,
        cmd: &mut D::Commands,
        frame: &FrameToken,
    ) -> Result<GraphStats, GraphError> {
        let CompiledGraph {
            decls,
            plan,
            mut bodies,
            mut stats,
        } = compiled;
        let rhi_err = |pass: &str, source| GraphError::Rhi {
            pass: pass.to_owned(),
            source,
        };
        let mut slot_entry = Vec::with_capacity(plan.slots.len());
        for s in &plan.slots {
            let name = &decls.res[s.first_owner.0 as usize].name;
            let (entry, created) = self
                .pool
                .acquire(s.desc, frame.frame, TextureState::Undefined, || {
                    device.create_texture(&TextureDesc {
                        label: Some(name),
                        width: s.desc.width,
                        height: s.desc.height,
                        format: s.desc.format,
                        usage: s.desc.usage,
                        mip_levels: 1,
                        array_layers: 1,
                        cube: false,
                    })
                })
                .map_err(|e| rhi_err(name, e))?;
            stats.textures_created += u32::from(created);
            slot_entry.push(entry);
        }
        let textures: Vec<TextureHandle> = decls
            .res
            .iter()
            .enumerate()
            .map(|(i, r)| match (&r.kind, plan.slot_of[i]) {
                (plan::ResKind::Imported { texture, .. }, _) => *texture,
                (_, Some(s)) => self.pool.entries[slot_entry[s]].handle,
                _ => TextureHandle::NULL,
            })
            .collect();
        // Transient buffers: one pooled buffer each (no aliasing).
        let mut buffer_entry = vec![usize::MAX; decls.res.len()];
        let mut buffers = vec![BufferHandle::NULL; decls.res.len()];
        for (i, r) in decls.res.iter().enumerate() {
            match r.kind {
                plan::ResKind::ImportedBuf { buffer, .. } => buffers[i] = buffer,
                plan::ResKind::TransientBuf(desc) if plan.lifetime[i].is_some() => {
                    let (e, created) = self
                        .buffer_pool
                        .acquire(desc, frame.frame, BufferState::Undefined, || {
                            device.create_buffer(&BufferDesc {
                                label: Some(&r.name),
                                size: desc.size,
                                usage: desc.usage,
                                memory: Memory::GpuOnly,
                            })
                        })
                        .map_err(|e| rhi_err(&r.name, e))?;
                    stats.textures_created += u32::from(created);
                    buffer_entry[i] = e;
                    buffers[i] = self.buffer_pool.entries[e].handle;
                }
                _ => {}
            }
        }

        let timed = device.timestamp_info().is_some();
        let mut names = Vec::new();
        if timed {
            cmd.write_timestamp(0);
        }
        let mut next_barrier = 0;
        let mut emit = |cmd: &mut D::Commands,
                        pool: &TransientPool<TransientDesc, TextureHandle, TextureState>,
                        bpool: &TransientPool<TransientBufferDesc, BufferHandle, BufferState>,
                        upto: usize| {
            while let Some(b) = plan.barriers.get(next_barrier).filter(|b| b.before <= upto) {
                let r = b.tex.0 as usize;
                match (b.from, b.to) {
                    (St::T(mut from), St::T(to)) => {
                        // A pooled texture's first barrier starts from the state its last
                        // frame left.
                        if from == TextureState::Undefined
                            && let Some(s) = plan.slot_of[r]
                            && plan.slots[s].first_owner == b.tex
                        {
                            from = pool.entries[slot_entry[s]].state;
                        }
                        cmd.texture_barrier(textures[r], from, to);
                    }
                    (St::B(mut from), St::B(to)) => {
                        if from == BufferState::Undefined && buffer_entry[r] != usize::MAX {
                            from = bpool.entries[buffer_entry[r]].state;
                        }
                        cmd.buffer_barrier(buffers[r], from, to);
                    }
                    _ => unreachable!("a resource is either a texture or a buffer"),
                }
                next_barrier += 1;
            }
        };
        for (k, &pi) in plan.order.iter().enumerate() {
            emit(cmd, &self.pool, &self.buffer_pool, k);
            let pass = &decls.passes[pi];
            tracing::trace!(pass = %pass.name, position = k, "graph pass");
            cmd.begin_label(&pass.name, pass.label_color);
            let extent = pass_extent(&decls, pass);
            if let Some((width, height)) = extent {
                let colors: Vec<ColorAttachment> = pass
                    .colors
                    .iter()
                    .map(|(t, c)| ColorAttachment {
                        texture: textures[t.0 as usize],
                        load: c.load,
                        store: c.store,
                        clear: c.clear,
                    })
                    .collect();
                cmd.begin_rendering(&RenderingDesc {
                    color: &colors,
                    depth: pass.depth.map(|(t, d)| DepthAttachment {
                        texture: textures[t.0 as usize],
                        load: d.load,
                        store: d.store,
                        clear: d.clear,
                    }),
                    width,
                    height,
                });
                cmd.set_viewport(Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                });
                cmd.set_scissor(Rect2D {
                    x: 0,
                    y: 0,
                    width,
                    height,
                });
            }
            if let Some(body) = bodies[pi].take() {
                body(&mut PassContext {
                    cmd,
                    device,
                    textures: &textures,
                    buffers: &buffers,
                })
                .map_err(|e| rhi_err(&pass.name, e))?;
            }
            if extent.is_some() {
                cmd.end_rendering();
            }
            cmd.end_label();
            // shortcut: passes past the query pool size (MAX_TIMESTAMPS) are not timed, raise
            // the pool when a graph needs more than 127 live passes.
            if timed && k + 1 < rhi::MAX_TIMESTAMPS as usize - 1 {
                cmd.write_timestamp(k as u32 + 1);
                names.push(pass.name.clone());
            }
        }
        emit(cmd, &self.pool, &self.buffer_pool, usize::MAX);
        if timed {
            cmd.write_timestamp(names.len() as u32 + 1);
            names.push(Cow::Borrowed("export barriers"));
            self.timed[frame.slot as usize] = Some((frame.frame, names));
        }

        for (s, &e) in slot_entry.iter().enumerate() {
            self.pool.entries[e].state = plan.slot_final[s];
        }
        for (i, r) in decls.res.iter().enumerate() {
            if let (St::B(fin), true) = (plan.res_final[i], buffer_entry[i] != usize::MAX) {
                self.buffer_pool.entries[buffer_entry[i]].state = fin;
            }
            if let plan::ResKind::ImportedBuf {
                buffer,
                initial: None,
            } = r.kind
                && plan.lifetime[i].is_some()
                && let St::B(fin) = plan.res_final[i]
            {
                self.buffer_states.insert(buffer, fin);
            }
            // An explicit initial state is re-declared every frame (swapchain images), so it is
            // never remembered; this keeps stale swapchain handles out of the map.
            if let plan::ResKind::Imported {
                texture,
                initial: None,
                ..
            } = r.kind
                && plan.lifetime[i].is_some()
                && let St::T(fin) = plan.res_final[i]
            {
                self.states.insert(texture, fin);
            }
        }
        self.pool.evict(frame.frame, |t| device.destroy_texture(t));
        self.buffer_pool
            .evict(frame.frame, |b| device.destroy_buffer(b));
        stats.pool_size = (self.pool.entries.len() + self.buffer_pool.entries.len()) as u32;
        Ok(stats)
    }

    /// GPU times of the last frame executed in frame slot `slot`. Call after
    /// [`Device::begin_frame`] returned that slot, before submitting the new frame; the result is
    /// `frames_in_flight` frames old. `None` without timestamp support or before the slot's
    /// first frame.
    ///
    /// # Errors
    /// [`RhiError`] from the query readback.
    pub fn gpu_timings<D: Device>(
        &self,
        device: &D,
        slot: u32,
    ) -> Result<Option<GpuTimings>, RhiError> {
        let (Some(info), Some(Some((frame, names)))) =
            (device.timestamp_info(), self.timed.get(slot as usize))
        else {
            return Ok(None);
        };
        let ticks = device.read_timestamps(slot)?;
        if ticks.len() != names.len() + 1 {
            return Ok(None);
        }
        let ms = |a: u64, b: u64| b.wrapping_sub(a) as f64 * f64::from(info.period_ns) * 1e-6;
        Ok(Some(GpuTimings {
            frame: *frame,
            passes: names
                .iter()
                .zip(ticks.windows(2))
                .map(|(n, w)| (n.clone(), ms(w[0], w[1])))
                .collect(),
            total_ms: ms(ticks[0], ticks[ticks.len() - 1]),
        }))
    }

    /// Destroys the pooled textures. Imported textures belong to the caller.
    pub fn destroy<D: Device>(self, device: &D) {
        for e in self.pool.entries {
            device.destroy_texture(e.handle);
        }
        for e in self.buffer_pool.entries {
            device.destroy_buffer(e.handle);
        }
    }
}
