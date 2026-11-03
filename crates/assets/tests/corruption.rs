//! Corrupt-input tests (NFR-REL-005, ADR-0015 §6): seeded, deterministic mutations of every
//! format the asset server reads. Each load must end `Loaded` or `Failed`, never panic (an
//! importer panic is caught by the server and reported as `Corrupt("importer panicked")`, which
//! this test rejects) and never request an allocation the file could not justify.
//!
//! `project.anvil` and the scene file reader live in `runtime` / `scene`; their mutations are in
//! `crates/runtime/tests/corruption.rs`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use assets::fixtures::{FixtureMaterial, checker_png, cube, glb};
use assets::{AssetError, AssetServer, LoadState};

/// Tracks the largest single allocation request since the last reset.
struct Counting;
static MAX_ALLOC: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call unchanged to the system allocator; only records sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        MAX_ALLOC.fetch_max(l.size(), Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        MAX_ALLOC.fetch_max(n, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

/// xorshift64*: tiny, seeded, deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const HUGE: [u64; 5] = [
    0xFFFF_FFFF,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF_FFFF_FFFF,
    0x0100_0000,
];

/// Mutation `i` of `seed`: truncation, byte flips, empty, junk or a huge value in the file.
fn mutate(seed: &[u8], i: usize, rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    match i % 5 {
        0 => b.truncate(rng.below(seed.len())),
        1 => {
            for _ in 0..=rng.below(8) {
                let at = rng.below(b.len());
                b[at] ^= 1 << rng.below(8);
            }
        }
        2 if i % 50 == 2 => b.clear(),
        2 => b = (0..rng.below(512)).map(|_| rng.next() as u8).collect(),
        _ => {
            let v = HUGE[rng.below(HUGE.len())].to_le_bytes();
            let n = if rng.below(2) == 0 { 4 } else { 8 };
            // Header fields live near the start; bias the offset there.
            let range = if rng.below(4) == 0 {
                b.len()
            } else {
                b.len().min(96)
            };
            let at = rng.below(range);
            for (k, byte) in v[..n].iter().enumerate() {
                if let Some(d) = b.get_mut(at + k) {
                    *d = *byte;
                }
            }
        }
    }
    b
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &x in data {
        c ^= u32::from(x);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// PNG with huge IHDR dimensions and a valid CRC (so the decoder gets past the checksum).
fn png_huge(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for at in [16, 20] {
        let v = HUGE[rng.below(HUGE.len())] as u32;
        b[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    let crc = crc32(&b[12..29]);
    b[29..33].copy_from_slice(&crc.to_be_bytes());
    b
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, &b)| a | u32::from(b) << (16 - 8 * i));
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// A one-triangle glTF with an embedded buffer.
fn gltf_text() -> Vec<u8> {
    let mut buf = Vec::new();
    for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}}],
"bufferViews":[{{"buffer":0,"byteLength":36}}],
"buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{}"}}]}}"#,
        b64(&buf)
    )
    .into_bytes()
}

fn ktx2(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![
        0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
    ];
    // vkFormat R8G8B8A8_UNORM, typeSize, width, height, depth, layers, faces, levels, supercompression
    for v in [37, 1, w, h, 0, 0, 1, 1, 0] {
        b.extend_from_slice(&u32::to_le_bytes(v));
    }
    for v in [104u32, 4, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes()); // dfd offset/length, kvd offset/length
    }
    b.extend_from_slice(&[0; 16]); // sgd offset/length
    let len = u64::from(w * h * 4);
    for v in [108u64, len, len] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&[4, 0, 0, 0]); // minimal DFD (total size only)
    b.extend(std::iter::repeat_n(0x7Fu8, len as usize));
    b
}

fn encode(img: image::DynamicImage, fmt: image::ImageFormat) -> Vec<u8> {
    let mut c = Cursor::new(Vec::new());
    img.write_to(&mut c, fmt).unwrap();
    c.into_inner()
}

fn read_sample(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/sample-project")
        .join(rel);
    std::fs::read(p).unwrap()
}

/// Loads `rel` from a fresh server on `dir`; `None` when the server could not open the project
/// or the file was skipped.
fn load(dir: &Path, rel: &str) -> Option<LoadState> {
    let mut s = AssetServer::new(dir).ok()?;
    let id = s.load_path(rel).ok()?;
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        s.update();
        match s.state(id) {
            Some(LoadState::Loading) | None => {
                assert!(Instant::now() < end, "{rel} did not settle");
                std::thread::sleep(Duration::from_micros(200));
            }
            Some(st) => return Some(st),
        }
    }
}

