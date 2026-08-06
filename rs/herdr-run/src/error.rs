//! Typed failures and stable process exit codes for `herdr-run`.

use std::fmt;

/// Configuration is malformed or cannot be read (`EX_CONFIG`).
pub const EXIT_CONFIG: i32 = 78;
/// The command was rejected by policy (`EX_NOPERM`).
pub const EXIT_REFUSED: i32 = 77;
/// Herdr or the requested session target is unavailable (`EX_UNAVAILABLE`).
pub const EXIT_UNAVAILABLE: i32 = 69;
/// The pane was not safe to use (`EX_TEMPFAIL`).
pub const EXIT_BUSY: i32 = 75;
/// The command was launched but did not report completion in time.
pub const EXIT_TIMEOUT: i32 = 76;

/// Stable categories for failures originating in `herdr-run` itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// A malformed, unreadable, or otherwise unusable project configuration.
    Config,
    /// A command rejected by the allowlist before any pane interaction.
    Refused,
    /// A missing or unusable Herdr server, workspace, tab, or pane.
    Unavailable,
    /// A pane that was not observably idle and safe to type into.
    Busy,
    /// A launched command whose completion file did not appear in time.
    Timeout,
    /// A generic tool failure without a more specific stable category.
    Other,
}

impl ErrorKind {
    /// Return the process exit code assigned to this failure category.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Config => EXIT_CONFIG,
            Self::Refused => EXIT_REFUSED,
            Self::Unavailable => EXIT_UNAVAILABLE,
            Self::Busy => EXIT_BUSY,
            Self::Timeout => EXIT_TIMEOUT,
            Self::Other => 1,
        }
    }
}

/// One typed `herdr-run` failure with human-readable context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HerdrRunError {
    kind: ErrorKind,
    message: String,
}

impl HerdrRunError {
    /// Construct a failure of `kind` with the supplied message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Construct a configuration failure.
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    /// Construct a policy refusal.
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Refused, message)
    }

    /// Construct a Herdr/session availability failure.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unavailable, message)
    }

    /// Construct a pane-busy failure.
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Busy, message)
    }

    /// Construct a launched-command timeout.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    /// Return this failure's stable category.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Return the diagnostic text without a CLI prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Return the process exit code assigned to this failure.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }
}

impl fmt::Display for HerdrRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HerdrRunError {}

/// Result type used throughout the crate.
pub type Result<T> = std::result::Result<T, HerdrRunError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_public_failure_kind_has_the_python_exit_code() {
        assert_eq!(ErrorKind::Config.exit_code(), 78);
        assert_eq!(ErrorKind::Refused.exit_code(), 77);
        assert_eq!(ErrorKind::Unavailable.exit_code(), 69);
        assert_eq!(ErrorKind::Busy.exit_code(), 75);
        assert_eq!(ErrorKind::Timeout.exit_code(), 76);
        assert_eq!(ErrorKind::Other.exit_code(), 1);
    }

    #[test]
    fn constructors_preserve_kind_message_and_display() {
        let error = HerdrRunError::refused("program 'sh' is not allowlisted");
        assert_eq!(error.kind(), ErrorKind::Refused);
        assert_eq!(error.exit_code(), EXIT_REFUSED);
        assert_eq!(error.message(), "program 'sh' is not allowlisted");
        assert_eq!(error.to_string(), error.message());
    }
}
