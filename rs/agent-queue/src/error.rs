//! One error type for every queue, file and process-identity operation.

use std::fmt;
use std::io;
use std::path::Path;

/// What kind of failure an [`Error`] reports, for callers that branch on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// A file-system call failed.
    Io,
    /// A path, file or directory failed an ownership, type or permission check.
    Unsafe,
    /// A stored document or a caller's value is malformed.
    Invalid,
    /// The message identifier already exists in some phase of the queue.
    Duplicate,
    /// The caller's waiter asked to stop before a lock was taken.
    Cancelled,
    /// A process could not be observed coherently, or is gone.
    Process,
}

/// A failure with a human-readable message.
///
/// The message is the whole diagnostic: it names the action and the path, so a caller can show
/// it as it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    /// A failure of `kind` described by `message`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A malformed document or value.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }

    /// A path that failed a safety check.
    pub fn unsafe_path(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsafe, message)
    }

    /// A process that could not be observed.
    pub fn process(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Process, message)
    }

    /// The failure's kind.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The failure's message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Result type of this library.
pub type Result<T> = std::result::Result<T, Error>;

/// `cannot {action} {path}: {error}`, the message every file-system failure uses.
pub fn io_error(action: &str, path: &Path, error: io::Error) -> Error {
    Error::new(
        ErrorKind::Io,
        format!("cannot {action} {}: {error}", path.display()),
    )
}
