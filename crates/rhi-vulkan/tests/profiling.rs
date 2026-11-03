//! GPU tests for timestamp queries and the persisted pipeline cache (ADR-0013), gated by
//! `ANVIL_GPU_TESTS=1`.

mod common;

use common::{FLAT_FRAG, FLAT_VERT, Pipe, assert_clean, desc, enabled};
use rhi::{Backend, CommandEncoder, Device, DeviceDesc, Format, Instance};
use rhi_vulkan::Vulkan;
use rhi_vulkan::testing::assert_no_leaks;

#[test]
fn timestamps_round_trip() {
    let Some((inst, mut dev)) = common::setup() else {
        return;
    };
    let Some(info) = dev.timestamp_info() else {
        eprintln!("skipped: no timestamp support");
        return;
    };
    assert!(info.period_ns > 0.0 && info.valid_bits >= 32);
    let frames = dev.diagnostics().frames_in_flight;
    // Each frame writes two timestamps with a barrier-free gap; read back one ring later.
    for i in 0..frames * 3 {
        let t = dev.begin_frame().unwrap();
        if i >= frames {
            let ts = dev.read_timestamps(t.slot).unwrap();
            assert_eq!(ts.len(), 2, "frame {i}");
            assert!(ts[1] >= ts[0]);
        }
        let mut c = dev.begin_commands(&t).unwrap();
        c.write_timestamp(0);
        c.write_timestamp(1);
        let fin = c.finish().unwrap();
        dev.submit_unwindowed(fin).unwrap();
    }
    dev.wait_idle().unwrap();
    assert_no_leaks(&mut dev);
    assert_clean(&inst);
}

#[test]
fn pipeline_cache_saved_and_reloaded() {
    if !enabled() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("anvil-pcache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.bin");
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let make = || {
        inst.create_device(
            &DeviceDesc {
                pipeline_cache: Some(path.clone()),
                ..DeviceDesc::default()
            },
            None,
        )
        .unwrap()
    };
    let dev = make();
    let p = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        None,
        "cache-test",
    );
    p.destroy(&dev);
    drop(dev);
    // Some drivers (lavapipe) return a header-only cache; `save` then writes nothing.
    let driver_caches = path.exists();
    if driver_caches {
        assert!(std::fs::read(&path).unwrap().len() > 32);
    } else {
        eprintln!("driver returned an empty pipeline cache; file checks skipped");
    }

    // Reload: same device, valid header, pipeline creation still works; saved again.
    let dev = make();
    let p = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        None,
        "cache-test",
    );
    p.destroy(&dev);
    drop(dev);
    assert_eq!(path.exists(), driver_caches);

    // A corrupt file is ignored (device still creates) and replaced by a fresh cache.
    std::fs::write(
        &path,
        b"not a pipeline cache at all, just junk bytes of some length..",
    )
    .unwrap();
    let dev = make();
    let p = Pipe::new(
        &dev,
        FLAT_VERT,
        FLAT_FRAG,
        Format::Rgba8Unorm,
        None,
        "cache-test",
    );
    p.destroy(&dev);
    drop(dev);
    if driver_caches {
        assert!(std::fs::read(&path).unwrap().len() > 32, "junk replaced");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
