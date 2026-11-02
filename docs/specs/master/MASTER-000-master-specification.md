# ANVIL ENGINE
## Master Project Specification & Development Plan

> Source of truth. MASTER-001…004, LAYER-*, MODULE-* refine this document and must not contradict it.
> Changes follow §13.5 (Specification Change Policy).
> Amendment (ADR-0008): crates drop the `anvil-` prefix; `anvil-core` is named `foundation`. Crate names below are the original names.

**Project Name:** Anvil Engine  
**Project Type:** Personal / Open-Source Potential / Production-Oriented Game Engine  
**Primary Language:** Rust  
**Primary Graphics API:** Vulkan  
**Architecture:** Modular, Renderer-First  
**Development Philosophy:** Documentation-First, Specification-Driven Engineering  
**Initial Platform:** Linux Desktop  
**Target Platforms:** Linux, Windows, macOS (subject to backend availability)  
**Project Status:** Planning / Foundation

**Tagline:** Forged for performance. Built for creation.

---

# 1. Project Vision

## 1.1 Overview

Anvil Engine is a modern, modular, high-performance 3D game engine written primarily in Rust.

The project prioritizes real-time rendering technology, GPU resource management, graphics architecture, performance engineering, and developer tooling.

Unlike a general-purpose engine developed around gameplay functionality, Anvil Engine follows a renderer-first development strategy.

The rendering system is the central architectural component. Other engine subsystems will be developed around the requirements of the rendering architecture.

The engine should eventually provide a complete development environment capable of creating, rendering, debugging, profiling, and shipping real-time 3D applications and games.

## 1.2 Vision

Build a production-quality rendering foundation that can evolve into a complete game engine without requiring fundamental architectural rewrites.

Anvil Engine should combine:

- Explicit GPU resource management.
- Modern real-time rendering techniques.
- Modular and extensible architecture.
- Efficient CPU/GPU workload coordination.
- Data-oriented engineering.
- Reliable asset and resource management.
- Advanced debugging and profiling.
- Developer-friendly APIs and tooling.

## 1.3 Core Philosophy

### Rendering First
Rendering technology is the primary engineering priority.

### Performance by Design
Performance, memory management, synchronization, and scalability must be considered during architecture design rather than added as late optimizations.

### Modular Architecture
Subsystems should have clear responsibilities, explicit interfaces, and controlled dependencies.

### Rust-Native Engineering
Use Rust's ownership, type system, concurrency model, and safety guarantees to build reliable systems.

### Explicit GPU Management
GPU resource lifetime, synchronization, memory allocation, and command execution should be designed deliberately.

### Documentation First
Research, specifications, architectural decisions, and acceptance criteria precede substantial implementation.

### Observable Systems
The engine must expose sufficient diagnostics to understand CPU execution, GPU execution, memory usage, rendering operations, and resource lifetime.

---

# 2. Project Objectives

## 2.1 Primary Objectives

1. Develop a modern Vulkan-based rendering backend.
2. Design a reusable Rendering Hardware Interface (RHI).
3. Implement a scalable rendering architecture.
4. Develop a render graph for rendering pass scheduling and resource dependencies.
5. Implement GPU resource and memory management.
6. Support modern physically based rendering.
7. Implement forward and deferred rendering.
8. Support multithreaded rendering preparation.
9. Implement advanced visibility and GPU-driven rendering techniques.
10. Establish a production-quality debugging and profiling environment.

## 2.2 Secondary Objectives

1. Develop a centralized asset management system.
2. Implement scene and material systems.
3. Support common 3D asset formats.
4. Develop an extensible runtime.
5. Build an integrated development editor.
6. Support additional graphics backends where justified.
7. Provide an API for game and application development.

## 2.3 Non-Goals

The initial project will not prioritize:

- Visual scripting.
- Multiplayer networking.
- A custom physics engine.
- A complete animation authoring system.
- Mobile rendering.
- Every possible graphics API.
- A universal gameplay framework.
- Replicating the entire feature set of Unreal Engine or Godot.

These capabilities may be considered after the rendering foundation becomes stable.

---

# 3. Target Applications

Anvil Engine should eventually support:

