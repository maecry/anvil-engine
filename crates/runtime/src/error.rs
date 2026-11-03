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
    Rhi(#[from] rhi::RhiError),
    /// Renderer failure.
    #[error(transparent)]
    Render(#[from] renderer::RenderError),
    /// Window or event loop failure.
    #[error(transparent)]
    Platform(#[from] platform::PlatformError),
    /// File failure (screenshot, project).
    #[error("io: {0}")]
    Io(String),
}
