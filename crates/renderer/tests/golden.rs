//! Golden-image regression tests (ADR-0015 §1, MASTER-000 §15.3), gated by `ANVIL_GPU_TESTS=1`
//! with validation and sync validation on. Each scene renders headless at 256x256 with a fixed
//! camera, TAA off, and is compared with `tests/golden/<name>.png` (one reference set for all
//! drivers, blessed on RADV; `ANVIL_BLESS=1` rewrites them). Failures leave the actual image and
//! an amplified diff in `target/golden-failures/`.

use foundation::math::{Mat4, Quat, Vec3};
use renderer::{
    AlphaMode, Camera, DebugDraw, Environment, Light, MaterialDesc, MeshInstance, PostSettings,
    RenderScene, Renderer, mesh,
};
use rhi::{
    Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc, Format, Instance,
    InstanceDesc, Memory, SwapchainHandle, SwapchainImage, TextureDesc, TextureState, TextureUsage,
    ValidationMode,
};
use rhi_vulkan::{Vulkan, VulkanDevice, testing::assert_no_leaks};

#[path = "golden/compare.rs"]
mod compare;
use compare::DEFAULT;

const SIZE: (u32, u32) = (256, 256);
const NO_POST: PostSettings = PostSettings {
    ssao: false,
    taa: false,
    bloom: false,
};

/// Renders `frames` frames of the scene built by `setup`, checks it against `name`, then tears
/// everything down and asserts nothing leaked and validation stayed silent.
fn golden(
    name: &str,
    tol: compare::Tolerance,
    setup: impl FnOnce(&mut Renderer, &VulkanDevice) -> RenderScene,
) {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return;
    }
    let _ = foundation::init_logging();
    let inst = Vulkan::create_instance(&InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    })
    .unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let format = Format::Rgba8Srgb;
    let mut r = Renderer::new(&dev, format, SIZE).unwrap();
    r.set_post(NO_POST);
    let scene = setup(&mut r, &dev);
    let target = dev
        .create_texture(&TextureDesc {
            label: Some("golden.target"),
            width: SIZE.0,
            height: SIZE.1,
            format,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
            mip_levels: 1,
            array_layers: 1,
            cube: false,
        })
        .unwrap();
    let image = SwapchainImage {
        swapchain: SwapchainHandle::NULL,
        texture: target,
        index: 0,
        extent: SIZE,
        format,
    };
    let rb = dev
        .create_buffer(&BufferDesc {
            label: Some("golden.readback"),
            size: u64::from(SIZE.0 * SIZE.1 * 4),
            usage: BufferUsage::TRANSFER_DST,
            memory: Memory::Readback,
        })
        .unwrap();
    // Several frames: environment bakes and streaming settle. One shared target and readback, so
    // frames must not overlap.
    for _ in 0..4 {
        let frame = dev.begin_frame().unwrap();
        let mut cmd = dev.begin_commands(&frame).unwrap();
        r.render(
            &dev,
            &mut cmd,
            &frame,
            &scene,
            &image,
            TextureState::TransferSrc,
        )
        .unwrap();
        cmd.copy_texture_to_buffer(target, rb, 0);
        let fin = cmd.finish().unwrap();
        dev.submit_unwindowed(fin).unwrap();
        dev.wait_idle().unwrap();
    }
    let mut out = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    dev.destroy_buffer(rb);
    dev.destroy_texture(target);
    r.destroy(&dev);
    assert_no_leaks(&mut dev);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
    compare::check(name, SIZE, &out, tol);
}

fn camera(position: Vec3, pitch: f32) -> Camera {
    Camera {
        position,
        rotation: Quat::from_rotation_x(pitch),
        fov_y: 1.0,
        near: 0.1,
        exposure: 1.0,
    }
}

fn sphere_at(r: &mut Renderer, dev: &VulkanDevice) -> renderer::MeshId {
    r.create_mesh(dev, "sphere", &mesh::uv_sphere(32, 16))
        .unwrap()
}

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// No lights, no environment: only emissive shows.
#[test]
fn unlit() {
    golden("unlit", DEFAULT, |r, dev| {
        let sphere = sphere_at(r, dev);
        let cube = r.create_mesh(dev, "cube", &mesh::cube()).unwrap();
        let mut emissive = |c| {
            r.create_material(&MaterialDesc {
                base_color: [0.0, 0.0, 0.0, 1.0],
                emissive: c,
                ..MaterialDesc::default()
            })
        };
        let (red, green, blue) = (
            emissive([0.9, 0.1, 0.1]),
            emissive([0.1, 0.8, 0.2]),
            emissive([0.2, 0.3, 0.9]),
        );
        RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 6.0), 0.0),
            meshes: vec![
                MeshInstance::new(sphere, red, at(-2.0, 0.0, 0.0)),
                MeshInstance::new(cube, green, at(0.0, 0.0, 0.0)),
                MeshInstance::new(sphere, blue, at(2.0, 0.0, 0.0)),
            ],
            lights: vec![],
            environment: Environment {
                intensity: 0.0,
                ..Environment::default()
            },
        }
    });
}

