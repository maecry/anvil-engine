//! M2 resource integration tests (MODULE-005/006/007), gated by `ANVIL_GPU_TESTS=1`
//! (validation On, sync validation on). Each test ends with zero validation errors and warnings.

mod common;

use common::*;
use rhi::{
    AddressMode, BufferDesc, BufferUsage, CommandEncoder, Device, Filter, Format, IndexFormat,
    Memory, RhiError, SamplerDesc, TextureDesc, TextureState, TextureUsage,
};

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn buf(dev: &impl Device, size: u64, usage: BufferUsage, memory: Memory) -> rhi::BufferHandle {
    dev.create_buffer(&BufferDesc {
        label: Some("test.buffer"),
        size,
        usage,
        memory,
    })
    .unwrap()
}

fn sampler(dev: &impl Device) -> rhi::SamplerHandle {
    dev.create_sampler(&SamplerDesc {
        label: Some("test.sampler"),
        mag_filter: Filter::Nearest,
        min_filter: Filter::Nearest,
        address_u: AddressMode::ClampToEdge,
        address_v: AddressMode::ClampToEdge,
        address_w: AddressMode::ClampToEdge,
    })
    .unwrap()
}

// ------------------------------------------------------------------ buffers (MODULE-005)

#[test]
fn buffer_write_read_roundtrip_exact() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let data = pattern(4096, 7);
    let up = buf(&dev, 4096, BufferUsage::TRANSFER_SRC, Memory::Upload);
    let rb = buf(&dev, 4096, BufferUsage::TRANSFER_DST, Memory::Readback);
    dev.write_buffer(up, 0, &data).unwrap();
    frame(&mut dev, |c| c.copy_buffer_to_buffer(up, 0, rb, 0, 4096));
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; 4096];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, data);
    dev.destroy_buffer(up);
    dev.destroy_buffer(rb);
    retire_all(&mut dev);
    assert_eq!(dev.diagnostics().live.buffers, 0);
    assert_clean(&inst);
}

#[test]
fn buffer_upload_gpu_roundtrip() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let gpu = buf(
        &dev,
        1024,
        BufferUsage::TRANSFER_DST | BufferUsage::TRANSFER_SRC,
        Memory::GpuOnly,
    );
    let rb = buf(&dev, 1024, BufferUsage::TRANSFER_DST, Memory::Readback);
    // Before the frame.
    let a = pattern(1024, 1);
    dev.upload_buffer(gpu, 0, &a).unwrap();
    frame(&mut dev, |c| c.copy_buffer_to_buffer(gpu, 0, rb, 0, 1024));
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; 1024];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, a);
    // During the frame (uploads are queued and run ahead of the recorded commands).
    let b = pattern(1024, 99);
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    dev.upload_buffer(gpu, 0, &b).unwrap();
    c.copy_buffer_to_buffer(gpu, 0, rb, 0, 1024);
    let fin = c.finish().unwrap();
    dev.submit_unwindowed(fin).unwrap();
    dev.wait_idle().unwrap();
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, b);
    dev.destroy_buffer(gpu);
    dev.destroy_buffer(rb);
    assert_clean(&inst);
}

#[test]
fn buffer_upload_offsets() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let gpu = buf(
        &dev,
        512,
        BufferUsage::TRANSFER_DST | BufferUsage::TRANSFER_SRC,
        Memory::GpuOnly,
    );
    let rb = buf(&dev, 512, BufferUsage::TRANSFER_DST, Memory::Readback);
    let (a, b) = (pattern(100, 3), pattern(200, 5));
    dev.upload_buffer(gpu, 0, &a).unwrap();
    dev.upload_buffer(gpu, 256, &b).unwrap();
    dev.upload_buffer(gpu, 300, &[]).unwrap(); // empty is a no-op
    frame(&mut dev, |c| c.copy_buffer_to_buffer(gpu, 0, rb, 0, 512));
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; 512];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out[..100], a[..]);
    assert_eq!(out[256..456], b[..]);
    dev.destroy_buffer(gpu);
    dev.destroy_buffer(rb);
    assert_clean(&inst);
}

