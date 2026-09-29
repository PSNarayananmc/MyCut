//! Engine error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("tool not found: {0}. Install FFmpeg (with libx264 and libass) or use the bundled AppImage.")]
    ToolNotFound(String),
    #[error("ffmpeg/ffprobe failed: {0}")]
    Tool(String),
    #[error("failed to spawn process: {0}")]
    Spawn(std::io::Error),
    #[error("job was cancelled")]
    Cancelled,
    #[error("unsafe path rejected: {0}")]
    UnsafePath(String),
    #[error("invalid parameter {name}={value}: {reason}")]
    Param {
        name: String,
        value: String,
        reason: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
