# LAYER-001: Foundation

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1 (foundation); math / task deferred |
| Crates | foundation (math, task deferred) |
| Traces to | MASTER-000 §5.1 (Layer 1), §8.1, §10.2, §11, §16, §22 |
| Last updated | 2026-11-02 |

## 1. Purpose

Small, dependency-free base shared by all crates: typed generational handles and the pool that backs them, frame/timing basics, logging setup, and plain-data diagnostic types. Nothing here knows about windows, GPUs or scenes.

## 2. Scope

| In | Out |
|---|---|
| `Handle<T>`, `Pool<T>` (ADR-0006) | Windowing, input (LAYER-002) |
| Logging init helper on `tracing` | Any GPU concept (LAYER-003) |
| `FrameCounter`, `Clock` (monotonic time, delta) | Math types (math, deferred; use `glam` directly until it exists) |
| Plain-data diagnostics types (`GpuScopeSample`, `MemorySnapshot`) | Task scheduler (task, deferred) |
| Engine configuration primitives (`EngineConfig` basics) | ECS, assets, serialization formats |

## 3. Crates

| Crate | State | Notes |
|---|---|---|
| foundation | stub, implemented in M1 | Depends on no anvil crate. External: `tracing`, `thiserror`; `tracing-subscriber` behind feature `log-init` |
| math | deferred | Introduce when a second crate needs math beyond `glam` re-exports (first likely M3 camera/transforms). Wrapper over `glam` plus conventions (handedness, depth range, Y direction) |
| task | deferred | Introduce at M5 (multithreaded preparation) when measurements justify it (MASTER-000 §11: custom scheduling only when necessary). Rayon first |

## 4. Responsibilities

- Handle and pool primitives with stale-handle detection.
- A single logging entry point: `init_logging()` installing a `tracing_subscriber` (env filter `ANVIL_LOG`, default `info`, `debug` for `anvil_*` in debug builds).
- Monotonic `Clock`, `FrameCounter` (u64).
- Plain data types crossing layer boundaries for diagnostics (see LAYER README §2.4).

## 5. Non-responsibilities

- Error aggregation across crates: each crate owns its error enum (no god `AnvilError`).
- Resource ownership: foundation provides the container; the RHI and backend own the pools.
- Profiling UI or Tracy integration (layer 5).

## 6. Dependencies

| Allowed | Forbidden |
|---|---|
| `tracing`, `thiserror`, `std` | Any other `anvil-*` crate |
| `tracing-subscriber` (feature-gated) | `ash`, `winit`, `glam` (core stays math-free), `anyhow` (binaries only) |

## 7. Key concepts and data flow

```
 Handle<T> { index:u32, generation:u32 }   (Copy, 8 bytes, PhantomData tag)
        |
   Pool<T>:  slots[index] = { generation, Option<T> }
        insert(v)  -> Handle<T>          (reuses free index, generation already bumped)
        remove(h)  -> Option<T>          (bumps generation; stale h now fails)
        get(h)     -> Option<&T>         (None if index OOB, vacant, or generation mismatch)
```

- Generation 0 is never issued; `Handle::NULL = {0,0}` is always invalid.
- A slot whose generation would wrap past `u32::MAX` is retired (never reused) rather than aliasing.
- The pool is not thread-safe; owners wrap it (`Mutex`) when they need sharing.

## 8. Public interface sketch

```rust
pub struct Handle<T> { index: u32, generation: u32, _tag: PhantomData<fn() -> T> }
impl<T> Handle<T> { pub const NULL: Self; pub fn index(self) -> u32; pub fn generation(self) -> u32; pub fn is_null(self) -> bool; }
// Copy, Clone, Eq, Hash, Debug implemented manually (no bound on T).

pub struct Pool<T> { /* slots, free list */ }
impl<T> Pool<T> {
    pub fn new() -> Self;
    pub fn insert(&mut self, value: T) -> Handle<T>;
    pub fn get(&self, h: Handle<T>) -> Option<&T>;
    pub fn get_mut(&mut self, h: Handle<T>) -> Option<&mut T>;
    pub fn remove(&mut self, h: Handle<T>) -> Option<T>;
    pub fn len(&self) -> usize;
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)>;
}

pub fn init_logging();                        // feature "log-init"; idempotent
pub struct Clock; impl Clock { pub fn new() -> Self; pub fn tick(&mut self) -> Duration /* delta */; }
pub struct FrameCounter(u64);

// Plain-data diagnostics (no behaviour)
pub struct GpuScopeSample { pub name: Box<str>, pub frame: u64, pub gpu_ns: u64 /* begin..end */ }
pub struct MemorySnapshot { pub device_local_used: u64, pub device_local_reserved: u64, pub host_visible_used: u64, pub live_buffers: u32, pub live_textures: u32 }

pub use tracing;                              // so crates share one version
```

