//! Typed failures at an external agent runtime boundary.
use std::fmt;
/// Input remains queued because its target is not ready.
pub const EXIT_BUSY: i32 = 75;
/// Input may have been submitted and is quarantined rather than replayed.
pub const EXIT_TIMEOUT: i32 = 76;
/// Adapter failure with a stable process exit code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterError {
    message: String,
}
impl AdapterError {
    /// Record an unavailable runtime or unverifiable runtime response.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
    /// Human-readable failure detail.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Unavailable runtime status (`EX_UNAVAILABLE`).
    pub const fn exit_code(&self) -> i32 {
        69
    }
}
impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for AdapterError {}
/// Result of a runtime adapter operation.
pub type Result<T> = std::result::Result<T, AdapterError>;