#[test]
fn buffer_address_nonzero_stable() {
    let Some((inst, dev)) = setup() else { return };
    let a = buf(&dev, 64, BufferUsage::STORAGE, Memory::GpuOnly);
    let b = buf(&dev, 64, BufferUsage::STORAGE, Memory::GpuOnly);
    let (aa, ab) = (
        dev.buffer_address(a).unwrap(),
        dev.buffer_address(b).unwrap(),
    );
    assert_ne!(aa, 0);
    assert_eq!(aa, dev.buffer_address(a).unwrap());
    assert_ne!(aa, ab);
    dev.destroy_buffer(a);
    dev.destroy_buffer(b);
    assert_clean(&inst);
}

#[test]
fn buffer_stale_handle() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let usage = BufferUsage::TRANSFER_DST | BufferUsage::TRANSFER_SRC;
    let b = buf(&dev, 64, usage, Memory::Upload);
    dev.destroy_buffer(b);
    let stale = |r: Result<(), RhiError>| {
        assert!(
            matches!(r, Err(RhiError::StaleHandle { kind: "buffer", .. })),
            "{r:?}"
        );
    };
    assert!(matches!(
        dev.buffer_address(b),
        Err(RhiError::StaleHandle { kind: "buffer", .. })
    ));
    stale(dev.write_buffer(b, 0, &[1]));
    stale(dev.upload_buffer(b, 0, &[1]));
    stale(dev.read_buffer(b, 0, &mut [0]));
    dev.destroy_buffer(b); // second destroy: warn + no-op
    retire_all(&mut dev);
    // The slot is reused after retirement; the old generation stays rejected.
    let n = buf(&dev, 64, usage, Memory::Upload);
    assert_eq!(n.index(), b.index());
    assert_ne!(n.generation(), b.generation());
    stale(dev.write_buffer(b, 0, &[1]));
    dev.write_buffer(n, 0, &[1]).unwrap();
    dev.destroy_buffer(n);
    // A stale handle in the encoder surfaces at finish.
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    c.copy_buffer_to_buffer(b, 0, n, 0, 1);
    assert!(matches!(
        c.finish(),
        Err(RhiError::StaleHandle { kind: "buffer", .. })
    ));
    // The warning counts are about validation messages only; stale handles never reached Vulkan.
    assert_clean(&inst);
}

#[test]
fn buffer_destroy_deferred() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let usage = BufferUsage::TRANSFER_DST | BufferUsage::TRANSFER_SRC;
    let a = buf(&dev, 256, usage, Memory::GpuOnly);
    let b = buf(&dev, 256, usage, Memory::GpuOnly);
    let tag = frame(&mut dev, |c| c.copy_buffer_to_buffer(a, 0, b, 0, 256));
    dev.destroy_buffer(a);
    dev.destroy_buffer(b);
    assert_eq!(dev.diagnostics().pending_destroys, 2);
    assert_eq!(dev.diagnostics().live.buffers, 0);
    dev.wait_idle().unwrap();
    // The tag is the NEXT submit's value, so completing the referencing submit is not enough.
    dev.begin_frame().unwrap();
    assert_eq!(
        dev.diagnostics().pending_destroys,
        2,
        "tag {tag} + 1 not reached"
    );
    frame(&mut dev, |_| {});
    dev.wait_idle().unwrap();
    dev.begin_frame().unwrap();
    let d = dev.diagnostics();
    assert!(d.gpu_completed_frame > tag);
    assert_eq!(d.pending_destroys, 0);
    assert_clean(&inst);
}

