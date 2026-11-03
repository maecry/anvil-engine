/// Editor failures.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    /// A command was rejected (unknown entity or asset, wrong state, failed assertion).
    #[error("{0}")]
    Command(String),
    /// File failure.
    #[error("io: {0}")]
    Io(String),
    /// Runtime failure.
    #[error(transparent)]
    Runtime(#[from] runtime::RuntimeError),
    /// Asset failure.
    #[error(transparent)]
    Asset(#[from] assets::AssetError),
    /// Scene failure.
    #[error(transparent)]
    Scene(#[from] scene::SceneError),
    /// Device failure.
    #[error(transparent)]
    Rhi(#[from] rhi::RhiError),
    /// Renderer failure.
    #[error(transparent)]
    Render(#[from] renderer::RenderError),
    /// Window or event loop failure.
    #[error(transparent)]
    Platform(#[from] platform::PlatformError),
}

impl EditorError {
    pub(crate) fn cmd(msg: impl Into<String>) -> Self {
        Self::Command(msg.into())
    }

    pub(crate) fn io(path: &std::path::Path, e: &dyn std::fmt::Display) -> Self {
        Self::Io(format!("{}: {e}", path.display()))
    }
}