Handle tags are zero-sized marker types defined by the owning crate (e.g. `pub struct BufferTag;` in rhi); `pub type BufferHandle = Handle<BufferTag>;`.

## 9. Ownership and lifetime

- `Handle<T>` is a plain copyable key; it owns nothing and does not keep the resource alive.
- `Pool<T>` owns values; dropping the pool drops all values (backend pools must be drained explicitly first because GPU objects need the device).
- No global mutable state except the `tracing` subscriber (set once).

## 10. Threading model

Everything is `Send` (and `Sync` where `T` is). No internal locking. `init_logging` may be called from any thread once.

**Worker pool (M5a, ADR-0013 §3, FR-PLAT-013).** `foundation::init_thread_pool()` configures the global `rayon` pool once (idempotent, returns the thread count) and names its threads `anvil-worker-N`. `ANVIL_THREADS=n` (n >= 1) overrides the default of one thread per logical CPU; an invalid value is ignored with a `warn!`. The pool lives in `foundation`, not `platform`, because the renderer (its main user) does not depend on `platform`; `Renderer::new` calls it, so applications need not. It is for short data-parallel CPU work only (renderer preparation: instance packing, frustum culling, cluster assignment, draw-list sorting). Command recording stays on the calling thread. Parallel code must return exactly what the serial loop would (order-preserving collects, stable sorts) and runs on the calling thread for small inputs (see LAYER-005 M5a).

## 11. Error handling

`Pool` uses `Option` (stale is an expected, checkable condition). foundation defines `CoreError` (thiserror) only for logging init failure. Callers (RHI) convert `None` into their own `StaleHandle` error.

## 12. Diagnostics and observability

- All engine crates log through `tracing` (`error!`, `warn!`, `info!`, `debug!`, `trace!`; spans for frame and init phases). No `println!`.
- `GpuScopeSample` / `MemorySnapshot` are the stable carriers consumed by layer 5.

## 13. Testing strategy

Unit tests: insert/get/remove; stale handle after remove; stale after slot reuse; NULL handle; generation retirement at `u32::MAX` (test via a constructor hook `#[cfg(test)]`); `Handle` is 8 bytes; iteration skips vacant slots; property test (optional, `proptest`) against a `HashMap` model.

## 14. Acceptance criteria

- [ ] foundation builds with no `anvil-*` dependencies.
- [ ] All pool unit tests pass; clippy clean with `-D warnings`.
- [ ] `Handle<T>` is `Copy + Send + Sync` for any `T` (compile test).
- [ ] `init_logging()` called twice does not panic.
- [ ] Public items documented.

## 15. Open questions

1. Math conventions (handedness, clip-space Y, reverse-Z): decide before math / M3; M1 triangle hardcodes vertices in clip space.
2. Should `Pool` offer a `Mutex`-wrapped `SharedPool`, or leave it to owners? (Default: leave to owners until M2.)
3. Is `EngineConfig` needed in M1, or only the RHI `DeviceDesc`? (Default: skip until a second consumer exists, MASTER-000 §22.5.)
4. `GpuScopeSample.name` as `Box<str>` vs interned id; revisit when the profiler is specified.

## 16. Related modules

None for M1. Pool/handle internals are small enough to live in this spec; a module spec is written only if they grow.

## Observability (ADR-0009)

`init_logging()` is configured by environment variables only; all are optional and read once at init.

| Variable | Values | Effect |
|---|---|---|
| `ANVIL_LOG` | `tracing` filter directives, e.g. `info,vulkan::validation=trace` | Overrides the default filter (debug builds: engine crates at `debug`, else `info`). Unparsable: `CoreError::InvalidLogFilter`. |
| `ANVIL_LOG_FORMAT` | `pretty` (default), `json` | `json` emits one JSON object per line with timestamp, level, target, fields, current span and span list. Other value: `CoreError::InvalidLogFormat`. |
| `ANVIL_LOG_FILE` | file path | Create/truncate the file and log there instead of stderr, without ANSI colors. Failure: `CoreError::LogFile { path, reason }`. |

Cargo feature `profile-tracy` (off by default; needs a C++ compiler for `tracy-client-sys`) adds a `tracing-tracy` layer (version 0.12) to the subscriber `init_logging` installs, so every `tracing` span (the renderer's `renderer.*` stage spans, `app_frame`, `frame`) appears in the Tracy profiler (ADR-0013 §2). Examples forward it as `cargo run --release -p pbr-scene --features profile-tracy`. Without the feature nothing changes.

Output always includes the target, so agents can grep on e.g. `vulkan::validation`. After a successful install, one `info!` records the effective `filter`, `format` and `destination`.
