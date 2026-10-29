#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    /// The process is not trusted for Accessibility, so an event tap could not be created.
    #[error("Accessibility permission is required")]
    PermissionDenied,
    #[error("I/O error: {message}")]
    Io { message: String },
    #[error("invalid config: {message}")]
    InvalidConfig { message: String },
    #[error("{message}")]
    Other { message: String },
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        CoreError::Io {
            message: error.to_string(),
        }
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(error: serde_json::Error) -> Self {
        CoreError::InvalidConfig {
            message: error.to_string(),
        }
    }
}

impl From<toml::de::Error> for CoreError {
    fn from(error: toml::de::Error) -> Self {
        CoreError::InvalidConfig {
            message: error.to_string(),
        }
    }
}

impl From<toml::ser::Error> for CoreError {
    fn from(error: toml::ser::Error) -> Self {
        CoreError::InvalidConfig {
            message: error.to_string(),
        }
    }
}
