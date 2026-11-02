//! Pure compile step: declarations in, ordered passes, culling, barriers and transient slots
//! out. No device access, so it is unit-testable (ADR-0012 §5).

use std::borrow::Cow;
use std::collections::HashMap;

use rhi::{
    BufferHandle, BufferState, BufferUsage, Format, LoadOp, StoreOp, TextureHandle, TextureState,
    TextureUsage,
};

/// A texture of the graph under construction, valid for one frame's builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexId(pub(crate) u32);

/// A buffer of the graph under construction, valid for one frame's builder. Shares the index
/// space of [`TexId`] (one list of resources).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufId(pub(crate) u32);

/// Description of a transient buffer (GPU-only memory); also the pool key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransientBufferDesc {
    /// Size in bytes.
    pub size: u64,
    /// Usage flags (include `TRANSFER_DST` when a pass fills it).
    pub usage: BufferUsage,
}

/// How a pass uses a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufMode {
    /// Reads earlier contents.
    Read,
    /// Overwrites; earlier contents are not needed.
    Write,
    /// Writes and needs the earlier contents (atomics, in-place updates).
    Modify,
}

/// A texture or buffer state; the planner tracks both in one list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    T(TextureState),
    B(BufferState),
}

impl St {
    /// State name for dumps.
    pub(crate) fn name(self) -> String {
        match self {
            St::T(t) => format!("{t:?}"),
            St::B(b) => format!("{b:?}"),
        }
    }
}

/// Description of a transient texture; also the pool key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransientDesc {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Texel format.
    pub format: Format,
    /// Usage flags.
    pub usage: TextureUsage,
}

/// Load/store behavior of a color attachment.
#[derive(Clone, Copy, Debug)]
pub struct ColorTarget {
    /// Load operation.
    pub load: LoadOp,
    /// Store operation.
    pub store: StoreOp,
    /// Clear color for [`LoadOp::Clear`].
    pub clear: [f32; 4],
}

impl ColorTarget {
    /// Clear to `color` and store.
    pub fn clear(color: [f32; 4]) -> Self {
        Self {
            load: LoadOp::Clear,
            store: StoreOp::Store,
            clear: color,
        }
    }

    /// Previous contents are not needed; store.
    pub fn dont_care() -> Self {
        Self {
            load: LoadOp::DontCare,
            store: StoreOp::Store,
            clear: [0.0; 4],
        }
    }
}

/// Load/store behavior of the depth attachment.
#[derive(Clone, Copy, Debug)]
pub struct DepthTarget {
    /// Load operation.
    pub load: LoadOp,
    /// Store operation.
    pub store: StoreOp,
    /// Clear depth for [`LoadOp::Clear`].
    pub clear: f32,
}

/// Declaration of one pass: what it samples and what it renders to. With at least one
/// attachment the graph wraps the body in `begin_rendering`/`end_rendering` and sets the
/// viewport and scissor to the attachment extent.
#[derive(Clone, Debug)]
pub struct Pass {
    pub(crate) name: Cow<'static, str>,
    pub(crate) label_color: [f32; 4],
    pub(crate) reads: Vec<TexId>,
    /// Textures written by compute shaders through storage images.
    pub(crate) storage: Vec<TexId>,
    pub(crate) buffers: Vec<(BufId, BufferState, BufMode)>,
    pub(crate) colors: Vec<(TexId, ColorTarget)>,
    pub(crate) depth: Option<(TexId, DepthTarget)>,
    pub(crate) side_effect: bool,
}

