/// Runtime failures.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// `project.anvil` is missing, malformed or from a newer engine.
    #[error("project: {0}")]
    Project(String),
    /// Asset system failure.
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
    /// The GPU device was lost (driver reset, hang, unplug). There is no recovery; the run ends
    /// after releasing CPU-side state (ADR-0015 §4).
    #[error("GPU device lost")]
    DeviceLost,
    /// Window or event loop failure.
    #[error(transparent)]
    Platform(#[from] platform::PlatformError),
    /// File failure (screenshot, project).
    #[error("io: {0}")]
    Io(String),
}

impl From<rhi::RhiError> for RuntimeError {
    fn from(e: rhi::RhiError) -> Self {
        match e {
            rhi::RhiError::DeviceLost => Self::DeviceLost,
            e => Self::Rhi(e),
        }
    }
}

impl From<renderer::RenderError> for RuntimeError {
    fn from(e: renderer::RenderError) -> Self {
        match e {
            renderer::RenderError::Rhi(e) => e.into(),
            e => Self::Render(e),
        }
    }
}
