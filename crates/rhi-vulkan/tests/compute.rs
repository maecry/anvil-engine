//! M5b RHI growth on a real device (gated by `ANVIL_GPU_TESTS=1`, validation and sync validation
//! on): compute and indirect dispatch, buffer barriers and fills, storage images per mip,
//! texture views, and indexed indirect draws with a GPU draw count.

mod common;

use common::*;
use rhi::{
    BufferDesc, BufferHandle, BufferState, BufferUsage, CommandEncoder, ComputePipelineDesc,
    Device, Format, Memory, RhiError, ShaderStageDesc, TextureDesc, TextureState, TextureUsage,
    TextureViewDesc,
};

fn buf(dev: &rhi_vulkan::VulkanDevice, label: &str, size: u64, usage: BufferUsage) -> BufferHandle {
    dev.create_buffer(&BufferDesc {
        label: Some(label),
        size,
        usage,
        memory: Memory::GpuOnly,
    })
    .unwrap()
}

fn read(dev: &rhi_vulkan::VulkanDevice, b: BufferHandle, n: usize) -> Vec<u32> {
    let mut out = vec![0u8; n * 4];
    dev.read_buffer(b, 0, &mut out).unwrap();
    out.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

#[test]
fn compute_writes_buffer_direct_indirect_and_fill() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let m = module(&dev, FILL_COMP, "fill.comp");
    let pipe = dev
        .create_compute_pipeline(&ComputePipelineDesc {
            label: Some("fill"),
            shader: ShaderStageDesc {
                module: m,
                entry: "main",
            },
        })
        .unwrap();
    const N: u32 = 200;
    let out = buf(
        &dev,
        "out",
        u64::from(N) * 4,
        BufferUsage::STORAGE | BufferUsage::TRANSFER_SRC | BufferUsage::TRANSFER_DST,
    );
    let args = buf(
        &dev,
        "args",
        12,
        BufferUsage::INDIRECT | BufferUsage::TRANSFER_DST,
    );
    dev.upload_buffer(args, 0, &bytes_of_u32(&[4, 1, 1]))
        .unwrap();
    let rb = dev
        .create_buffer(&BufferDesc {
            label: Some("rb"),
            size: u64::from(N) * 4,
            usage: BufferUsage::TRANSFER_DST,
            memory: Memory::Readback,
        })
        .unwrap();
    let addr = dev.buffer_address(out).unwrap();
    let push = |c: &mut rhi_vulkan::VulkanCommands, mul: u32| {
        let mut pc = [0u8; 16];
        pc[..8].copy_from_slice(&addr.to_le_bytes());
        pc[8..12].copy_from_slice(&N.to_le_bytes());
        pc[12..].copy_from_slice(&mul.to_le_bytes());
        c.push_constants(0, &pc);
    };
    for indirect in [false, true] {
        frame(&mut dev, |c| {
            // Reset with a fill, then compute over it, then copy out.
            c.fill_buffer(out, 0, u64::from(N) * 4, 0xFFFF_FFFF);
            c.buffer_barrier(out, BufferState::TransferDst, BufferState::ShaderWrite);
            c.bind_pipeline(pipe);
            push(c, if indirect { 7 } else { 3 });
            if indirect {
                c.dispatch_indirect(args, 0);
            } else {
                c.dispatch(4, 1, 1);
            }
            c.buffer_barrier(out, BufferState::ShaderWrite, BufferState::TransferSrc);
            c.copy_buffer_to_buffer(out, 0, rb, 0, u64::from(N) * 4);
        });
        dev.wait_idle().unwrap();
        let v = read(&dev, rb, N as usize);
        let mul = if indirect { 7 } else { 3 };
        assert!(v.iter().enumerate().all(|(i, x)| *x == i as u32 * mul));
    }
    for b in [out, args, rb] {
        dev.destroy_buffer(b);
    }
    dev.destroy_pipeline(pipe);
    dev.destroy_shader_module(m);
    retire_all(&mut dev);
    assert_clean(&inst);
}