/// Metallic/roughness grid lit by the default environment only (IBL).
#[test]
fn pbr_spheres_ibl() {
    golden("pbr_spheres_ibl", DEFAULT, |r, dev| {
        let sphere = sphere_at(r, dev);
        let mut meshes = Vec::new();
        for row in 0..3u32 {
            for col in 0..4u32 {
                let m = r.create_material(&MaterialDesc {
                    base_color: [0.9, 0.55, 0.3, 1.0],
                    metallic: row as f32 * 0.5,
                    roughness: 0.15 + col as f32 * 0.28,
                    ..MaterialDesc::default()
                });
                meshes.push(MeshInstance::new(
                    sphere,
                    m,
                    at(col as f32 * 1.4 - 2.1, 1.4 - row as f32 * 1.4, 0.0)
                        * Mat4::from_scale(Vec3::splat(0.6)),
                ));
            }
        }
        RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 6.0), 0.0),
            meshes,
            lights: vec![],
            environment: Environment::default(),
        }
    });
}

/// A directional shadow (cascades) and a point-light cube shadow on a plane.
#[test]
fn shadows() {
    golden("shadows", DEFAULT, |r, dev| {
        let (plane, cube, sphere) = (
            r.create_mesh(dev, "plane", &mesh::plane(20.0)).unwrap(),
            r.create_mesh(dev, "cube", &mesh::cube()).unwrap(),
            sphere_at(r, dev),
        );
        let m = r.create_material(&MaterialDesc {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.8,
            ..MaterialDesc::default()
        });
        let sun = Vec3::new(0.5, -1.0, -0.3).normalize();
        RenderScene {
            camera: camera(Vec3::new(0.0, 4.5, 6.5), -0.6),
            meshes: vec![
                MeshInstance::new(plane, m, Mat4::IDENTITY),
                MeshInstance::new(cube, m, at(-1.5, 0.5, 0.0)),
                MeshInstance::new(sphere, m, at(1.5, 0.6, 0.5)),
            ],
            lights: vec![
                Light::Directional {
                    direction: sun,
                    color: Vec3::new(1.0, 0.95, 0.85),
                    intensity: 2.5,
                    cast_shadows: true,
                },
                Light::Point {
                    position: Vec3::new(0.0, 2.5, 2.5),
                    color: Vec3::new(0.4, 0.6, 1.0),
                    intensity: 30.0,
                    range: 10.0,
                    cast_shadows: true,
                },
            ],
            environment: Environment {
                sun_direction: sun,
                intensity: 0.3,
                ..Environment::default()
            },
        }
    });
}

/// 64x64 RGBA texture: orange where the 8-texel checker is on, transparent elsewhere.
fn checker_texels() -> Vec<u8> {
    (0..64 * 64)
        .flat_map(|i| {
            let (x, y) = (i % 64, i / 64);
            let on = (x / 8 + y / 8) % 2 == 0;
            [230, 130, 30, if on { 255 } else { 0 }]
        })
        .collect()
}

/// A double-sided alpha-masked quad in front of a lit wall.
#[test]
fn alpha_mask() {
    golden("alpha_mask", DEFAULT, |r, dev| {
        let (plane, cube) = (
            r.create_mesh(dev, "plane", &mesh::plane(2.0)).unwrap(),
            r.create_mesh(dev, "cube", &mesh::cube()).unwrap(),
        );
        let tex = r
            .create_texture(dev, "checker", &checker_texels(), 64, 64, true)
            .unwrap();
        let masked = r.create_material(&MaterialDesc {
            base_color_texture: Some(tex),
            metallic: 0.0,
            roughness: 0.7,
            alpha_mode: AlphaMode::Mask(0.5),
            double_sided: true,
            ..MaterialDesc::default()
        });
        let wall = r.create_material(&MaterialDesc {
            base_color: [0.2, 0.5, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.8,
            ..MaterialDesc::default()
        });
        // The plane lies in XZ; stand it up to face the camera.
        let upright = at(0.0, 0.0, 1.0) * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2);
        RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 5.0), 0.0),
            meshes: vec![
                MeshInstance::new(
                    cube,
                    wall,
                    at(0.0, 0.0, -1.0) * Mat4::from_scale(Vec3::new(3.0, 3.0, 0.2)),
                ),
                MeshInstance::new(plane, masked, upright * Mat4::from_scale(Vec3::splat(1.6))),
            ],
            lights: vec![Light::Directional {
                direction: Vec3::new(0.3, -0.4, -1.0).normalize(),
                color: Vec3::ONE,
                intensity: 3.0,
                cast_shadows: false,
            }],
            environment: Environment::default(),
        }
    });
}