- Real-time 3D games.
- First-person and third-person games.
- RPGs and action games.
- Procedural environments.
- Large-scale 3D scenes.
- Voxel-based environments.
- Technical visualization.
- Interactive graphics applications.

The renderer must not be tightly coupled to a particular game genre.

---

# 4. Technical Requirements

## 4.1 Rendering Requirements

The renderer should progressively support:

- Hardware-accelerated rendering.
- PBR metallic-roughness materials.
- Directional, point, and spot lights.
- Shadow mapping.
- Image-based lighting.
- HDR rendering.
- Tone mapping.
- Bloom.
- Depth prepass.
- Forward rendering.
- Deferred rendering.
- Clustered lighting.
- Screen-space ambient occlusion.
- Temporal anti-aliasing.
- GPU-driven rendering.
- Frustum and occlusion culling.
- Level of Detail.
- Compute workloads.
- Texture streaming.

## 4.2 GPU Requirements

The GPU abstraction must provide:

- Buffer management.
- Texture and image management.
- Image views.
- Samplers.
- Shader modules.
- Graphics pipelines.
- Compute pipelines.
- Pipeline layouts.
- Descriptor or resource binding management.
- Command recording.
- Queue management.
- Synchronization.
- Memory allocation.
- Resource lifetime management.
- Swapchain management.
- Debug labels and GPU diagnostics.

## 4.3 Performance Requirements

Performance requirements must be measurable.

The project should track:

- CPU frame preparation time.
- GPU frame execution time.
- Frame pacing.
- GPU memory consumption.
- CPU memory consumption.
- Draw-call count.
- Pipeline switching.
- Resource upload time.
- Shader compilation time.
- Asset loading time.
- Render graph execution time.

Do not establish arbitrary performance guarantees before benchmark hardware and workloads are defined.

---

# 5. Architecture Overview

Anvil Engine uses a layered architecture.

## 5.1 Architectural Layers

### Layer 1: Foundation and Platform

Responsibilities:

- Application lifecycle.
- Window management.
- Input.
- Filesystem.
- Logging.
- Timing.
- Thread management.
- Platform integration.

### Layer 2: Rendering Hardware Interface

Responsibilities:

- Graphics device.
- GPU resources.
- Memory allocation.
- Pipeline creation.
- Command recording.
- Queue management.
- Synchronization.
- Backend implementation.

Initial backend: Vulkan.

### Layer 3: Rendering Architecture

Responsibilities:

- Render world.
- Scene extraction.
- Visibility.
- Render graph.
- Rendering passes.
- Materials.
- Lighting.
- Post-processing.
- GPU-driven rendering.

### Layer 4: Engine Runtime

Responsibilities:

- Scene management.
- Asset management.
- Entity and component systems.
- Camera systems.
- Runtime lifecycle.
- Application-facing interfaces.

### Layer 5: Development Tools

Responsibilities:

- Editor.
- Scene viewport.
- Asset browser.
- Material inspector.
- Debug visualization.
- Profiling interfaces.
- Project management.

## 5.2 Dependency Rules

The architecture must follow these rules:

1. Platform code must not depend on the editor.
2. The RHI must not depend on gameplay or scene systems.
3. The Vulkan backend must implement the RHI rather than define the entire engine API.
4. The renderer must not depend on editor implementation details.
5. Scene data must not directly expose Vulkan resource objects.
6. The editor must interact through runtime and editor-facing interfaces.
7. Resource lifetime must be explicitly managed.
8. Circular crate dependencies are prohibited.

---

# 6. Technology Stack

## 6.1 Core

| Component | Technology |
|---|---|
| Programming language | Rust stable |
| Build system | Cargo |
| Workspace | Cargo workspace |
| Graphics API | Vulkan |
| Vulkan bindings | ash |
| Window management | winit |
| Mathematics | glam |
| Serialization | serde |
| Logging | tracing |
| Error handling | thiserror / anyhow |
| Parallel execution | Rayon / custom task system |

## 6.2 Graphics

| Component | Technology |
|---|---|
| GPU API | Vulkan |
| GPU allocation | gpu-allocator or VMA bindings |
| Shader language | GLSL initially |
| Shader compilation | shaderc |
| Shader representation | SPIR-V |
| GPU validation | Vulkan Validation Layers |
| GPU debugging | RenderDoc |
| GPU profiling | Radeon GPU Profiler where supported |

