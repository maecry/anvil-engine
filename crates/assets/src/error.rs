use crate::AssetId;

/// Why an asset failed to load. Strings, not source errors, so the value is `Clone` and can be
/// kept in a [`crate::LoadState`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AssetError {
    /// Reading or writing a file failed, or the asset is unknown.
    #[error("io: {0}")]
    Io(String),
    /// The file is not a valid or supported instance of its format.
    #[error("format: {0}")]
    Format(String),
    /// The file was written by a newer engine than this one.
    #[error("version {found} is newer than the supported {supported}")]
    Version {
        /// Version found in the file.
        found: u32,
        /// Newest version this build reads.
        supported: u32,
    },
    /// A dependency failed to load.
    #[error("dependency {id} failed: {reason}")]
    Dependency {
        /// The failed dependency.
        id: AssetId,
        /// Its error, rendered.
        reason: String,
    },
    /// Internal data is inconsistent (bad indices, damaged cache entry, importer panic).
    #[error("corrupt: {0}")]
    Corrupt(String),
}