#[test]
fn buffer_invalid_args() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let invalid = |r: Result<(), RhiError>| {
        assert!(matches!(r, Err(RhiError::InvalidDescriptor(_))), "{r:?}");
    };
    let zero = dev.create_buffer(&BufferDesc {
        label: None,
        size: 0,
        usage: BufferUsage::STORAGE,
        memory: Memory::GpuOnly,
    });
    invalid(zero.map(|_| ()));
    let empty = dev.create_buffer(&BufferDesc {
        label: None,
        size: 8,
        usage: BufferUsage::NONE,
        memory: Memory::GpuOnly,
    });
    invalid(empty.map(|_| ()));
    let gpu = buf(&dev, 16, BufferUsage::INDEX, Memory::GpuOnly);
    invalid(dev.write_buffer(gpu, 0, &[1]));
    invalid(dev.read_buffer(gpu, 0, &mut [0]));
    invalid(dev.upload_buffer(gpu, 0, &[1])); // no TRANSFER_DST
    let up = buf(&dev, 16, BufferUsage::TRANSFER_DST, Memory::Upload);
    invalid(dev.write_buffer(up, 8, &[0; 9]));
    invalid(dev.write_buffer(up, u64::MAX, &[0; 2]));
    dev.write_buffer(up, 8, &[0; 8]).unwrap();
    dev.write_buffer(up, 16, &[]).unwrap();
    invalid(dev.read_buffer(up, 0, &mut [0])); // not Readback
    // Index misalignment is recorded in the encoder and surfaces at finish.
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    c.begin_rendering(&rhi::RenderingDesc {
        color: &[],
        depth: None,
        width: 1,
        height: 1,
    });
    invalid(c.finish().map(|_| ()));
    let idx = buf(&dev, 16, BufferUsage::INDEX, Memory::GpuOnly);
    let tgt = offscreen_target(&dev, 4, 4);
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    c.texture_barrier(tgt, TextureState::Undefined, TextureState::ColorAttachment);
    c.begin_rendering(&rhi::RenderingDesc {
        color: &[rhi::ColorAttachment {
            texture: tgt,
            load: rhi::LoadOp::Clear,
            store: rhi::StoreOp::Store,
            clear: [0.0; 4],
        }],
        depth: None,
        width: 4,
        height: 4,
    });
    c.bind_index_buffer(idx, 2, IndexFormat::U32);
    c.end_rendering();
    invalid(c.finish().map(|_| ()));
    for h in [gpu, up, idx] {
        dev.destroy_buffer(h);
    }
    dev.destroy_texture(tgt);
    assert_clean(&inst);
}

#[test]
fn buffer_stress_create_destroy_100k() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    for i in 0..100_000u32 {
        let b = buf(&dev, 256, BufferUsage::TRANSFER_DST, Memory::GpuOnly);
        dev.destroy_buffer(b);
        if i % 500 == 499 {
            frame(&mut dev, |_| {});
        }
    }
    dev.wait_idle().unwrap();
    retire_all(&mut dev);
    let d = dev.diagnostics();
    assert_eq!(d.live.buffers, 0);
    eprintln!(
        "stress: allocated {} reserved {}",
        d.memory.allocated_bytes, d.memory.reserved_bytes
    );
    assert_eq!(d.memory.allocated_bytes, 0);
    assert_clean(&inst);
}

#[test]
fn buffer_leak_report_at_shutdown() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let _leaked = buf(&dev, 64, BufferUsage::STORAGE, Memory::GpuOnly);
    let cap = Capture::default();
    tracing::subscriber::with_default(cap.clone(), || dev.shutdown().unwrap());
    assert_eq!(cap.warnings_containing("leaked buffer"), 1);
    assert_clean(&inst);
}

#[test]
fn buffer_memory_diagnostics() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let base = dev.diagnostics().memory.allocated_bytes;
    let b = buf(&dev, 1 << 20, BufferUsage::STORAGE, Memory::GpuOnly);
    let m = dev.diagnostics().memory;
    assert!(m.allocated_bytes >= base + (1 << 20));
    assert!(m.reserved_bytes >= m.allocated_bytes);
    assert_eq!(dev.diagnostics().live.buffers, 1);
    dev.destroy_buffer(b);
    assert!(
        dev.diagnostics().memory.allocated_bytes >= base + (1 << 20),
        "freed only at retirement"
    );
    retire_all(&mut dev);
    assert_eq!(dev.diagnostics().memory.allocated_bytes, base);
    assert_clean(&inst);
}

