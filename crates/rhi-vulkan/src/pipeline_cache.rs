//! `VkPipelineCache` persistence (FR-VK-011, MODULE-007 §M5): load at device creation, save at
//! device destruction, header validated against the physical device.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Default cache file (relative to the working directory).
const DEFAULT_PATH: &str = "target/anvil-pipeline-cache.bin";
/// `VkPipelineCacheHeaderVersionOne` header size: size, version, vendor, device, UUID.
const HEADER_BYTES: usize = 32;

/// Identity of the device a cache blob must have been written by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheIdentity {
    pub vendor_id: u32,
    pub device_id: u32,
    pub uuid: [u8; 16],
}

/// The cache file: `desc` path, else env `ANVIL_PIPELINE_CACHE`, else [`DEFAULT_PATH`].
pub(crate) fn resolve_path(desc: Option<&Path>) -> PathBuf {
    desc.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ANVIL_PIPELINE_CACHE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH))
}

/// Why a blob is not usable; logged as a warning, the cache then starts empty.
pub(crate) fn validate(data: &[u8], id: &CacheIdentity) -> Result<(), String> {
    let word = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().expect("4 bytes"));
    if data.len() < HEADER_BYTES {
        return Err(format!("{} bytes, shorter than the header", data.len()));
    }
    if (word(0) as usize) < HEADER_BYTES || word(4) != 1 {
        return Err(format!(
            "unknown header (size {}, version {})",
            word(0),
            word(4)
        ));
    }
    if word(8) != id.vendor_id || word(12) != id.device_id {
        return Err(format!(
            "written by device {:04x}:{:04x}, this is {:04x}:{:04x}",
            word(8),
            word(12),
            id.vendor_id,
            id.device_id
        ));
    }
    if data[16..32] != id.uuid {
        return Err("pipelineCacheUUID differs (driver changed)".to_owned());
    }
    Ok(())
}

/// Reads and validates the file. `None` (with a warning for anything but a missing file)
/// means "start empty".
pub(crate) fn load(path: &Path, id: &CacheIdentity) -> Option<Vec<u8>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "pipeline cache unreadable, ignored");
            }
            return None;
        }
    };
    match validate(&data, id) {
        Ok(()) => {
            tracing::info!(path = %path.display(), bytes = data.len(), "pipeline cache loaded");
            Some(data)
        }
        Err(why) => {
            tracing::warn!(path = %path.display(), why, "pipeline cache invalid, ignored");
            None
        }
    }
}

/// Writes `data` atomically (temp file + rename, so concurrent devices never see a torn file).
/// Does not create directories. Failures only warn.
pub(crate) fn save(path: &Path, data: &[u8]) {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    if data.len() <= HEADER_BYTES {
        return; // nothing compiled
    }
    if path
        .parent()
        .is_some_and(|d| !d.as_os_str().is_empty() && !d.is_dir())
    {
        tracing::debug!(path = %path.display(), "pipeline cache directory missing, not saved");
        return;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = PathBuf::from(tmp);
    match std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => {
            tracing::info!(path = %path.display(), bytes = data.len(), "pipeline cache saved")
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(path = %path.display(), error = %e, "pipeline cache not saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(id: &CacheIdentity) -> Vec<u8> {
        let mut v = Vec::new();
        for w in [32u32, 1, id.vendor_id, id.device_id] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v.extend_from_slice(&id.uuid);
        v.extend_from_slice(&[7; 40]);
        v
    }

    #[test]
    fn header_validation() {
        let id = CacheIdentity {
            vendor_id: 0x1002,
            device_id: 0x7550,
            uuid: [3; 16],
        };
        let ok = blob(&id);
        assert!(validate(&ok, &id).is_ok());
        assert!(validate(&ok[..20], &id).is_err());
        assert!(validate(&[], &id).is_err());
        let other = CacheIdentity {
            vendor_id: 0x10de,
            ..id
        };
        assert!(validate(&ok, &other).unwrap_err().contains("written by"));
        let other = CacheIdentity {
            uuid: [4; 16],
            ..id
        };
        assert!(validate(&ok, &other).unwrap_err().contains("UUID"));
        let mut bad = ok.clone();
        bad[4] = 2;
        assert!(validate(&bad, &id).is_err());
    }

    #[test]
    fn save_then_load_and_corrupt() {
        let id = CacheIdentity {
            vendor_id: 1,
            device_id: 2,
            uuid: [9; 16],
        };
        let dir = std::env::temp_dir().join(format!("anvil-pc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.bin");
        assert!(load(&path, &id).is_none());
        let data = blob(&id);
        save(&path, &data);
        assert_eq!(load(&path, &id).as_deref(), Some(&data[..]));
        std::fs::write(&path, b"garbage").unwrap();
        assert!(load(&path, &id).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