impl Pass {
    /// Starts a declaration; `name` is also the debug label.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            label_color: [0.5, 0.5, 0.5, 1.0],
            reads: Vec::new(),
            storage: Vec::new(),
            buffers: Vec::new(),
            colors: Vec::new(),
            depth: None,
            side_effect: false,
        }
    }

    /// Debug label color.
    pub fn label_color(mut self, c: [f32; 4]) -> Self {
        self.label_color = c;
        self
    }

    /// The pass samples `t` in shaders.
    pub fn read(mut self, t: TexId) -> Self {
        self.reads.push(t);
        self
    }

    /// A compute pass writes `t` through a bindless storage image ([`TextureState::StorageWrite`]).
    pub fn write_storage(mut self, t: TexId) -> Self {
        self.storage.push(t);
        self
    }

    /// The pass reads `b` in a read `state` (`ShaderRead`, `Indirect`, `TransferSrc`, ...).
    pub fn read_buffer(mut self, b: BufId, state: BufferState) -> Self {
        debug_assert!(!state.is_write(), "read_buffer with a write state");
        self.buffers.push((b, state, BufMode::Read));
        self
    }

    /// The pass overwrites `b` (`ShaderWrite` or `TransferDst`); earlier contents are not needed.
    pub fn write_buffer(mut self, b: BufId, state: BufferState) -> Self {
        debug_assert!(state.is_write(), "write_buffer with a read state");
        self.buffers.push((b, state, BufMode::Write));
        self
    }

    /// The pass updates `b` in place (`ShaderWrite`): it needs the earlier contents.
    pub fn modify_buffer(mut self, b: BufId, state: BufferState) -> Self {
        debug_assert!(state.is_write(), "modify_buffer with a read state");
        self.buffers.push((b, state, BufMode::Modify));
        self
    }

    /// The pass renders to `t` as a color attachment.
    pub fn color(mut self, t: TexId, target: ColorTarget) -> Self {
        self.colors.push((t, target));
        self
    }

    /// The pass renders to `t` as the depth attachment.
    pub fn depth(mut self, t: TexId, target: DepthTarget) -> Self {
        self.depth = Some((t, target));
        self
    }

    /// The pass is a root: it runs even when nothing reads its outputs.
    pub fn side_effect(mut self) -> Self {
        self.side_effect = true;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ResKind {
    Transient(TransientDesc),
    Imported {
        texture: TextureHandle,
        extent: (u32, u32),
        initial: Option<TextureState>,
    },
    TransientBuf(TransientBufferDesc),
    ImportedBuf {
        buffer: BufferHandle,
        initial: Option<BufferState>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ResDecl {
    pub name: Cow<'static, str>,
    pub kind: ResKind,
}

impl ResDecl {
    pub fn extent(&self) -> (u32, u32) {
        match &self.kind {
            ResKind::Transient(d) => (d.width, d.height),
            ResKind::Imported { extent, .. } => *extent,
            ResKind::TransientBuf(_) | ResKind::ImportedBuf { .. } => (0, 0),
        }
    }
}

/// Everything a frame declares, without the pass bodies.
#[derive(Clone, Debug, Default)]
pub(crate) struct Decls {
    pub res: Vec<ResDecl>,
    pub passes: Vec<Pass>,
    pub exports: Vec<(TexId, TextureState)>,
}

impl Decls {
    pub fn transient(&mut self, name: Cow<'static, str>, desc: TransientDesc) -> TexId {
        self.res.push(ResDecl {
            name,
            kind: ResKind::Transient(desc),
        });
        TexId(self.res.len() as u32 - 1)
    }

    pub fn transient_buffer(
        &mut self,
        name: Cow<'static, str>,
        desc: TransientBufferDesc,
    ) -> BufId {
        self.res.push(ResDecl {
            name,
            kind: ResKind::TransientBuf(desc),
        });
        BufId(self.res.len() as u32 - 1)
    }

    /// Importing the same buffer twice returns the first id.
    pub fn import_buffer(
        &mut self,
        name: Cow<'static, str>,
        buffer: BufferHandle,
        initial: Option<BufferState>,
    ) -> BufId {
        let same =
            |r: &ResDecl| matches!(r.kind, ResKind::ImportedBuf { buffer: b, .. } if b == buffer);
        if let Some(i) = self.res.iter().position(same) {
            return BufId(i as u32);
        }
        self.res.push(ResDecl {
            name,
            kind: ResKind::ImportedBuf { buffer, initial },
        });
        BufId(self.res.len() as u32 - 1)
    }

    /// Importing the same texture twice returns the first id.
    pub fn import(
        &mut self,
        name: Cow<'static, str>,
        texture: TextureHandle,
        extent: (u32, u32),
        initial: Option<TextureState>,
    ) -> TexId {
        let same =
            |r: &ResDecl| matches!(r.kind, ResKind::Imported { texture: t, .. } if t == texture);
        if let Some(i) = self.res.iter().position(same) {
            return TexId(i as u32);
        }
        self.res.push(ResDecl {
            name,
            kind: ResKind::Imported {
                texture,
                extent,
                initial,
            },
        });
        TexId(self.res.len() as u32 - 1)
    }
}

/// A planned state transition, recorded before the live pass at position `before`
/// (`order.len()` = after the last pass).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Barrier {
    pub before: usize,
    /// The texture or buffer (one index space).
    pub tex: TexId,
    pub from: St,
    pub to: St,
}

/// A physical texture shared by transients with disjoint lifetimes.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    pub desc: TransientDesc,
    /// Its first user: the one whose first barrier starts from the pooled texture's real state.
    pub first_owner: TexId,
    free_after: usize,
}

/// Output of [`plan`].
#[derive(Debug)]
pub(crate) struct Plan {
    /// Per declared pass: reaches a root.
    pub live: Vec<bool>,
    /// Live passes in execution (declaration) order.
    pub order: Vec<usize>,
    pub barriers: Vec<Barrier>,
    /// Per resource: first and last position in `order` that uses it.
    pub lifetime: Vec<Option<(usize, usize)>>,
    /// Per resource: its slot (transients with a lifetime only).
    pub slot_of: Vec<Option<usize>>,
    pub slots: Vec<Slot>,
    /// State of each slot at the end of the frame.
    pub slot_final: Vec<TextureState>,
    /// State of each resource at the end of the frame.
    pub res_final: Vec<St>,
}

/// Why a graph does not compile or run. Messages name the pass and the resource.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// A pass samples or loads a texture no earlier pass wrote.
    #[error("pass `{pass}` reads `{resource}` before any pass wrote it{}", later(.later_writer))]
    ReadBeforeWrite {
        /// Reading pass.
        pass: String,
        /// Texture.
        resource: String,
        /// A pass declared after the reader that writes it, if any.
        later_writer: Option<String>,
    },
    /// A pass samples a texture it also renders to.
    #[error("pass `{pass}` both reads and writes `{resource}`")]
    ReadAndWrite {
        /// Pass.
        pass: String,
        /// Texture.
        resource: String,
    },
    /// A texture is bound twice in one pass.
    #[error("pass `{pass}` binds `{resource}` more than once")]
    BoundTwice {
        /// Pass.
        pass: String,
        /// Texture.
        resource: String,
    },
    /// A pass uses an imported texture whose state is unknown.
    #[error("pass `{pass}` uses imported `{resource}` which has no initial state")]
    NoInitialState {
        /// Pass.
        pass: String,
        /// Texture.
        resource: String,
    },
    /// Attachments of one pass differ in size.
    #[error("pass `{pass}`: attachment `{resource}` is {found:?}, expected {expected:?}")]
    ExtentMismatch {
        /// Pass.
        pass: String,
        /// Texture.
        resource: String,
        /// Extent of the first attachment.
        expected: (u32, u32),
        /// Extent of this one.
        found: (u32, u32),
    },
    /// An exported texture has no content to export.
    #[error("exported `{resource}` is never written")]
    ExportUnwritten {
        /// Texture.
        resource: String,
    },
    /// A pass body or the device failed.
    #[error("pass `{pass}` failed: {source}")]
    Rhi {
        /// Pass.
        pass: String,
        /// Underlying error.
        source: rhi::RhiError,
    },
}