#[test]
fn storage_images_per_mip_and_view_creation() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let m = module(&dev, STORAGE_COMP, "storage.comp");
    let pipe = dev
        .create_compute_pipeline(&ComputePipelineDesc {
            label: Some("storage"),
            shader: ShaderStageDesc {
                module: m,
                entry: "main",
            },
        })
        .unwrap();
    let tex = dev
        .create_texture(&TextureDesc {
            label: Some("pyramid"),
            width: 16,
            height: 8,
            format: Format::R32Float,
            usage: TextureUsage::SAMPLED | TextureUsage::STORAGE | TextureUsage::TRANSFER_SRC,
            mip_levels: 5,
            array_layers: 1,
            cube: false,
        })
        .unwrap();
    // The whole mip chain is samplable, but not a storage image (several mips).
    dev.texture_index(tex).unwrap();
    assert!(dev.storage_index(tex).is_err());
    let view_desc = |mip| TextureViewDesc {
        label: Some("pyramid.mip"),
        base_mip: mip,
        mip_count: 1,
        base_layer: 0,
        layer_count: 1,
        usage: TextureUsage::SAMPLED | TextureUsage::STORAGE | TextureUsage::TRANSFER_SRC,
    };
    let views: Vec<_> = (0..5)
        .map(|m| dev.create_texture_view(tex, &view_desc(m)).unwrap())
        .collect();
    let mut slots: Vec<u32> = views
        .iter()
        .map(|v| {
            let s = dev.storage_index(*v).unwrap();
            assert_eq!(s, dev.texture_index(*v).unwrap());
            s
        })
        .collect();
    slots.push(dev.texture_index(tex).unwrap());
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots.len(), 6, "every view has its own bindless slot");
    // Invalid views.
    let bad = |d: TextureViewDesc| {
        matches!(
            dev.create_texture_view(tex, &d),
            Err(RhiError::InvalidDescriptor(_))
        )
    };
    assert!(bad(TextureViewDesc {
        base_mip: 5,
        ..view_desc(0)
    }));
    assert!(bad(TextureViewDesc {
        mip_count: 3,
        usage: TextureUsage::STORAGE,
        ..view_desc(3)
    }));
    assert!(bad(TextureViewDesc {
        usage: TextureUsage::COLOR_ATTACHMENT,
        ..view_desc(0)
    }));
    assert!(bad(TextureViewDesc {
        layer_count: 2,
        ..view_desc(0)
    }));
    assert!(
        dev.create_texture_view(views[0], &view_desc(0)).is_err(),
        "no view of a view"
    );
    // Invalid texture descriptions.
    let d = |mips, layers, cube| TextureDesc {
        label: None,
        width: 16,
        height: 16,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::SAMPLED,
        mip_levels: mips,
        array_layers: layers,
        cube,
    };
    assert!(
        dev.create_texture(&d(6, 1, false)).is_err(),
        "too many mips"
    );
    assert!(dev.create_texture(&d(0, 1, false)).is_err());
    assert!(
        dev.create_texture(&d(1, 5, true)).is_err(),
        "cube needs 6 layers"
    );

    let sizes = [(16u32, 8u32), (8, 4), (4, 2), (2, 1), (1, 1)];
    let rbs: Vec<_> = sizes
        .iter()
        .map(|(w, h)| readback_buffer(&dev, u64::from(w * h) * 4))
        .collect();
    let storage_slots: Vec<u32> = views
        .iter()
        .map(|v| dev.storage_index(*v).unwrap())
        .collect();
    frame(&mut dev, |c| {
        c.bind_pipeline(pipe);
        for (i, v) in views.iter().enumerate() {
            c.texture_barrier(*v, TextureState::Undefined, TextureState::StorageWrite);
            let (w, h) = sizes[i];
            let mut pc = [0u8; 16];
            pc[..4].copy_from_slice(&storage_slots[i].to_le_bytes());
            pc[4..8].copy_from_slice(&w.to_le_bytes());
            pc[8..12].copy_from_slice(&h.to_le_bytes());
            pc[12..].copy_from_slice(&(i as f32 * 100.0).to_le_bytes());
            c.push_constants(0, &pc);
            c.dispatch(w.div_ceil(8), h.div_ceil(8), 1);
            c.texture_barrier(*v, TextureState::StorageWrite, TextureState::TransferSrc);
            c.copy_texture_to_buffer(*v, rbs[i], 0);
        }
    });
    dev.wait_idle().unwrap();
    for (i, rb) in rbs.iter().enumerate() {
        let (w, h) = sizes[i];
        let v = read(&dev, *rb, (w * h) as usize);
        for (n, x) in v.iter().enumerate() {
            let want = i as f32 * 100.0 + (n as u32 % w) as f32;
            assert_eq!(f32::from_bits(*x), want, "mip {i} texel {n}");
        }
        dev.destroy_buffer(*rb);
    }
    // A cube map with per-face views.
    let cube = dev
        .create_texture(&TextureDesc {
            label: Some("cube"),
            width: 8,
            height: 8,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED | TextureUsage::COLOR_ATTACHMENT,
            mip_levels: 4,
            array_layers: 6,
            cube: true,
        })
        .unwrap();
    // A whole cube is sampled through `textureCube[]` in the same binding (M5c).
    dev.texture_index(cube).unwrap();
    let faces: Vec<_> = (0..6)
        .map(|f| {
            dev.create_texture_view(
                cube,
                &TextureViewDesc {
                    label: Some("cube.face"),
                    base_mip: 0,
                    mip_count: 1,
                    base_layer: f,
                    layer_count: 1,
                    usage: TextureUsage::SAMPLED | TextureUsage::COLOR_ATTACHMENT,
                },
            )
            .unwrap()
        })
        .collect();
    for f in &faces {
        dev.texture_index(*f).unwrap();
    }
    for v in views.into_iter().chain(faces) {
        dev.destroy_texture(v);
    }
    dev.destroy_texture(cube);
    dev.destroy_texture(tex);
    dev.destroy_pipeline(pipe);
    dev.destroy_shader_module(m);
    retire_all(&mut dev);
    assert_clean(&inst);
}

