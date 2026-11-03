//! Runs `spirv-val --target-env vulkan1.3` on every SPIR-V blob this crate's build produced
//! (ADR-0015 §5, FR-SHD-009). The `debug` and `rhi-vulkan` crates include this file with
//! `#[path]` to check their own `OUT_DIR`. Skipped with a message when `spirv-val` is missing
//! (CI installs `spirv-tools`).

use std::path::Path;
use std::process::Command;

#[test]
fn embedded_spirv_is_valid_for_vulkan_1_3() {
    let dir = Path::new(env!("OUT_DIR"));
    let mut blobs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "spv"))
        .collect();
    blobs.sort();
    assert!(!blobs.is_empty(), "no .spv files in {}", dir.display());
    let mut failures = Vec::new();
    for blob in &blobs {
        let out = match Command::new("spirv-val")
            .args(["--target-env", "vulkan1.3"])
            .arg(blob)
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                eprintln!("skipped: spirv-val not available ({e}); install spirv-tools");
                return;
            }
        };
        if !out.status.success() {
            failures.push(format!(
                "{}: {}",
                blob.file_name().unwrap().to_string_lossy(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    eprintln!(
        "validated {} SPIR-V blobs in {}",
        blobs.len(),
        dir.display()
    );
    assert!(
        failures.is_empty(),
        "invalid SPIR-V:\n{}",
        failures.join("\n")
    );
}