fn later(w: &Option<String>) -> String {
    w.as_ref()
        .map_or(String::new(), |p| format!(" (pass `{p}` writes it later)"))
}

/// One texture use of a pass.
struct Use {
    tex: TexId,
    state: St,
    /// Needs earlier contents (sampled, or `LoadOp::Load`).
    needs_content: bool,
    write: bool,
}

fn uses(p: &Pass) -> impl Iterator<Item = Use> + '_ {
    let reads = p.reads.iter().map(|&tex| Use {
        tex,
        state: St::T(TextureState::ShaderRead),
        needs_content: true,
        write: false,
    });
    let storage = p.storage.iter().map(|&tex| Use {
        tex,
        state: St::T(TextureState::StorageWrite),
        needs_content: false,
        write: true,
    });
    let colors = p.colors.iter().map(|(tex, t)| Use {
        tex: *tex,
        state: St::T(TextureState::ColorAttachment),
        needs_content: t.load == LoadOp::Load,
        write: true,
    });
    let depth = p.depth.iter().map(|(tex, t)| Use {
        tex: *tex,
        state: St::T(TextureState::DepthAttachment),
        needs_content: t.load == LoadOp::Load,
        write: true,
    });
    let buffers = p.buffers.iter().map(|&(b, state, mode)| Use {
        tex: TexId(b.0),
        state: St::B(state),
        needs_content: mode != BufMode::Write,
        write: mode != BufMode::Read,
    });
    reads
        .chain(storage)
        .chain(colors)
        .chain(depth)
        .chain(buffers)
}

/// Extent of the pass's attachments, `None` for a pass without any.
pub(crate) fn pass_extent(d: &Decls, p: &Pass) -> Option<(u32, u32)> {
    let first = p.colors.first().map(|c| c.0).or(p.depth.map(|d| d.0))?;
    Some(d.res[first.0 as usize].extent())
}

/// Sampled and rendered-to textures of a pass.
pub(crate) fn pass_io(p: &Pass) -> (Vec<TexId>, Vec<TexId>) {
    let writes = p
        .colors
        .iter()
        .map(|c| c.0)
        .chain(p.depth.map(|d| d.0))
        .chain(p.storage.iter().copied());
    let mut reads = p.reads.clone();
    let mut writes: Vec<TexId> = writes.collect();
    for &(b, _, mode) in &p.buffers {
        let id = TexId(b.0);
        if mode != BufMode::Write {
            reads.push(id);
        }
        if mode != BufMode::Read {
            writes.push(id);
        }
    }
    (reads, writes)
}