#[test]
fn indexed_indirect_and_count_draws() {
    let Some((inst, mut dev)) = setup() else {
        return;
    };
    let (w, h) = (32u32, 32u32);
    let quad = Quad::new(&dev);
    let pipe = Pipe::new(&dev, FLAT_VERT, FLAT_FRAG, Format::Rgba8Unorm, None, "flat");
    let target = offscreen_target(&dev, w, h);
    // Two triangles of the quad as two commands: (index_count, instances, first_index, base
    // vertex, first instance).
    let cmds = buf(
        &dev,
        "cmds",
        40,
        BufferUsage::INDIRECT | BufferUsage::TRANSFER_DST,
    );
    dev.upload_buffer(cmds, 0, &bytes_of_u32(&[3, 1, 0, 0, 0, 3, 1, 3, 0, 0]))
        .unwrap();
    let counts = buf(
        &dev,
        "counts",
        8,
        BufferUsage::INDIRECT | BufferUsage::TRANSFER_DST,
    );
    let run = |dev: &mut rhi_vulkan::VulkanDevice, mode: u32, n: u32| {
        dev.upload_buffer(counts, 0, &bytes_of_u32(&[n, 0]))
            .unwrap();
        render_offscreen(dev, target, None, (w, h), |c| {
            c.bind_pipeline(pipe.pipeline);
            let mut pc = [0u8; 32];
            pc[..8].copy_from_slice(&quad.vertex_address.to_le_bytes());
            pc[16..20].copy_from_slice(&1.0f32.to_le_bytes());
            pc[20..24].copy_from_slice(&1.0f32.to_le_bytes());
            pc[24..28].copy_from_slice(&0.5f32.to_le_bytes());
            c.push_constants(0, &pc[..28]);
            c.bind_index_buffer(quad.indices, 0, rhi::IndexFormat::U32);
            if mode == 0 {
                c.draw_indexed_indirect(cmds, 0, n, 20);
            } else {
                c.draw_indexed_indirect_count(cmds, 0, counts, 0, 2, 20);
            }
        })
    };
    let (tl, br) = ((1, 1), (w - 2, h - 2));
    for mode in [0, 1] {
        for n in [0, 1, 2] {
            let img = run(&mut dev, mode, n);
            let lit = |p: (u32, u32)| pixel(&img, w, p.0, p.1)[0] > 200;
            assert_eq!(
                (lit(tl), lit(br)),
                (n >= 1, n >= 2),
                "mode {mode} count {n}"
            );
        }
    }
    dev.destroy_buffer(cmds);
    dev.destroy_buffer(counts);
    dev.destroy_texture(target);
    quad.destroy(&dev);
    pipe.destroy(&dev);
    retire_all(&mut dev);
    assert_clean(&inst);
}
