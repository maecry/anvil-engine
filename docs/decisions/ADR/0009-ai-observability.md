# ADR-0009: AI-first observability

- Status: Accepted
- Date: 2026-11-02

## Context
Anvil is developed with AI agents writing, running and debugging the code. An agent cannot watch a window or click through RenderDoc. It understands the engine only through text it can read: logs, error values, diagnostics dumps and test output. MASTER-000 §1.3 "Observable Systems" already requires diagnostics. This ADR makes them machine-first.

## Problem
How do we make every engine operation traceable and every state inspectable by an AI agent, without breaking §22.4 ("never expose backend-specific types unnecessarily")?

## Considered alternatives
1. Human-oriented logs only (pretty text), with ad hoc `println!` debugging.
2. Expose backend internals (raw `vk::*` handles) publicly so tools can poke at them.
3. Structured, machine-readable telemetry plus read-only introspection APIs made of plain data types.

## Decision
Option 3. These rules bind every crate:

1. **Structured logs.** Everything goes through `tracing` with structured fields (`handle = ?h, label, width, height`), never values formatted into the message text. `ANVIL_LOG_FORMAT=json` emits JSON lines (one event per line, with span context); the default is pretty text. `ANVIL_LOG` sets the filter and `ANVIL_LOG_FILE=path` writes to a file instead of stderr.
2. **Stable targets.** Log targets are the crate module paths (e.g. `rhi_vulkan::swapchain`). Vulkan validation messages use the fixed target `vulkan::validation`, with fields `severity`, `message_id`, `message` and `objects` (debug names).
3. **Every public operation is traced.** Create and destroy of every resource logs at `debug` with its handle and label. Per-frame operations (acquire, submit, present) log at `trace` inside a `frame` span carrying `frame = n`. State changes (swapchain recreation, device selection with scores, chosen formats and modes) log at `info`.
4. **Everything has a name.** Every resource descriptor has `label: Option<&str>`. The backend applies labels as Vulkan debug names (`VK_EXT_debug_utils`), so validation messages and RenderDoc show them. Unlabeled resources get a generated name (`texture#3v1`).
5. **Introspection API.** The RHI exposes read-only diagnostics as plain data (`Debug + Clone`, no backend types): `Device::diagnostics() -> DeviceDiagnostics`. It reports device info, the live resource count per kind, the frame index, swapchain state, and validation message counts by severity. Every `RhiError` carries enough context to be acted on without a debugger (the operation name and the handle).
6. **Fail-fast hooks for agent loops.** `ANVIL_VALIDATION_PANIC=1` turns any validation error into a panic with a backtrace. Integration tests assert `diagnostics().validation.errors == 0`, which is the M1 "no validation errors" criterion made automatic.
7. **Headless verifiability.** Examples accept `ANVIL_EXIT_AFTER_FRAMES=n` to render n frames and exit with a nonzero code if validation errors occurred. Diagnostics are printed at exit, so an agent can run an example and judge the result from text alone.

## Advantages
- An agent can run, observe and judge correctness without a human looking at the screen.
- The same data serves humans, CI and later tools (egui panels, Tracy).
- No backend types leak, so §22.4 holds.

## Disadvantages
- More logging calls and fields to maintain. `trace` in the frame loop must stay cheap: it is compiled in but filtered at runtime.
- JSON output adds `tracing-subscriber`'s `json` feature (serde_json) to `foundation`'s optional `log-init`.

## Consequences
- LAYER-001 (logging init), LAYER-003 (`label` fields, `diagnostics()`, error context) and LAYER-004 (debug names, the validation target, counters) gain an "Observability (ADR-0009)" section.
- Code review rejects new public operations that have no tracing or no label.

## Revisit conditions
- Trace overhead becomes measurable in the frame loop. Then add compile-time level caps (`tracing` `max_level_*` features) for release builds.
