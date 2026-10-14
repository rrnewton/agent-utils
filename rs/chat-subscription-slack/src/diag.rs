//! Bounded operator diagnostics.
//!
//! The bridge discards a plugin's stderr, so both executables can append diagnostics to an
//! operator-chosen private file. Lines never contain message text, tokens or credentials: callers
//! log identifiers, exit statuses and bounded error codes only. The file is rotated to `<path>.1`
//! once it exceeds 1 MiB, so at most about 2 MiB are kept.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_LINE_BYTES: usize = 2_000;

/// A diagnostics sink; without a path, lines go to stderr.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    path: Option<PathBuf>,
    label: &'static str,
}

impl Diagnostics {
    /// Create a sink labelled with the emitting executable's role.
    #[must_use]
    pub fn new(path: Option<PathBuf>, label: &'static str) -> Self {
        Self { path, label }
    }

    /// Append one bounded line. Failures to write diagnostics are ignored.
    pub fn log(&self, message: &str) {
        let mut line = format!("{} {}: {}", unix_millis(), self.label, message);
        if line.len() > MAX_LINE_BYTES {
            let mut boundary = MAX_LINE_BYTES;
            while !line.is_char_boundary(boundary) {
                boundary -= 1;
            }
            line.truncate(boundary);
        }
        line = line.replace(['\n', '\r'], " ");
        match &self.path {
            None => eprintln!("{line}"),
            Some(path) => {
                if fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
                    let mut rotated = path.clone().into_os_string();
                    rotated.push(".1");
                    let _ = fs::rename(path, rotated);
                }
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(path)
                {
                    let _ = writeln!(file, "{line}");
                }
            }
        }
    }
}

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
