//! Golden image comparison (ADR-0015 §1), shared by the renderer and runtime golden tests via
//! `#[path]`. References live in `crates/renderer/tests/golden/<name>.png`, one set for all
//! drivers; `ANVIL_BLESS=1` rewrites them.
#![allow(dead_code)] // each test binary uses a subset

use std::path::{Path, PathBuf};

/// A pixel is bad when any channel differs by more than this (out of 255).
pub const MAX_CHANNEL_ERROR: u8 = 8;
/// At least this fraction of the pixels must not be bad.
pub const MIN_GOOD_FRACTION: f64 = 0.99;
/// Root mean square error over all channels must not exceed this (out of 255).
pub const MAX_RMSE: f64 = 2.0;

/// The two thresholds; widen them for one scene only with a comment saying why.
#[derive(Clone, Copy)]
pub struct Tolerance {
    pub max_channel_error: u8,
    pub min_good_fraction: f64,
    pub max_rmse: f64,
}

pub const DEFAULT: Tolerance = Tolerance {
    max_channel_error: MAX_CHANNEL_ERROR,
    min_good_fraction: MIN_GOOD_FRACTION,
    max_rmse: MAX_RMSE,
};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../renderer/tests/golden")
}

fn failures_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    target.join("golden-failures")
}

fn write_png(path: &Path, (w, h): (u32, u32), rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn read_png(path: &Path) -> ((u32, u32), Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba, "{}", path.display());
    buf.truncate(info.buffer_size());
    ((info.width, info.height), buf)
}

/// Compares `rgba` (8 bit, `size`) with the reference `name`. On mismatch writes
/// `<name>.actual.png` and an amplified `<name>.diff.png` to `target/golden-failures/` and panics.
pub fn check(name: &str, size: (u32, u32), rgba: &[u8], tol: Tolerance) {
    let reference = golden_dir().join(format!("{name}.png"));
    if std::env::var("ANVIL_BLESS").as_deref() == Ok("1") {
        write_png(&reference, size, rgba);
        eprintln!("blessed {}", reference.display());
        return;
    }
    assert!(
        reference.exists(),
        "no reference {}; run with ANVIL_BLESS=1 to create it",
        reference.display()
    );
    let (ref_size, want) = read_png(&reference);
    let fail = |why: String, diff: Option<Vec<u8>>| -> ! {
        let dir = failures_dir();
        write_png(&dir.join(format!("{name}.actual.png")), size, rgba);
        if let Some(d) = diff {
            write_png(&dir.join(format!("{name}.diff.png")), size, &d);
        }
        panic!("golden {name}: {why} (images in {})", dir.display());
    };
    if ref_size != size || want.len() != rgba.len() {
        fail(format!("size {size:?}, reference {ref_size:?}"), None);
    }
    let mut bad = 0usize;
    let mut sq = 0f64;
    let mut diff = vec![255u8; rgba.len()];
    for ((a, b), d) in rgba.chunks(4).zip(want.chunks(4)).zip(diff.chunks_mut(4)) {
        let mut worst = 0u8;
        for c in 0..4 {
            let e = a[c].abs_diff(b[c]);
            worst = worst.max(e);
            sq += f64::from(e) * f64::from(e);
            if c < 3 {
                d[c] = e.saturating_mul(10); // amplified
            }
        }
        bad += usize::from(worst > tol.max_channel_error);
    }
    let pixels = rgba.len() / 4;
    let good = 1.0 - bad as f64 / pixels as f64;
    let rmse = (sq / rgba.len() as f64).sqrt();
    eprintln!("golden {name}: good {:.4}% rmse {rmse:.3}", good * 100.0);
    if good < tol.min_good_fraction || rmse > tol.max_rmse {
        fail(
            format!(
                "{bad} of {pixels} pixels off by more than {} (need {:.2}% good, got {:.2}%), rmse {rmse:.3} (max {})",
                tol.max_channel_error,
                tol.min_good_fraction * 100.0,
                good * 100.0,
                tol.max_rmse
            ),
            Some(diff),
        );
    }
}