#[test]
fn abandoned_frame_keeps_uploads() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let gpu = buf(
        &dev,
        64,
        BufferUsage::TRANSFER_DST | BufferUsage::TRANSFER_SRC,
        Memory::GpuOnly,
    );
    let rb = buf(&dev, 64, BufferUsage::TRANSFER_DST, Memory::Readback);
    let data = pattern(64, 11);
    dev.upload_buffer(gpu, 0, &data).unwrap();
    dev.begin_frame().unwrap();
    dev.begin_frame().unwrap(); // first frame abandoned without submit
    frame(&mut dev, |c| c.copy_buffer_to_buffer(gpu, 0, rb, 0, 64));
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; 64];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, data);
    dev.destroy_buffer(gpu);
    dev.destroy_buffer(rb);
    assert_clean(&inst);
}

// ------------------------------------------------------------------ textures (MODULE-006)

fn tex_desc(w: u32, h: u32, format: Format, usage: TextureUsage) -> TextureDesc<'static> {
    TextureDesc {
        label: Some("test.texture"),
        width: w,
        height: h,
        format,
        usage,
    }
}

fn readable(w: u32, h: u32) -> TextureDesc<'static> {
    tex_desc(
        w,
        h,
        Format::Rgba8Unorm,
        TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST | TextureUsage::TRANSFER_SRC,
    )
}

#[test]
fn texture_upload_readback_exact() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let t = dev.create_texture(&readable(16, 16)).unwrap();
    let data = pattern(16 * 16 * 4, 21);
    dev.upload_texture(t, &data).unwrap();
    let rb = readback_buffer(&dev, data.len() as u64);
    frame(&mut dev, |c| {
        c.texture_barrier(t, TextureState::ShaderRead, TextureState::TransferSrc);
        c.copy_texture_to_buffer(t, rb, 0);
    });
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; data.len()];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, data);
    dev.destroy_buffer(rb);
    dev.destroy_texture(t);
    assert_clean(&inst);
}

#[test]
fn texture_upload_twice() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let t = dev.create_texture(&readable(8, 8)).unwrap();
    let (a, b) = (pattern(256, 1), pattern(256, 200));
    dev.upload_texture(t, &a).unwrap();
    frame(&mut dev, |_| {});
    dev.upload_texture(t, &b).unwrap(); // from ShaderRead
    let rb = readback_buffer(&dev, 256);
    frame(&mut dev, |c| {
        c.texture_barrier(t, TextureState::ShaderRead, TextureState::TransferSrc);
        c.copy_texture_to_buffer(t, rb, 0);
    });
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; 256];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    assert_eq!(out, b);
    // Uploading while tracked as TransferSrc is rejected.
    assert!(matches!(
        dev.upload_texture(t, &a),
        Err(RhiError::InvalidDescriptor(_))
    ));
    dev.destroy_buffer(rb);
    dev.destroy_texture(t);
    assert_clean(&inst);
}

#[test]
fn texture_upload_invalid_args() {
    let Some((inst, dev)) = setup() else { return };
    let t = dev.create_texture(&readable(4, 4)).unwrap();
    let invalid = |r: Result<(), RhiError>| {
        assert!(matches!(r, Err(RhiError::InvalidDescriptor(_))), "{r:?}");
    };
    invalid(dev.upload_texture(t, &[0; 63]));
    invalid(dev.upload_texture(t, &[0; 65]));
    let nodst = dev
        .create_texture(&tex_desc(4, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED))
        .unwrap();
    invalid(dev.upload_texture(nodst, &[0; 64]));
    let nosamp = dev
        .create_texture(&tex_desc(
            4,
            4,
            Format::Rgba8Unorm,
            TextureUsage::TRANSFER_DST,
        ))
        .unwrap();
    invalid(dev.texture_index(nosamp).map(|_| ()));
    assert_eq!(dev.texture_index(t).unwrap(), t.index());
    for h in [t, nodst, nosamp] {
        dev.destroy_texture(h);
    }
    assert_clean(&inst);
}