fn check(what: &str, i: usize, st: Option<LoadState>, len: usize) {
    if let Some(LoadState::Failed(e)) = &st {
        assert!(
            !matches!(e, AssetError::Corrupt(m) if m.contains("panicked")),
            "{what} case {i}: importer panicked"
        );
    }
    let max = MAX_ALLOC.load(Ordering::Relaxed);
    // Decoders are capped at 128 MiB (`import_image`); anything else scales with the file.
    assert!(
        max <= (128 << 20) + len * 64,
        "{what} case {i}: allocated {max} bytes for a {len}-byte file"
    );
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

const CASES: usize = 1000;

#[test]
fn corrupt_inputs_fail_cleanly() {
    let mut cube_mesh = cube(1.0);
    cube_mesh.material = Some(0);
    let glb_bytes = glb(
        &[cube_mesh],
        &[FixtureMaterial {
            base_color: [1.0; 4],
            textured: true,
            metallic: 0.0,
            roughness: 0.5,
        }],
        Some(&checker_png(8, 2, [255; 3], [0; 3])),
    );
    let rgb = image::RgbImage::from_fn(16, 16, |x, y| image::Rgb([x as u8 * 8, y as u8 * 8, 99]));
    let rgbaf = image::Rgba32FImage::from_pixel(4, 4, image::Rgba([0.5, 1.5, 0.1, 1.0]));
    let png = checker_png(16, 4, [255; 3], [10; 3]);

    let seeds: Vec<(&str, &str, Vec<u8>)> = vec![
        ("glb", "m.glb", glb_bytes),
        ("gltf", "m.gltf", gltf_text()),
        ("png", "t.png", png),
        (
            "jpeg",
            "t.jpg",
            encode(
                image::DynamicImage::ImageRgb8(rgb),
                image::ImageFormat::Jpeg,
            ),
        ),
        (
            "exr",
            "t.exr",
            encode(
                image::DynamicImage::ImageRgba32F(rgbaf),
                image::ImageFormat::OpenExr,
            ),
        ),
        ("ktx2", "t.ktx2", ktx2(4, 4)),
        (
            "material",
            "m.material",
            read_sample("assets/materials/bricks.material"),
        ),
        ("scene", "s.scene", read_sample("assets/scenes/main.scene")),
    ];

    let mut total = 0;
    let started = Instant::now();
    for (n, (name, file, seed)) in seeds.iter().enumerate() {
        let rel = format!("assets/{file}");
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &rel, seed);
        // The unmutated seed must parse, or the mutations prove nothing (the sample material and
        // scene point at assets this temp project lacks, which is a dependency failure).
        let st = load(dir.path(), &rel);
        assert!(
            matches!(
                st,
                Some(LoadState::Loaded | LoadState::Failed(AssetError::Dependency { .. }))
            ),
            "{name} seed: {st:?}"
        );

        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ n as u64);
        for i in 0..CASES {
            let bytes = if *name == "png" && i % 5 == 3 {
                png_huge(seed, &mut rng)
            } else {
                mutate(seed, i, &mut rng)
            };
            write(dir.path(), &rel, &bytes);
            MAX_ALLOC.store(0, Ordering::Relaxed);
            let st = load(dir.path(), &rel);
            check(name, i, st, bytes.len());
            total += 1;
        }

        if *name == "material" {
            // Same seed, mutated `.meta` sidecar.
            let meta_rel = format!("{rel}.meta");
            let meta = std::fs::read(dir.path().join(&meta_rel)).unwrap();
            write(dir.path(), &rel, seed);
            for i in 0..CASES {
                let bytes = mutate(&meta, i, &mut rng);
                write(dir.path(), &meta_rel, &bytes);
                MAX_ALLOC.store(0, Ordering::Relaxed);
                let st = load(dir.path(), &rel);
                check("meta", i, st, bytes.len());
                total += 1;
            }
        }
    }

    // Cache `.bin` of a glb (meshes, textures, materials, prefab) and of a scene (RON): a bad
    // cache entry is dropped and the source reimported, so the load must still succeed.
    for (name, file) in [("glb", "m.glb"), ("scene", "s.scene")] {
        let seed = &seeds.iter().find(|s| s.0 == name).unwrap().2;
        let rel = format!("assets/{file}");
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &rel, seed);
        assert_eq!(load(dir.path(), &rel), Some(LoadState::Loaded));
        let bin = std::fs::read_dir(dir.path().join(".anvil/imported"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let cache = std::fs::read(&bin).unwrap();
        let mut rng = Rng(0xC0FFEE ^ cache.len() as u64);
        for i in 0..CASES {
            let bytes = mutate(&cache, i, &mut rng);
            std::fs::write(&bin, &bytes).unwrap();
            MAX_ALLOC.store(0, Ordering::Relaxed);
            let st = load(dir.path(), &rel);
            assert_eq!(st, Some(LoadState::Loaded), "cache({name}) case {i}");
            check("cache", i, st, bytes.len().max(seed.len()));
            total += 1;
        }
    }
    eprintln!("{total} cases in {:?}", started.elapsed());
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "corruption suite too slow"
    );
}