## 6.3 Assets

| Asset | Format |
|---|---|
| Models | glTF 2.0 |
| Textures | PNG, JPEG, EXR |
| GPU textures | KTX2 / Basis Universal |
| Materials | Engine-defined format |
| Scenes | Engine-defined format |
| Configuration | TOML / RON |
| Metadata | Serde-based formats |

## 6.4 Developer Tools

| Purpose | Technology |
|---|---|
| Debug UI | egui |
| CPU profiling | Tracy |
| Frame capture | RenderDoc |
| GPU profiling | Vendor-specific profiling tools |
| Testing | cargo test |
| Linting | clippy |
| Formatting | rustfmt |
| Documentation | Markdown / rustdoc |
| CI | GitHub Actions |

Technology choices must be revisited through ADRs when requirements or dependency constraints change.

---

# 7. Repository Structure

Initial proposed structure:

```text
anvil-engine/
│
├── docs/
│   ├── research/
│   │   ├── graphics-api/
│   │   ├── gpu-architecture/
│   │   ├── rendering-techniques/
│   │   ├── memory-management/
│   │   └── engine-analysis/
│   │
│   ├── architecture/
│   │   ├── overview.md
│   │   ├── dependency-rules.md
│   │   ├── rendering-architecture.md
│   │   ├── resource-management.md
│   │   └── threading-model.md
│   │
│   ├── specs/
│   │   ├── master/
│   │   ├── layers/
│   │   └── modules/
│   │
│   ├── decisions/
│   │   └── ADR/
│   │
│   ├── development/
│   │   ├── roadmap.md
│   │   ├── milestones.md
│   │   ├── coding-standards.md
│   │   └── contribution-guide.md
│   │
│   ├── testing/
│   ├── benchmarks/
│   └── user-guide/
│
├── crates/
│   ├── anvil-core/
│   ├── anvil-platform/
│   ├── anvil-math/
│   ├── anvil-task/
│   ├── anvil-rhi/
│   ├── anvil-rhi-vulkan/
│   ├── anvil-renderer/
│   ├── anvil-render-graph/
│   ├── anvil-shader/
│   ├── anvil-assets/
│   ├── anvil-scene/
│   ├── anvil-mesh/
│   ├── anvil-material/
│   ├── anvil-camera/
│   ├── anvil-debug/
│   ├── anvil-profiler/
│   ├── anvil-runtime/
│   └── anvil-editor/
│
├── examples/
│   ├── triangle/
│   ├── textured-cube/
│   ├── pbr-scene/
│   ├── shadows/
│   └── deferred-rendering/
│
├── tests/
│   ├── unit/
│   ├── integration/
│   ├── rendering/
│   └── performance/
│
├── assets/
├── tools/
├── Cargo.toml
├── rust-toolchain.toml
├── rustfmt.toml
├── clippy.toml
├── README.md
├── LICENSE
└── CONTRIBUTING.md
```

This structure represents the intended modular architecture. Crates should be introduced incrementally as their responsibilities and interfaces become clear.

---

# 8. Major Subsystems

## 8.1 anvil-core

The foundational utilities shared by engine components.

Responsibilities:

- Common types.
- Error definitions.
- Engine configuration.
- Logging integration.
- Time abstractions.
- Identifiers.
- Diagnostics.

## 8.2 anvil-platform

Platform-specific functionality.

Responsibilities:

- Windows.
- Linux.
- Window events.
- Input.
- Surface creation.
- Platform capabilities.

## 8.3 anvil-rhi

The graphics hardware abstraction.

Responsibilities:

- Device interface.
- Buffer interface.
- Texture interface.
- Pipeline interface.
- Command interface.
- Synchronization interface.
- Resource handles.

## 8.4 anvil-rhi-vulkan

The Vulkan backend.

Responsibilities:

- Instance and device creation.
- Physical device selection.
- Queue management.
- Swapchain.
- Command pools.
- GPU allocation.
- Descriptor management.
- Vulkan pipelines.
- Resource barriers.
- Synchronization.