fn checker(w: u32, h: u32) -> (Vec<u8>, impl Fn(u32, u32) -> [u8; 4]) {
    let cell = |x: u32, y: u32| -> [u8; 4] {
        if (x + y).is_multiple_of(2) {
            [220, 40, 40, 255]
        } else {
            [40, 40, 220, 255]
        }
    };
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&cell(x, y));
        }
    }
    (data, cell)
}

fn push_sample(c: &mut rhi_vulkan::VulkanCommands, q: &Quad, tex: u32, smp: u32) {
    let mut pc = q.vertex_address.to_le_bytes().to_vec();
    pc.extend_from_slice(&tex.to_le_bytes());
    pc.extend_from_slice(&smp.to_le_bytes());
    c.push_constants(0, &pc);
}

#[test]
fn texture_sample_in_draw() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let tex = dev.create_texture(&readable(2, 2)).unwrap();
    let solid: Vec<u8> = [10u8, 200, 30, 255].repeat(4);
    dev.upload_texture(tex, &solid).unwrap();
    let smp = sampler(&dev);
    let q = Quad::new(&dev);
    let pipe = Pipe::new(
        &dev,
        SAMPLE_VERT,
        SAMPLE_FRAG,
        Format::Rgba8Unorm,
        None,
        "sample",
    );
    let target = offscreen_target(&dev, 8, 8);
    let (ti, si) = (
        dev.texture_index(tex).unwrap(),
        dev.sampler_index(smp).unwrap(),
    );
    let out = render_offscreen(&mut dev, target, None, (8, 8), |c| {
        c.bind_pipeline(pipe.pipeline);
        push_sample(c, &q, ti, si);
        q.draw(c);
    });
    for (x, y) in [(0, 0), (7, 7), (3, 4), (0, 7)] {
        assert_eq!(pixel(&out, 8, x, y), [10, 200, 30, 255], "pixel {x},{y}");
    }
    pipe.destroy(&dev);
    q.destroy(&dev);
    dev.destroy_texture(tex);
    dev.destroy_texture(target);
    dev.destroy_sampler(smp);
    assert_clean(&inst);
}

#[test]
fn texture_checkerboard_quad_pixels() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let tex = dev.create_texture(&readable(4, 4)).unwrap();
    let (data, cell) = checker(4, 4);
    dev.upload_texture(tex, &data).unwrap();
    let smp = sampler(&dev);
    let q = Quad::new(&dev);
    let pipe = Pipe::new(
        &dev,
        SAMPLE_VERT,
        SAMPLE_FRAG,
        Format::Rgba8Unorm,
        None,
        "sample",
    );
    let target = offscreen_target(&dev, 16, 16);
    let (ti, si) = (
        dev.texture_index(tex).unwrap(),
        dev.sampler_index(smp).unwrap(),
    );
    let out = render_offscreen(&mut dev, target, None, (16, 16), |c| {
        c.bind_pipeline(pipe.pipeline);
        push_sample(c, &q, ti, si);
        q.draw(c);
    });
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(pixel(&out, 16, x, y), cell(x / 4, y / 4), "pixel {x},{y}");
        }
    }
    pipe.destroy(&dev);
    q.destroy(&dev);
    dev.destroy_texture(tex);
    dev.destroy_texture(target);
    dev.destroy_sampler(smp);
    assert_clean(&inst);
}

fn flat_push(q: &Quad, color: [f32; 4], z: f32) -> Vec<u8> {
    let mut pc = q.vertex_address.to_le_bytes().to_vec();
    pc.extend_from_slice(&[0u8; 8]); // std430: vec4 aligns to 16
    pc.extend_from_slice(&bytes_of_f32(&color));
    pc.extend_from_slice(&z.to_le_bytes());
    pc
}

