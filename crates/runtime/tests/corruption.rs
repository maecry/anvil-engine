//! Corrupt-input tests (NFR-REL-005, ADR-0015 §6) for the readers that live above `assets`:
//! `project.anvil` and the scene file (parse, spawn, transform update). Seeded mutations of
//! the sample project's files; every call must return `Err` or `Ok`, never panic. The asset
//! formats are covered by `crates/assets/tests/corruption.rs`.

use std::path::Path;

use runtime::Project;
use scene::{SceneFile, World, update_transforms};

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

/// Truncation, byte flips, empty, junk, or a huge number written over the text.
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
        3 => {
            let huge = [
                &b"4294967295"[..],
                b"-2147483648",
                b"1e308",
                b"18446744073709551615",
            ];
            let at = rng.below(b.len());
            let end = (at + rng.below(12)).min(b.len());
            let h = huge[rng.below(huge.len())];
            b.splice(at..end, h.iter().copied());
        }
        _ => {
            // Deeply nested brackets: recursion limits.
            let at = rng.below(b.len());
            b.splice(at..at, std::iter::repeat_n(b'(', 10_000));
        }
    }
    b
}

const CASES: usize = 1500;

#[test]
fn project_and_scene_readers_never_panic() {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project");
    let project = std::fs::read(sample.join("project.anvil")).unwrap();
    let scene = std::fs::read(sample.join("assets/scenes/main.scene")).unwrap();

    let dir = tempfile::tempdir().unwrap();
    assert!(Project::load(dir.path()).is_err(), "missing file");
    std::fs::write(dir.path().join("project.anvil"), &project).unwrap();
    assert!(Project::load(dir.path()).is_ok());
    assert!(SceneFile::from_ron(std::str::from_utf8(&scene).unwrap()).is_ok());

    let mut rng = Rng(0x5EED);
    for i in 0..CASES {
        std::fs::write(
            dir.path().join("project.anvil"),
            mutate(&project, i, &mut rng),
        )
        .unwrap();
        let _ = Project::load(dir.path());

        let bytes = mutate(&scene, i, &mut rng);
        // Invalid UTF-8 never reaches `from_ron` (the loaders read text), so lossy-convert.
        if let Ok(file) = SceneFile::from_ron(&String::from_utf8_lossy(&bytes)) {
            let mut world = World::new();
            let _ = file.spawn_into(&mut world);
            update_transforms(&mut world);
        }
    }
}