## 8.5 anvil-renderer

The primary rendering system.

Responsibilities:

- Render world.
- Scene extraction.
- Camera extraction.
- Mesh submission.
- Material processing.
- Visibility.
- Rendering pipelines.
- Lighting.
- Post-processing.

## 8.6 anvil-render-graph

The rendering dependency and execution system.

Responsibilities:

- Render passes.
- Resource declarations.
- Dependency analysis.
- Execution ordering.
- Resource transition planning.
- Transient resource allocation.
- Graph diagnostics.

## 8.7 anvil-shader

Responsibilities:

- Shader source management.
- Compilation.
- Reflection.
- Pipeline compatibility.
- Shader cache.
- Hot reload.
- Error reporting.

## 8.8 anvil-assets

Responsibilities:

- Asset database.
- Asset loading.
- Asset handles.
- Asset metadata.
- Import pipeline.
- Dependency tracking.
- Resource caching.
- Streaming.

## 8.9 anvil-scene

Responsibilities:

- Scene representation.
- Transform hierarchy.
- Camera components.
- Mesh components.
- Light components.
- Scene serialization.

## 8.10 anvil-debug

Responsibilities:

- Debug overlays.
- Resource visualization.
- Render graph visualization.
- GPU diagnostics.
- Rendering statistics.
- Debug geometry.

## 8.11 anvil-profiler

Responsibilities:

- CPU profiling.
- GPU timestamps.
- Memory tracking.
- Frame timing.
- Performance statistics.
- Profiling exports.

## 8.12 anvil-editor

Responsibilities:

- Editor application.
- Viewport.
- Scene hierarchy.
- Inspector.
- Asset browser.
- Material editor.
- Debug panels.

---

# 9. Rendering Architecture

## 9.1 Frame Lifecycle

The renderer should progressively implement this lifecycle:

```text
Application Update
       |
       v
Scene Update
       |
       v
Render Extraction
       |
       v
Render World Update
       |
       v
Visibility Processing
       |
       v
Render Graph Construction
       |
       v
Graph Compilation
       |
       v
Command Recording
       |
       v
GPU Submission
       |
       v
Presentation
```

## 9.2 Rendering Pipeline Evolution

### Initial
- Basic rasterization.
- Forward rendering.
- Depth testing.
- Textures.
- Basic lighting.

### Intermediate
- PBR.
- Shadow mapping.
- Image-based lighting.
- Deferred rendering.
- Post-processing.

### Advanced
- Clustered lighting.
- TAA.
- SSAO.
- GPU-driven rendering.
- Indirect drawing.
- GPU culling.

### Experimental
- Ray tracing.
- Global illumination.
- Virtual texturing.
- Advanced visibility systems.
- Advanced temporal rendering.

## 9.3 Render Graph

The render graph must eventually provide:

- Pass registration.
- Resource declarations.
- Dependency tracking.
- Read/write validation.
- Execution scheduling.
- Resource transitions.
- Transient resource management.
- Debug visualization.

It should be designed as a rendering execution system rather than merely a collection of pass callbacks.

---

# 10. GPU Resource Management

Resource management is a foundational system.

## 10.1 Resource Categories

- GPU buffers.
- GPU images.
- Image views.
- Samplers.
- Shader modules.
- Pipelines.
- Descriptor resources.
- Render targets.
- Acceleration structures, if ray tracing is introduced.

## 10.2 Handle Design

Use typed handles with generation tracking.

Example:

```rust
struct TextureHandle {
    index: u32,
    generation: u32,
}
```

Requirements:

- Detect stale handles.
- Avoid accidental resource reuse.
- Centralize ownership.
- Track resource state.
- Support deferred destruction.
- Expose memory diagnostics.

## 10.3 Memory Management

Progressively implement:

- GPU allocator.
- Buffer suballocation.
- Image allocation.
- Staging buffers.
- Upload scheduling.
- Deferred destruction.
- Memory statistics.
- Transient render resources.

The design must account for GPU execution still referencing resources when the CPU requests their destruction.

---

# 11. Threading and Concurrency

The engine should separate application execution, render preparation, and GPU execution.

Potential responsibilities:

- Main thread: application lifecycle and window events.
- Worker threads: asset processing and parallel scene preparation.
- Render preparation: visibility, sorting, batching, graph construction.
- GPU queues: graphics, compute, and transfer execution.

Concurrency requirements:

- Avoid unnecessary locks in hot paths.
- Use explicit synchronization.
- Avoid sharing mutable scene state across rendering workers.
- Prefer immutable extracted render data.
- Measure task scheduling overhead.
- Introduce custom scheduling only when necessary.

---

# 12. Asset Pipeline

The asset system must eventually support:

1. Asset discovery.
2. Asset identification.
3. Importing.
4. Validation.
5. Metadata generation.
6. Dependency tracking.
7. Runtime loading.
8. GPU upload.
9. Caching.
10. Hot reload.
11. Streaming.
12. Versioning.

Initial supported formats:

- glTF 2.0.
- PNG/JPEG.
- EXR.
- KTX2.
- Engine-defined scene and material formats.

The import pipeline must remain separate from runtime GPU resource management.

---

# 13. Development Methodology

Anvil Engine follows documentation-first development.

The mandatory development lifecycle is:

```text
Research
   |
   v
Architecture Analysis
   |
   v
Architecture Decision
   |
   v
Layer Specification
   |
   v
Module Specification
   |
   v
Implementation Planning
   |
   v
Implementation
   |
   v
Testing and Validation
   |
   v
Benchmarking
   |
   v
Documentation Update
```

## 13.1 Documentation Hierarchy

### MASTER

Defines the entire project.

Contains:

- Vision.
- Objectives.
- Requirements.
- Scope.
- Architecture overview.
- Technology strategy.
- Roadmap.
- Quality requirements.

### LAYER

Defines major architectural subsystems.

Examples:

- Platform Layer.
- RHI Layer.
- Rendering Layer.
- Asset Layer.
- Runtime Layer.
- Tooling Layer.

### MODULE

Defines individual modules.

Examples:

- VulkanDevice.
- BufferAllocator.
- TextureRegistry.
- RenderGraphCompiler.
- ShaderCompiler.
- PipelineCache.

## 13.2 Research Documentation

Research documents record:

- Technical concepts.
- Existing engine analysis.
- API behavior.
- Architecture comparisons.
- Performance considerations.
- Experimental findings.
- References.

Research is not an implementation specification.

## 13.3 Architecture Decision Records

All important architectural decisions should have an ADR.

Each ADR must document:

- Context.
- Problem.
- Considered alternatives.
- Decision.
- Advantages.
- Disadvantages.
- Consequences.
- Revisit conditions.

## 13.4 Specification Requirements

Every module specification must define:

- Purpose.
- Responsibilities.
- Non-responsibilities.
- Dependencies.
- Public API.
- Data structures.
- Ownership and lifetime.
- Execution flow.
- Threading model.
- Error handling.
- Performance requirements.
- Testing requirements.
- Acceptance criteria.

## 13.5 Specification Change Policy

When implementation reveals architectural problems:

1. Stop and identify the discrepancy.
2. Document the problem.
3. Research alternatives.
4. Propose an architectural change.
5. Update affected specifications.
6. Record an ADR if necessary.
7. Resume implementation.

The codebase and specifications must remain consistent.

---

# 14. Development Roadmap

## M0 — Foundation

Deliverables:

- Master specification.
- Initial architecture.
- Repository structure.
- Initial ADRs.
- Cargo workspace.
- CI configuration.
- Development standards.

Completion criteria:

- Clean workspace builds.
- Documentation structure is established.
- Dependency rules are documented.

## M1 — Vulkan Foundation

Deliverables:

- Vulkan initialization.
- Physical device selection.
- Logical device.
- Window surface.
- Swapchain.
- Command buffers.
- Synchronization.
- Validation.
- Triangle rendering.

Completion criteria:

- Stable application lifecycle.
- Window resizing.
- Clean shutdown.
- No known Vulkan validation errors in the baseline test.

## M2 — RHI and Resources

Deliverables:

- GPU buffers.
- Textures.
- Samplers.
- Pipelines.
- Binding management.
- Resource handles.
- Memory allocation.
- Deferred destruction.

Completion criteria:

- Reusable GPU resource infrastructure.

## M3 — Core Renderer

Deliverables:

- Mesh rendering.
- Cameras.
- Transforms.
- Textures.
- Materials.
- Basic lighting.
- PBR.
- Shadow mapping.
- Environment lighting.

Completion criteria:

- Complete PBR scene rendering.

## M4 — Render Graph and Advanced Rendering

Deliverables:

- Render graph.
- Dependency analysis.
- Resource transitions.
- Deferred rendering.
- Post-processing.
- SSAO.
- TAA.
- Clustered lighting.

Completion criteria:

- Configurable rendering pipeline with validated dependencies.

## M5 — Performance Engineering

Deliverables:

- Multithreaded preparation.
- GPU-driven rendering.
- Indirect drawing.
- GPU culling.
- LOD.
- Streaming.
- Pipeline cache.
- Profiling.

Completion criteria:

- Reproducible benchmarks and measurable optimizations.

## M6 — Engine Runtime and Tooling

Deliverables:

- Asset database.
- Scene system.
- Runtime.
- Editor viewport.
- Inspector.
- Asset browser.
- Material tools.
- Debug visualization.

Completion criteria:

- A complete sample project can be created, saved, reopened, and executed.

## M7 — Production Hardening

Deliverables:

- Regression testing.
- Resource leak testing.
- Device-loss handling.
- Shader compatibility tests.
- Asset corruption handling.
- Stable public interfaces.
- Packaging.
- User documentation.

Completion criteria:

- Reproducible release candidate with documented limitations.

---

# 15. Testing Strategy

Testing must exist throughout development.

## 15.1 Unit Tests

Examples:

- Resource handle generation.
- Resource registry.
- Render graph dependency resolution.
- Shader metadata.
- Asset identifiers.
- Transform calculations.

## 15.2 Integration Tests

Examples:

- Vulkan device initialization.
- Buffer creation.
- Texture upload.
- Pipeline creation.
- Swapchain recreation.
- Resource destruction.

## 15.3 Rendering Tests

Create deterministic test scenes.

Compare output images using appropriate tolerances.

Test:

- Geometry.
- Materials.
- Lighting.
- Shadows.
- Transparency.
- Post-processing.
- Rendering edge cases.

## 15.4 Performance Tests

Track:

- CPU frame time.
- GPU frame time.
- GPU memory.
- Draw calls.
- Upload performance.
- Render graph overhead.
- Asset loading performance.

## 15.5 Stress Tests

Test:

- Large resource counts.
- Repeated allocation and destruction.
- Window resizing.
- Shader reload.
- Asset streaming.
- Long-running sessions.
- Device-loss recovery where supported.

---

# 16. Quality Standards

## Code Quality

- Follow Rust conventions.
- Use rustfmt.
- Run clippy.
- Document public APIs.
- Avoid unnecessary unsafe code.
- Isolate unsafe operations.
- Prefer explicit ownership.
- Avoid premature abstraction.

## Architecture Quality

- Enforce dependency boundaries.
- Avoid circular dependencies.
- Keep modules independently testable.
- Record architectural decisions.
- Maintain specification consistency.

## Performance Quality

- Benchmark before optimizing.
- Track performance regressions.
- Avoid unnecessary CPU/GPU synchronization.
- Monitor memory usage.
- Profile real workloads.

## Reliability Quality

- Handle errors explicitly.
- Validate GPU operations.
- Avoid silent failures.
- Track resource lifetime.
- Test shutdown and recovery paths.

---

# 17. CI/CD and Release Engineering

CI should progressively include:

1. Formatting.
2. Clippy.
3. Unit tests.
4. Workspace builds.
5. Shader compilation.
6. Integration tests.
7. GPU tests on dedicated runners.
8. Rendering regression tests.
9. Performance regression tests.

Release artifacts should include:

- Runtime binaries.
- Required assets.
- Shader artifacts or compiler support.
- Runtime configuration.
- Version information.
- User documentation.

Maintain separate development, experimental, and stable release channels when the project becomes mature enough.

---

# 18. Initial Research Curriculum

The project owner should study these subjects in parallel with implementation.

## Graphics Fundamentals