#[test]
fn push_constants_reach_shader() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let q = Quad::new(&dev);
    let pipe = Pipe::new(&dev, FLAT_VERT, FLAT_FRAG, Format::Rgba8Unorm, None, "flat");
    let target = offscreen_target(&dev, 8, 8);
    let out = render_offscreen(&mut dev, target, None, (8, 8), |c| {
        c.push_constants(0, &flat_push(&q, [0.2, 0.4, 0.8, 1.0], 0.5)); // before any pipeline
        c.bind_pipeline(pipe.pipeline);
        q.draw(c);
    });
    assert_eq!(pixel(&out, 8, 4, 4), [51, 102, 204, 255]);
    pipe.destroy(&dev);
    q.destroy(&dev);
    dev.destroy_texture(target);
    assert_clean(&inst);
}

#[test]
fn push_constants_misuse_surfaces_at_finish() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    for (offset, len) in [(2u32, 4usize), (0, 3), (4, 128), (128, 4), (0, 132)] {
        let t = dev.begin_frame().unwrap();
        let mut c = dev.begin_commands(&t).unwrap();
        c.push_constants(offset, &vec![0u8; len]);
        assert!(
            matches!(c.finish(), Err(RhiError::InvalidDescriptor(_))),
            "offset {offset} len {len}"
        );
    }
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    c.push_constants(0, &[0u8; 128]);
    c.push_constants(124, &[0u8; 4]);
    c.finish().unwrap();
    assert_clean(&inst);
}

#[test]
fn pipelines_share_one_layout() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let q = Quad::new(&dev);
    let p1 = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        None,
        "flat1",
    );
    let p2 = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        None,
        "flat2",
    );
    let target = offscreen_target(&dev, 8, 8);
    let out = render_offscreen(&mut dev, target, None, (8, 8), |c| {
        c.bind_pipeline(p1.pipeline);
        c.push_constants(0, &flat_push(&q, [1.0, 0.0, 0.0, 1.0], 0.5));
        q.draw(c);
        c.bind_pipeline(p2.pipeline);
        c.push_constants(0, &flat_push(&q, [0.0, 1.0, 0.0, 1.0], 0.5));
        q.draw(c);
    });
    assert_eq!(pixel(&out, 8, 2, 2), [0, 255, 0, 255]);
    p1.destroy(&dev); // destroying one does not invalidate the other
    let target2 = offscreen_target(&dev, 8, 8);
    let out = render_offscreen(&mut dev, target2, None, (8, 8), |c| {
        c.bind_pipeline(p2.pipeline);
        c.push_constants(0, &flat_push(&q, [0.0, 0.0, 1.0, 1.0], 0.5));
        q.draw(c);
    });
    assert_eq!(pixel(&out, 8, 2, 2), [0, 0, 255, 255]);
    p2.destroy(&dev);
    q.destroy(&dev);
    dev.destroy_texture(target);
    dev.destroy_texture(target2);
    assert_clean(&inst);
}

#[test]
fn depth_pipeline_draw() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let q = Quad::new(&dev);
    let pipe = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        Some(Format::D32Sfloat),
        "flat.depth",
    );
    let target = offscreen_target(&dev, 8, 8);
    let depth = dev
        .create_texture(&tex_desc(
            8,
            8,
            Format::D32Sfloat,
            TextureUsage::DEPTH_ATTACHMENT,
        ))
        .unwrap();
    let out = render_offscreen(&mut dev, target, Some(depth), (8, 8), |c| {
        c.bind_pipeline(pipe.pipeline);
        // Near (green) first, far (red) second: without a working depth test red would win.
        c.push_constants(0, &flat_push(&q, [0.0, 1.0, 0.0, 1.0], 0.3));
        q.draw(c);
        c.push_constants(0, &flat_push(&q, [1.0, 0.0, 0.0, 1.0], 0.7));
        q.draw(c);
    });
    assert_eq!(pixel(&out, 8, 4, 4), [0, 255, 0, 255]);
    pipe.destroy(&dev);
    q.destroy(&dev);
    dev.destroy_texture(target);
    dev.destroy_texture(depth);
    assert_clean(&inst);
}