fn validate(d: &Decls) -> Result<(), GraphError> {
    let name = |t: TexId| d.res[t.0 as usize].name.to_string();
    for p in &d.passes {
        let mut seen: Vec<(TexId, bool)> = Vec::new();
        for u in uses(p) {
            if let Some(&(_, was_write)) = seen.iter().find(|s| s.0 == u.tex) {
                let (pass, resource) = (p.name.to_string(), name(u.tex));
                return Err(if was_write != u.write {
                    GraphError::ReadAndWrite { pass, resource }
                } else {
                    GraphError::BoundTwice { pass, resource }
                });
            }
            seen.push((u.tex, u.write));
        }
        if let Some(expected) = pass_extent(d, p) {
            let atts = p.colors.iter().map(|c| c.0).chain(p.depth.map(|d| d.0));
            for t in atts {
                let found = d.res[t.0 as usize].extent();
                if found != expected {
                    return Err(GraphError::ExtentMismatch {
                        pass: p.name.to_string(),
                        resource: name(t),
                        expected,
                        found,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Compiles `d`: validate, cull, order, plan transient slots, then plan barriers while
/// checking the dataflow. `known` holds the remembered state of imported textures.
pub(crate) fn plan(
    d: &Decls,
    known: &HashMap<TextureHandle, TextureState>,
    known_bufs: &HashMap<BufferHandle, BufferState>,
) -> Result<Plan, GraphError> {
    validate(d)?;
    let (n, np) = (d.res.len(), d.passes.len());

    // Cull: a pass is live when a root depends on it through the last writer of each input.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); np];
    let mut last_writer: Vec<Option<usize>> = vec![None; n];
    for (i, p) in d.passes.iter().enumerate() {
        for u in uses(p) {
            if u.needs_content
                && let Some(w) = last_writer[u.tex.0 as usize]
            {
                deps[i].push(w);
            }
        }
        for u in uses(p).filter(|u| u.write) {
            last_writer[u.tex.0 as usize] = Some(i);
        }
    }
    let mut live: Vec<bool> = d.passes.iter().map(|p| p.side_effect).collect();
    for (t, _) in &d.exports {
        if let Some(w) = last_writer[t.0 as usize] {
            live[w] = true;
        }
    }
    for i in (0..np).rev() {
        if live[i] {
            for &w in &deps[i] {
                live[w] = true;
            }
        }
    }
    let order: Vec<usize> = (0..np).filter(|&i| live[i]).collect();

    // Lifetimes over live positions; exports live to the end.
    let mut lifetime: Vec<Option<(usize, usize)>> = vec![None; n];
    for (k, &pi) in order.iter().enumerate() {
        for u in uses(&d.passes[pi]) {
            let l = &mut lifetime[u.tex.0 as usize];
            *l = Some(l.map_or((k, k), |(a, _)| (a, k)));
        }
    }
    for (t, _) in &d.exports {
        if let Some(l) = &mut lifetime[t.0 as usize] {
            l.1 = order.len();
        }
    }

    // Slots: a transient takes a free slot of the same desc whose owner's last use is earlier.
    let mut by_start: Vec<usize> = (0..n)
        .filter(|&r| matches!(d.res[r].kind, ResKind::Transient(_)) && lifetime[r].is_some())
        .collect();
    by_start.sort_by_key(|&r| lifetime[r].map(|l| l.0));
    let mut slots: Vec<Slot> = Vec::new();
    let mut slot_of = vec![None; n];
    for r in by_start {
        let ResKind::Transient(desc) = d.res[r].kind else {
            continue;
        };
        let (first, last) = lifetime[r].unwrap_or_default();
        let s = match slots
            .iter()
            .position(|s| s.desc == desc && s.free_after < first)
        {
            Some(s) => s,
            None => {
                slots.push(Slot {
                    desc,
                    first_owner: TexId(r as u32),
                    free_after: 0,
                });
                slots.len() - 1
            }
        };
        slots[s].free_after = last;
        slot_of[r] = Some(s);
    }

    // Barriers. Physical state lives per import and per slot.
    let phys = |r: usize| slot_of[r].map_or(r, |s| n + s);
    let mut state = vec![St::T(TextureState::Undefined); n + slots.len()];
    for (i, r) in d.res.iter().enumerate() {
        if matches!(
            r.kind,
            ResKind::TransientBuf(_) | ResKind::ImportedBuf { .. }
        ) {
            state[i] = St::B(BufferState::Undefined);
        }
    }
    let mut has_state = vec![true; n];
    let mut written = vec![false; n];
    for (i, r) in d.res.iter().enumerate() {
        match &r.kind {
            ResKind::Imported {
                texture, initial, ..
            } => match initial.or(known.get(texture).copied()) {
                Some(s) => {
                    state[i] = St::T(s);
                    written[i] = s != TextureState::Undefined;
                }
                None => has_state[i] = false,
            },
            // A persistent buffer always has contents; an unknown state starts Undefined.
            ResKind::ImportedBuf { buffer, initial } => {
                state[i] = St::B(
                    initial
                        .or(known_bufs.get(buffer).copied())
                        .unwrap_or(BufferState::Undefined),
                );
                written[i] = true;
            }
            _ => {}
        }
    }
    let mut barriers = Vec::new();
    for (k, &pi) in order.iter().enumerate() {
        let p = &d.passes[pi];
        for u in uses(p) {
            let r = u.tex.0 as usize;
            // Strings are built only on the error paths.
            let names = || (p.name.to_string(), d.res[r].name.to_string());
            if !has_state[r] {
                let (pass, resource) = names();
                return Err(GraphError::NoInitialState { pass, resource });
            }
            if u.needs_content && !written[r] {
                let (pass, resource) = names();
                let later_writer = d.passes[pi + 1..]
                    .iter()
                    .find(|q| uses(q).any(|v| v.write && v.tex == u.tex))
                    .map(|q| q.name.to_string());
                return Err(GraphError::ReadBeforeWrite {
                    pass,
                    resource,
                    later_writer,
                });
            }
            let cur = state[phys(r)];
            // Consecutive writes in the same state still need an execution dependency.
            if cur != u.state || u.write {
                barriers.push(Barrier {
                    before: k,
                    tex: u.tex,
                    from: cur,
                    to: u.state,
                });
                state[phys(r)] = u.state;
            }
        }
        for u in uses(p).filter(|u| u.write) {
            written[u.tex.0 as usize] = true;
        }
    }
    for &(t, fin) in &d.exports {
        let r = t.0 as usize;
        if !written[r] && fin != TextureState::Undefined {
            return Err(GraphError::ExportUnwritten {
                resource: d.res[r].name.to_string(),
            });
        }
        let cur = state[phys(r)];
        if cur != St::T(fin) {
            barriers.push(Barrier {
                before: order.len(),
                tex: t,
                from: cur,
                to: St::T(fin),
            });
            state[phys(r)] = St::T(fin);
        }
    }
    let res_final = (0..n).map(|r| state[phys(r)]).collect();
    Ok(Plan {
        live,
        order,
        barriers,
        lifetime,
        slot_of,
        slot_final: state[n..]
            .iter()
            .map(|s| match s {
                St::T(t) => *t,
                St::B(_) => TextureState::Undefined,
            })
            .collect(),
        res_final,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextureState::{
        ColorAttachment as Col, DepthAttachment as Dep, Present, ShaderRead as Sr, Undefined as Un,
    };
    use rhi::TextureTag;

    fn handles(n: usize) -> Vec<TextureHandle> {
        let mut p = foundation::Pool::<TextureTag>::new();
        (0..n).map(|_| p.insert(TextureTag)).collect()
    }

    fn desc(w: u32) -> TransientDesc {
        TransientDesc {
            width: w,
            height: w,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        }
    }

    fn tr(d: &mut Decls, n: &'static str, w: u32) -> TexId {
        d.transient(n.into(), desc(w))
    }

    fn run(d: &Decls) -> Result<Plan, GraphError> {
        plan(d, &HashMap::new(), &HashMap::new())
    }

    fn names(d: &Decls, p: &Plan) -> Vec<String> {
        p.order
            .iter()
            .map(|&i| d.passes[i].name.to_string())
            .collect()
    }

    fn bar(p: &Plan, tex: TexId) -> Vec<(usize, TextureState, TextureState)> {
        p.barriers
            .iter()
            .filter(|b| b.tex == tex)
            .map(|b| (b.before, tex_of(b.from), tex_of(b.to)))
            .collect()
    }

    fn tex_of(s: St) -> TextureState {
        match s {
            St::T(t) => t,
            St::B(b) => panic!("buffer state {b:?} where a texture state was expected"),
        }
    }

    fn buf_bar(p: &Plan, b: BufId) -> Vec<(usize, BufferState, BufferState)> {
        p.barriers
            .iter()
            .filter(|x| x.tex.0 == b.0)
            .map(|x| match (x.from, x.to) {
                (St::B(f), St::B(t)) => (x.before, f, t),
                _ => panic!("texture barrier on a buffer"),
            })
            .collect()
    }

    /// shadow -> forward(reads shadow, writes hdr) -> tonemap(reads hdr, writes swapchain).
    fn frame() -> (Decls, TexId, TexId, TexId) {
        let h = handles(2);
        let mut d = Decls::default();
        let shadow = d.import("shadow".into(), h[0], (16, 16), Some(Sr));
        let hdr = tr(&mut d, "hdr", 16);
        let sc = d.import("swapchain".into(), h[1], (16, 16), Some(Un));
        d.passes.push(Pass::new("shadow").depth(
            shadow,
            DepthTarget {
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: 1.0,
            },
        ));
        d.passes.push(
            Pass::new("forward")
                .read(shadow)
                .color(hdr, ColorTarget::clear([0.0; 4])),
        );
        d.passes.push(
            Pass::new("tonemap")
                .read(hdr)
                .color(sc, ColorTarget::dont_care()),
        );
        d.exports.push((sc, Present));
        (d, shadow, hdr, sc)
    }

    #[test]
    fn declaration_order_and_barriers() {
        let (d, shadow, hdr, sc) = frame();
        let p = run(&d).unwrap();
        assert_eq!(names(&d, &p), ["shadow", "forward", "tonemap"]);
        assert_eq!(bar(&p, shadow), [(0, Sr, Dep), (1, Dep, Sr)]);
        assert_eq!(bar(&p, hdr), [(1, Un, Col), (2, Col, Sr)]);
        assert_eq!(bar(&p, sc), [(2, Un, Col), (3, Col, Present)]);
        assert_eq!(p.barriers.last().map(|b| b.before), Some(p.order.len()));
    }

    #[test]
    fn unreachable_passes_are_culled() {
        let (mut d, _, hdr, _) = frame();
        let dead = tr(&mut d, "dead", 16);
        d.passes.push(
            Pass::new("orphan")
                .read(hdr)
                .color(dead, ColorTarget::dont_care()),
        );
        let p = run(&d).unwrap();
        assert_eq!(names(&d, &p), ["shadow", "forward", "tonemap"]);
        assert_eq!(p.live, [true, true, true, false]);
        assert!(p.lifetime[dead.0 as usize].is_none() && p.slot_of[dead.0 as usize].is_none());
        // A side effect keeps a pass without readers.
        d.passes[3] = d.passes[3].clone().side_effect();
        assert_eq!(run(&d).unwrap().order.len(), 4);
    }

    #[test]
    fn cull_follows_dependencies_transitively() {
        let mut d = Decls::default();
        let (a, b) = (tr(&mut d, "a", 8), tr(&mut d, "b", 8));
        d.passes
            .push(Pass::new("wa").color(a, ColorTarget::dont_care()));
        d.passes
            .push(Pass::new("wb").read(a).color(b, ColorTarget::dont_care()));
        d.passes.push(Pass::new("root").read(b).side_effect());
        assert_eq!(run(&d).unwrap().order, [0, 1, 2]);
    }

    #[test]
    fn same_state_writes_and_reads_barrier_correctly() {
        let mut d = Decls::default();
        let a = tr(&mut d, "a", 8);
        d.passes
            .push(Pass::new("w1").color(a, ColorTarget::dont_care()));
        d.passes.push(Pass::new("w2").color(
            a,
            ColorTarget {
                load: LoadOp::Load,
                ..ColorTarget::dont_care()
            },
        ));
        d.passes.push(Pass::new("r1").read(a).side_effect());
        d.passes.push(Pass::new("r2").read(a).side_effect());
        let p = run(&d).unwrap();
        // WAW needs a barrier, a second read does not.
        assert_eq!(bar(&p, a), [(0, Un, Col), (1, Col, Col), (2, Col, Sr)]);
    }

    #[test]
    fn remembered_import_state_is_used() {
        let (d, shadow, ..) = frame();
        let mut d2 = d.clone();
        let h = match d2.res[shadow.0 as usize].kind {
            ResKind::Imported { texture, .. } => texture,
            _ => unreachable!(),
        };
        d2.res[shadow.0 as usize].kind = ResKind::Imported {
            texture: h,
            extent: (16, 16),
            initial: None,
        };
        let known = HashMap::from([(h, Sr)]);
        let p = plan(&d2, &known, &HashMap::new()).unwrap();
        assert_eq!(bar(&p, shadow)[0], (0, Sr, Dep));
        assert_eq!(p.res_final[shadow.0 as usize], St::T(Sr));
        assert!(matches!(run(&d2), Err(GraphError::NoInitialState { .. })));
    }

    fn msg(r: Result<Plan, GraphError>) -> String {
        r.unwrap_err().to_string()
    }

    #[test]
    fn validation_errors_name_pass_and_resource() {
        let (mut d, _, hdr, _) = frame();
        // Read of something nobody wrote, with a later writer.
        let mut late = d.clone();
        let x = tr(&mut late, "x", 16);
        late.passes
            .insert(0, Pass::new("early").read(x).side_effect());
        late.passes.push(
            Pass::new("writer")
                .color(x, ColorTarget::dont_care())
                .side_effect(),
        );
        assert_eq!(
            msg(run(&late)),
            "pass `early` reads `x` before any pass wrote it (pass `writer` writes it later)"
        );
        // Read and write in one pass.
        d.passes[1] = Pass::new("forward")
            .read(hdr)
            .color(hdr, ColorTarget::clear([0.0; 4]));
        assert_eq!(msg(run(&d)), "pass `forward` both reads and writes `hdr`");
        d.passes[1] = Pass::new("forward").read(hdr).read(hdr);
        assert_eq!(msg(run(&d)), "pass `forward` binds `hdr` more than once");
        // Mismatched attachment sizes.
        let (mut d, _, hdr, sc) = frame();
        let small = tr(&mut d, "small", 8);
        d.passes[2] = Pass::new("tonemap")
            .read(hdr)
            .color(sc, ColorTarget::dont_care())
            .color(small, ColorTarget::dont_care());
        assert_eq!(
            msg(run(&d)),
            "pass `tonemap`: attachment `small` is (8, 8), expected (16, 16)"
        );
        // Loading an undefined import.
        let (mut d, _, hdr, sc) = frame();
        d.passes[2] = Pass::new("tonemap").read(hdr).color(
            sc,
            ColorTarget {
                load: LoadOp::Load,
                ..ColorTarget::dont_care()
            },
        );
        assert_eq!(
            msg(run(&d)),
            "pass `tonemap` reads `swapchain` before any pass wrote it"
        );
        // Export with no writer.
        let mut d = Decls::default();
        let u = tr(&mut d, "never", 8);
        d.exports.push((u, Sr));
        assert_eq!(msg(run(&d)), "exported `never` is never written");
    }

    #[test]
    fn import_without_state_is_rejected_by_name() {
        let h = handles(1);
        let mut d = Decls::default();
        let t = d.import("history".into(), h[0], (8, 8), None);
        d.passes.push(
            Pass::new("taa")
                .color(t, ColorTarget::dont_care())
                .side_effect(),
        );
        assert_eq!(
            msg(run(&d)),
            "pass `taa` uses imported `history` which has no initial state"
        );
    }

    #[test]
    fn transients_with_disjoint_lifetimes_share_a_slot() {
        let mut d = Decls::default();
        let (a, b, c, e) = (
            tr(&mut d, "a", 8),
            tr(&mut d, "b", 8),
            tr(&mut d, "c", 8),
            tr(&mut d, "e", 16),
        );
        d.passes
            .push(Pass::new("p0").color(a, ColorTarget::dont_care()));
        d.passes
            .push(Pass::new("p1").read(a).color(b, ColorTarget::dont_care()));
        d.passes
            .push(Pass::new("p2").read(b).color(c, ColorTarget::dont_care()));
        d.passes
            .push(Pass::new("p3").read(c).color(e, ColorTarget::dont_care()));
        d.passes.push(Pass::new("p4").read(e).side_effect());
        let p = run(&d).unwrap();
        let s = |t: TexId| p.slot_of[t.0 as usize].unwrap();
        // a: 0..1, b: 1..2, c: 2..3. c reuses a (a ended at 1 < 2); b overlaps both; e differs in desc.
        assert_eq!(s(a), s(c));
        assert_ne!(s(a), s(b));
        assert_ne!(s(e), s(a));
        assert_eq!(p.slots.len(), 3);
        // The reuse barrier continues from the previous owner's last state.
        assert_eq!(bar(&p, c), [(2, Sr, Col), (3, Col, Sr)]);
        // Exported transients are never recycled.
        d.exports.push((a, Sr));
        assert_ne!(run(&d).unwrap().slots.len(), 2);
    }

    use BufferState as B;

    fn tbuf(d: &mut Decls, n: &'static str) -> BufId {
        d.transient_buffer(
            n.into(),
            TransientBufferDesc {
                size: 256,
                usage: BufferUsage::STORAGE | BufferUsage::INDIRECT | BufferUsage::TRANSFER_DST,
            },
        )
    }

    /// reset (fill) -> cull (modify counts, write commands) -> draw (indirect reads) -> draw 2.
    #[test]
    fn buffer_barriers_for_a_culling_frame() {
        let h = handles(1);
        let mut d = Decls::default();
        let counts = tbuf(&mut d, "counts");
        let cmds = tbuf(&mut d, "cmds");
        let hdr = d.import("hdr".into(), h[0], (8, 8), Some(Un));
        d.passes
            .push(Pass::new("reset").write_buffer(counts, B::TransferDst));
        d.passes.push(
            Pass::new("cull")
                .modify_buffer(counts, B::ShaderWrite)
                .write_buffer(cmds, B::ShaderWrite),
        );
        for (name, load) in [("draw1", LoadOp::DontCare), ("draw2", LoadOp::Load)] {
            d.passes.push(
                Pass::new(name)
                    .read_buffer(cmds, B::Indirect)
                    .read_buffer(counts, B::Indirect)
                    .color(
                        hdr,
                        ColorTarget {
                            load,
                            ..ColorTarget::dont_care()
                        },
                    ),
            );
        }
        d.exports.push((hdr, Sr));
        let p = run(&d).unwrap();
        assert_eq!(names(&d, &p), ["reset", "cull", "draw1", "draw2"]);
        // The first use needs no barrier source; writes are chained; a second reader is free.
        assert_eq!(
            buf_bar(&p, counts),
            [
                (0, B::Undefined, B::TransferDst),
                (1, B::TransferDst, B::ShaderWrite),
                (2, B::ShaderWrite, B::Indirect)
            ]
        );
        assert_eq!(
            buf_bar(&p, cmds),
            [
                (1, B::Undefined, B::ShaderWrite),
                (2, B::ShaderWrite, B::Indirect)
            ]
        );
        assert_eq!(p.res_final[cmds.0 as usize], St::B(B::Indirect));
    }

    #[test]
    fn consecutive_buffer_writes_and_read_state_changes_barrier() {
        let mut d = Decls::default();
        let b = tbuf(&mut d, "b");
        d.passes
            .push(Pass::new("w1").write_buffer(b, B::ShaderWrite));
        d.passes
            .push(Pass::new("w2").modify_buffer(b, B::ShaderWrite));
        d.passes
            .push(Pass::new("r1").read_buffer(b, B::ShaderRead).side_effect());
        d.passes
            .push(Pass::new("r2").read_buffer(b, B::ShaderRead).side_effect());
        d.passes
            .push(Pass::new("r3").read_buffer(b, B::Indirect).side_effect());
        d.passes.push(
            Pass::new("w3")
                .write_buffer(b, B::TransferDst)
                .side_effect(),
        );
        let p = run(&d).unwrap();
        assert_eq!(
            buf_bar(&p, b),
            [
                (0, B::Undefined, B::ShaderWrite),
                (1, B::ShaderWrite, B::ShaderWrite),
                (2, B::ShaderWrite, B::ShaderRead),
                (4, B::ShaderRead, B::Indirect),
                (5, B::Indirect, B::TransferDst)
            ]
        );
    }

    #[test]
    fn buffer_dataflow_errors_and_culling() {
        // Reading a transient buffer nobody wrote.
        let mut d = Decls::default();
        let b = tbuf(&mut d, "scratch");
        d.passes.push(
            Pass::new("reader")
                .read_buffer(b, B::ShaderRead)
                .side_effect(),
        );
        assert_eq!(
            msg(run(&d)),
            "pass `reader` reads `scratch` before any pass wrote it"
        );
        // Modifying one needs contents as well; a pure write does not.
        let mut d = Decls::default();
        let b = tbuf(&mut d, "scratch");
        d.passes.push(
            Pass::new("rmw")
                .modify_buffer(b, B::ShaderWrite)
                .side_effect(),
        );
        assert!(matches!(run(&d), Err(GraphError::ReadBeforeWrite { .. })));
        // Twice in one pass.
        let mut d = Decls::default();
        let b = tbuf(&mut d, "scratch");
        d.passes.push(
            Pass::new("p")
                .write_buffer(b, B::ShaderWrite)
                .read_buffer(b, B::ShaderRead)
                .side_effect(),
        );
        assert_eq!(msg(run(&d)), "pass `p` both reads and writes `scratch`");
        // A cull pass whose output nobody reads is dropped; one feeding a live pass is kept.
        let mut d = Decls::default();
        let (dead, live) = (tbuf(&mut d, "dead"), tbuf(&mut d, "live"));
        d.passes
            .push(Pass::new("dead_cull").write_buffer(dead, B::ShaderWrite));
        d.passes
            .push(Pass::new("live_cull").write_buffer(live, B::ShaderWrite));
        d.passes.push(
            Pass::new("draw")
                .read_buffer(live, B::Indirect)
                .side_effect(),
        );
        assert_eq!(names(&d, &run(&d).unwrap()), ["live_cull", "draw"]);
    }

    #[test]
    fn imported_buffer_state_is_remembered_and_never_unknown() {
        let mut p = foundation::Pool::<rhi::BufferTag>::new();
        let h = p.insert(rhi::BufferTag);
        let mut d = Decls::default();
        let b = d.import_buffer("instances".into(), h, None);
        assert_eq!(d.import_buffer("again".into(), h, None), b, "deduplicated");
        d.passes.push(
            Pass::new("cull")
                .read_buffer(b, B::ShaderRead)
                .side_effect(),
        );
        // Unknown: starts Undefined (no error); remembered: continues from it.
        let first = plan(&d, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(buf_bar(&first, b), [(0, B::Undefined, B::ShaderRead)]);
        let known = HashMap::from([(h, B::TransferDst)]);
        let next = plan(&d, &HashMap::new(), &known).unwrap();
        assert_eq!(buf_bar(&next, b), [(0, B::TransferDst, B::ShaderRead)]);
        assert_eq!(next.res_final[b.0 as usize], St::B(B::ShaderRead));
    }

    /// The depth pyramid: each mip view is its own graph texture. Mip k is written by a compute
    /// pass (StorageWrite) and sampled by the pass that builds mip k+1, so only the written mip
    /// ever transitions; the consumer samples all of them.
    #[test]
    fn per_mip_views_are_tracked_independently() {
        let h = handles(4);
        let mut d = Decls::default();
        let mips: Vec<TexId> = (0..3)
            .map(|i| d.import(format!("mip{i}").into(), h[i], (8 >> i, 8 >> i), None))
            .collect();
        let depth = d.import("depth".into(), h[3], (16, 16), Some(Dep));
        d.passes
            .push(Pass::new("hiz0").read(depth).write_storage(mips[0]));
        for i in 1..3 {
            d.passes.push(
                Pass::new(format!("hiz{i}"))
                    .read(mips[i - 1])
                    .write_storage(mips[i]),
            );
        }
        d.passes.push(
            Pass::new("cull2")
                .read(mips[0])
                .read(mips[1])
                .read(mips[2])
                .side_effect(),
        );
        // Last frame left every mip sampled.
        let known: HashMap<_, _> = h[..3].iter().map(|&t| (t, Sr)).collect();
        let p = plan(&d, &known, &HashMap::new()).unwrap();
        assert_eq!(names(&d, &p), ["hiz0", "hiz1", "hiz2", "cull2"]);
        let sw = TextureState::StorageWrite;
        assert_eq!(bar(&p, depth), [(0, Dep, Sr)]);
        assert_eq!(bar(&p, mips[0]), [(0, Sr, sw), (1, sw, Sr)]);
        assert_eq!(bar(&p, mips[1]), [(1, Sr, sw), (2, sw, Sr)]);
        // The last mip is only sampled by the consumer.
        assert_eq!(bar(&p, mips[2]), [(2, Sr, sw), (3, sw, Sr)]);
        // A storage write does not need earlier contents; sampling a never-written mip fails.
        let mut d2 = d.clone();
        d2.passes.remove(0);
        let e = plan(&d2, &HashMap::new(), &HashMap::new());
        assert!(matches!(e, Err(GraphError::NoInitialState { .. })));
    }
}