- GPU architecture.
- Rasterization.
- Coordinate transformations.
- Linear algebra.
- Depth buffering.
- Texture sampling.
- Lighting.
- BRDF.
- PBR.

## Vulkan

- Instance and device.
- Queues.
- Command buffers.
- Synchronization.
- Memory allocation.
- Resource barriers.
- Descriptor sets.
- Graphics pipelines.
- Compute pipelines.
- Swapchains.

## Rendering

- Forward rendering.
- Deferred rendering.
- Shadow mapping.
- Image-based lighting.
- HDR.
- Post-processing.
- Temporal rendering.
- GPU-driven rendering.

## Rust

- Ownership.
- Lifetimes.
- Traits.
- Generics.
- Concurrency.
- Atomics.
- Unsafe Rust.
- Memory allocation.
- Data-oriented design.

## Engine Architecture

- Resource management.
- Render graphs.
- ECS.
- Asset pipelines.
- Scene extraction.
- Task scheduling.
- Profiling.
- Serialization.

---

# 19. Initial Reference Projects

Study these projects as architectural references:

| Project | Research Focus |
|---|---|
| Godot | Scene, resource, editor and renderer architecture |
| Bevy | Rust-native rendering and ECS |
| Fyrox | Rust engine and editor |
| Unreal Engine | Large-scale rendering architecture |
| Filament | PBR rendering |
| The Forge | Explicit graphics architecture |
| Diligent Engine | Graphics API abstraction |

Do not reproduce complete implementations. Extract concepts, understand trade-offs, and design solutions appropriate for Anvil Engine.

---

# 20. First 30-Day Execution Plan

## Week 1 — Research and Design

- Establish master specification.
- Research Vulkan architecture.
- Research GPU synchronization.
- Research resource lifetime.
- Define initial crate boundaries.
- Write ADRs.

## Week 2 — Workspace and Platform

- Initialize Cargo workspace.
- Create core crate.
- Create platform crate.
- Integrate winit.
- Implement application lifecycle.
- Configure logging and error handling.

## Week 3 — Vulkan Initialization

- Create Vulkan instance.
- Enumerate physical devices.
- Select queues.
- Create logical device.
- Create surface.
- Create swapchain.
- Enable validation layers.

## Week 4 — First Rendering

- Command pools.
- Command buffers.
- Synchronization.
- Graphics pipeline.
- Shader compilation.
- Triangle rendering.
- Initial integration tests.

## Month 1 Acceptance Criteria

- Anvil Engine builds successfully.
- Application starts on Linux.
- Vulkan device initializes.
- A window is created.
- Swapchain works.
- Triangle is rendered.
- Validation is enabled.
- Logs and errors are accessible.
- Initial documentation matches implementation.

---

# 21. Project Success Criteria

## Rendering Engine

- Stable Vulkan backend.
- PBR rendering.
- Shadows.
- Post-processing.
- Forward and deferred pipelines.
- Render graph.
- GPU resource management.
- Profiling.

## Complete Engine

- Runtime.
- Asset pipeline.
- Scene system.
- Editor.
- Debugging tools.
- Public development interfaces.

## Production Quality

- Automated testing.
- Regression testing.
- Performance benchmarks.
- Resource lifetime correctness.
- Error handling.
- Stable release process.
- Complete technical documentation.

---

# 22. Final Engineering Principles

1. Never implement a major subsystem without understanding its responsibilities.
2. Never let implementation silently redefine specifications.
3. Never optimize without measurements.
4. Never expose backend-specific types unnecessarily.
5. Never create abstractions without a real requirement.
6. Never compromise resource lifetime correctness for convenience.
7. Always consider GPU synchronization and memory ownership.
8. Always document architectural trade-offs.
9. Always test rendering output.
10. Build working vertical slices rather than disconnected infrastructure.

---

# 23. Project Direction

Anvil Engine begins as a Vulkan-based rendering engine.

Its first objective is not to become a complete game engine.

Its first objective is to establish a technically sound rendering foundation capable of supporting increasingly complex real-time 3D workloads.

Once the rendering architecture becomes stable, the engine can grow into a complete development environment.

**ANVIL ENGINE**

*Forged for performance. Built for creation.*