#[test]
fn draw_format_mismatch_rejected() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let pipe = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        Some(Format::D32Sfloat),
        "flat.depth",
    );
    let target = offscreen_target(&dev, 4, 4);
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    c.texture_barrier(
        target,
        TextureState::Undefined,
        TextureState::ColorAttachment,
    );
    c.begin_rendering(&rhi::RenderingDesc {
        color: &[rhi::ColorAttachment {
            texture: target,
            load: rhi::LoadOp::Clear,
            store: rhi::StoreOp::Store,
            clear: [0.0; 4],
        }],
        depth: None,
        width: 4,
        height: 4,
    });
    c.bind_pipeline(pipe.pipeline); // pipeline expects a depth target
    c.end_rendering();
    assert!(matches!(c.finish(), Err(RhiError::InvalidDescriptor(_))));
    pipe.destroy(&dev);
    dev.destroy_texture(target);
    assert_clean(&inst);
}

#[test]
fn bindless_slot_not_reused_before_retire() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let a = dev.create_texture(&readable(4, 4)).unwrap();
    let a_index = dev.texture_index(a).unwrap();
    dev.destroy_texture(a);
    let b = dev.create_texture(&readable(4, 4)).unwrap();
    assert_ne!(
        dev.texture_index(b).unwrap(),
        a_index,
        "slot reused before retirement"
    );
    retire_all(&mut dev);
    let c = dev.create_texture(&readable(4, 4)).unwrap();
    assert_eq!(
        c.index(),
        a.index(),
        "quarantined slot reusable after retirement"
    );
    assert_ne!(c.generation(), a.generation());
    for h in [b, c] {
        dev.destroy_texture(h);
    }
    // Samplers follow the same rule.
    let s1 = sampler(&dev);
    dev.destroy_sampler(s1);
    let s2 = sampler(&dev);
    assert_ne!(s2.index(), s1.index());
    retire_all(&mut dev);
    let s3 = sampler(&dev);
    assert_eq!(s3.index(), s1.index());
    dev.destroy_sampler(s2);
    dev.destroy_sampler(s3);
    assert_clean(&inst);
}

#[test]
fn bindless_stale_index_rejected() {
    let Some((inst, dev)) = setup() else { return };
    let t = dev.create_texture(&readable(4, 4)).unwrap();
    let s = sampler(&dev);
    dev.destroy_texture(t);
    dev.destroy_sampler(s);
    assert!(matches!(
        dev.texture_index(t),
        Err(RhiError::StaleHandle {
            kind: "texture",
            ..
        })
    ));
    assert!(matches!(
        dev.sampler_index(s),
        Err(RhiError::StaleHandle {
            kind: "sampler",
            ..
        })
    ));
    dev.destroy_texture(t); // stale: warn + no-op
    dev.destroy_sampler(s);
    assert_clean(&inst);
}

#[test]
fn texture_unsupported_combo() {
    let Some((inst, dev)) = setup() else { return };
    let r = dev.create_texture(&tex_desc(4, 4, Format::D32Sfloat, TextureUsage::SAMPLED));
    assert!(matches!(r, Err(RhiError::Unsupported { .. })), "{r:?}");
    for d in [
        tex_desc(0, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED),
        tex_desc(4, 4, Format::Rgba8Unorm, TextureUsage::NONE),
        tex_desc(4, 4, Format::Rgba8Unorm, TextureUsage::DEPTH_ATTACHMENT),
        tex_desc(4, 4, Format::D32Sfloat, TextureUsage::COLOR_ATTACHMENT),
        tex_desc(1 << 20, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED),
    ] {
        let r = dev.create_texture(&d);
        assert!(
            matches!(r, Err(RhiError::InvalidDescriptor(_))),
            "{d:?}: {r:?}"
        );
    }
    assert_eq!(dev.diagnostics().live.textures, 0);
    assert_clean(&inst);
}

#[test]
fn sampler_create_destroy() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let s = sampler(&dev);
    assert_eq!(dev.sampler_index(s).unwrap(), s.index());
    assert_eq!(dev.diagnostics().live.samplers, 1);
    dev.destroy_sampler(s);
    assert_eq!(dev.diagnostics().live.samplers, 0);
    retire_all(&mut dev);
    assert_clean(&inst);
}

