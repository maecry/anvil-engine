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
    Runtime(runtime::RuntimeError),
    /// The GPU device was lost; the process should exit with code 3 (ADR-0015 §4).
    #[error("GPU device lost")]
    DeviceLost,
    /// Asset failure.
    #[error(transparent)]
    Asset(#[from] assets::AssetError),
    /// Scene failure.
    #[error(transparent)]
    Scene(#[from] scene::SceneError),
    /// Device failure.
    #[error(transparent)]
    Rhi(rhi::RhiError),
    /// Renderer failure.
    #[error(transparent)]
    Render(renderer::RenderError),
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

impl From<runtime::RuntimeError> for EditorError {
    fn from(e: runtime::RuntimeError) -> Self {
        match e {
            runtime::RuntimeError::DeviceLost => Self::DeviceLost,
            e => Self::Runtime(e),
        }
    }
}

impl From<rhi::RhiError> for EditorError {
    fn from(e: rhi::RhiError) -> Self {
        match e {
            rhi::RhiError::DeviceLost => Self::DeviceLost,
            e => Self::Rhi(e),
        }
    }
}

impl From<renderer::RenderError> for EditorError {
    fn from(e: renderer::RenderError) -> Self {
        match e {
            renderer::RenderError::Rhi(e) => e.into(),
            e => Self::Render(e),
        }
    }
}