/// Overlapping blended spheres over an opaque one, sorted back to front.
#[test]
fn transparency() {
    golden("transparency", DEFAULT, |r, dev| {
        let sphere = sphere_at(r, dev);
        let opaque = r.create_material(&MaterialDesc {
            base_color: [0.9, 0.9, 0.9, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            ..MaterialDesc::default()
        });
        let mut glass = |c: [f32; 4]| {
            r.create_material(&MaterialDesc {
                base_color: c,
                metallic: 0.0,
                roughness: 0.2,
                alpha_mode: AlphaMode::Blend,
                ..MaterialDesc::default()
            })
        };
        let (red, green, blue) = (
            glass([0.9, 0.2, 0.2, 0.5]),
            glass([0.2, 0.9, 0.2, 0.5]),
            glass([0.2, 0.2, 0.9, 0.5]),
        );
        RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 6.0), 0.0),
            meshes: vec![
                MeshInstance::new(
                    sphere,
                    opaque,
                    at(0.0, 0.0, -1.5) * Mat4::from_scale(Vec3::splat(1.2)),
                ),
                MeshInstance::new(sphere, red, at(-0.7, 0.4, 1.0)),
                MeshInstance::new(sphere, green, at(0.7, 0.4, 0.5)),
                MeshInstance::new(sphere, blue, at(0.0, -0.6, 1.5)),
            ],
            lights: vec![Light::Directional {
                direction: Vec3::new(0.4, -0.6, -0.7).normalize(),
                color: Vec3::ONE,
                intensity: 3.0,
                cast_shadows: false,
            }],
            environment: Environment::default(),
        }
    });
}

/// Bloom and SSAO on a bright emissive sphere over a lit ground.
#[test]
fn post() {
    golden("post", DEFAULT, |r, dev| {
        r.set_post(PostSettings {
            ssao: true,
            taa: false,
            bloom: true,
        });
        let (plane, sphere, cube) = (
            r.create_mesh(dev, "plane", &mesh::plane(20.0)).unwrap(),
            sphere_at(r, dev),
            r.create_mesh(dev, "cube", &mesh::cube()).unwrap(),
        );
        let ground = r.create_material(&MaterialDesc {
            base_color: [0.6, 0.6, 0.6, 1.0],
            metallic: 0.0,
            roughness: 0.9,
            ..MaterialDesc::default()
        });
        let glow = r.create_material(&MaterialDesc {
            base_color: [0.0, 0.0, 0.0, 1.0],
            emissive: [12.0, 7.0, 2.0],
            ..MaterialDesc::default()
        });
        RenderScene {
            camera: camera(Vec3::new(0.0, 2.5, 6.0), -0.3),
            meshes: vec![
                MeshInstance::new(plane, ground, Mat4::IDENTITY),
                MeshInstance::new(
                    sphere,
                    glow,
                    at(0.0, 1.0, 0.0) * Mat4::from_scale(Vec3::splat(0.7)),
                ),
                MeshInstance::new(cube, ground, at(-2.0, 0.5, 0.5)),
            ],
            lights: vec![Light::Directional {
                direction: Vec3::new(0.3, -1.0, -0.5).normalize(),
                color: Vec3::ONE,
                intensity: 2.0,
                cast_shadows: false,
            }],
            environment: Environment::default(),
        }
    });
}

/// A grid of spheres drawn through the GPU culling path, with debug bounds.
#[test]
fn gpu_culled_grid() {
    golden("gpu_culled_grid", DEFAULT, |r, dev| {
        r.set_gpu_culling(true);
        r.set_lod_pixels(0.0);
        r.set_debug_draw(DebugDraw::None);
        let sphere = sphere_at(r, dev);
        let m = r.create_material(&MaterialDesc {
            base_color: [0.7, 0.75, 0.9, 1.0],
            metallic: 0.2,
            roughness: 0.5,
            ..MaterialDesc::default()
        });
        let meshes = (0..100u32)
            .map(|i| {
                let (x, y) = ((i % 10) as f32 - 4.5, (i / 10) as f32 - 4.5);
                MeshInstance::new(
                    sphere,
                    m,
                    at(x * 0.9, y * 0.9, 0.0) * Mat4::from_scale(Vec3::splat(0.4)),
                )
            })
            .collect();
        RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 8.0), 0.0),
            meshes,
            lights: vec![Light::Directional {
                direction: Vec3::new(0.3, -0.5, -1.0).normalize(),
                color: Vec3::ONE,
                intensity: 3.0,
                cast_shadows: false,
            }],
            environment: Environment::default(),
        }
    });
}