#[test]
fn texture_destroy_while_in_flight() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let tex = dev.create_texture(&readable(2, 2)).unwrap();
    dev.upload_texture(tex, &[1u8; 16]).unwrap();
    let smp = sampler(&dev);
    let q = Quad::new(&dev);
    let pipe = Pipe::new(
        &dev,
        SAMPLE_VERT,
        SAMPLE_FRAG,
        Format::Rgba8Unorm,
        None,
        "sample",
    );
    let target = offscreen_target(&dev, 8, 8);
    let (ti, si) = (
        dev.texture_index(tex).unwrap(),
        dev.sampler_index(smp).unwrap(),
    );
    let rb = readback_buffer(&dev, 8 * 8 * 4);
    // Submit a draw that samples the texture, destroy everything right after, no wait.
    frame(&mut dev, |c| {
        c.texture_barrier(
            target,
            TextureState::Undefined,
            TextureState::ColorAttachment,
        );
        c.begin_rendering(&rhi::RenderingDesc {
            color: &[rhi::ColorAttachment {
                texture: target,
                load: rhi::LoadOp::Clear,
                store: rhi::StoreOp::Store,
                clear: [0.0; 4],
            }],
            depth: None,
            width: 8,
            height: 8,
        });
        c.bind_pipeline(pipe.pipeline);
        push_sample(c, &q, ti, si);
        q.draw(c);
        c.end_rendering();
        c.texture_barrier(
            target,
            TextureState::ColorAttachment,
            TextureState::TransferSrc,
        );
        c.copy_texture_to_buffer(target, rb, 0);
    });
    dev.destroy_texture(tex);
    dev.destroy_texture(target);
    dev.destroy_sampler(smp);
    dev.destroy_buffer(rb);
    pipe.destroy(&dev);
    q.destroy(&dev);
    assert!(dev.diagnostics().pending_destroys > 0);
    retire_all(&mut dev);
    let d = dev.diagnostics();
    assert_eq!(
        (d.live.textures, d.live.samplers, d.live.buffers),
        (0, 0, 0)
    );
    assert_clean(&inst);
}

#[test]
fn diagnostics_bindless_and_memory() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let d0 = dev.diagnostics();
    assert!(d0.bindless.image_capacity >= 1024 && d0.bindless.image_capacity <= 65536);
    assert!(d0.bindless.sampler_capacity >= 64 && d0.bindless.sampler_capacity <= 1024);
    let t = dev.create_texture(&readable(64, 64)).unwrap();
    let s = sampler(&dev);
    let d1 = dev.diagnostics();
    assert_eq!((d1.live.textures, d1.live.samplers), (1, 1));
    assert_eq!(d1.bindless.image_high_water, t.index() + 1);
    assert_eq!(d1.bindless.sampler_high_water, s.index() + 1);
    assert!(d1.memory.allocated_bytes > d0.memory.allocated_bytes);
    dev.destroy_texture(t);
    dev.destroy_sampler(s);
    assert!(dev.diagnostics().memory.allocated_bytes > d0.memory.allocated_bytes);
    retire_all(&mut dev);
    let d2 = dev.diagnostics();
    assert_eq!(d2.memory.allocated_bytes, d0.memory.allocated_bytes);
    assert_eq!((d2.live.textures, d2.live.samplers), (0, 0));
    assert!(
        d2.bindless.image_high_water >= 1,
        "high-water never decreases"
    );
    assert_clean(&inst);
}

#[test]
fn texture_leak_at_shutdown() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let _t = dev.create_texture(&readable(4, 4)).unwrap();
    let _s = sampler(&dev);
    let cap = Capture::default();
    tracing::subscriber::with_default(cap.clone(), || dev.shutdown().unwrap());
    assert_eq!(cap.warnings_containing("leaked texture"), 1);
    assert_eq!(cap.warnings_containing("leaked sampler"), 1);
    assert_clean(&inst);
}
