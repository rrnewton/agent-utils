//! Byte-safe parsing for Linux `/proc/PID/stat` process identity records.
//!
//! The parenthesized `comm` field is opaque kernel data. It may contain whitespace,
//! parentheses, newlines, and bytes that are not UTF-8, so callers must locate its final closing
//! parenthesis before interpreting the stable ASCII tail.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
pub(crate) use testrun_core::procstat::status_field;

const UTIME_INDEX: usize = 14 - 3;
const STIME_INDEX: usize = 15 - 3;
const CUTIME_INDEX: usize = 16 - 3;
const CSTIME_INDEX: usize = 17 - 3;
const STARTTIME_INDEX: usize = 22 - 3;

/// Every state byte Linux has exposed in field 3 of `/proc/PID/stat`.
///
/// `W`, `x`, and `K` are retained for kernels on which those states can still be
/// observed. Accepting arbitrary ASCII here is unsafe: a stray closing parenthesis can otherwise
/// move a numeric field into the state slot and make a malformed record look structurally valid.
fn linux_process_state(state: u8) -> bool {
    matches!(
        state,
        b'R' | b'S' | b'D' | b'Z' | b'T' | b't' | b'X' | b'x' | b'K' | b'W' | b'P' | b'I'
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcStat {
    pub(crate) pid: u32,
    pub(crate) state: u8,
    pub(crate) ppid: u32,
    pub(crate) pgrp: u32,
    pub(crate) utime_ticks: u64,
    pub(crate) stime_ticks: u64,
    pub(crate) child_utime_ticks: i64,
    pub(crate) child_stime_ticks: i64,
    pub(crate) starttime_ticks: u64,
}

#[derive(Debug)]
pub(crate) enum ProcStatError {
    Read { path: PathBuf, source: io::Error },
    Malformed { path: PathBuf, detail: &'static str },
}

impl fmt::Display for ProcStatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "read process identity {}: {source}",
                    path.display()
                )
            }
            Self::Malformed { path, detail } => {
                write!(
                    formatter,
                    "process identity {} is malformed: {detail}",
                    path.display()
                )
            }
        }
    }
}

fn parse_u32(token: &[u8]) -> Option<u32> {
    std::str::from_utf8(token).ok()?.parse().ok()
}

fn parse_u64(token: &[u8]) -> Option<u64> {
    std::str::from_utf8(token).ok()?.parse().ok()
}

fn parse_i64(token: &[u8]) -> Option<i64> {
    std::str::from_utf8(token).ok()?.parse().ok()
}

/// Parse one stat record without ever decoding its opaque command field.
pub(crate) fn parse(record: &[u8], expected_pid: Option<u32>) -> Result<ProcStat, &'static str> {
    let opening = record
        .windows(2)
        .position(|window| window == b" (")
        .ok_or("missing command opener")?;
    let closing = record
        .iter()
        .rposition(|byte| *byte == b')')
        .filter(|closing| *closing > opening + 1)
        .ok_or("missing command terminator")?;
    let state = match record.get(closing + 1..closing + 4) {
        Some([b' ', state, b' ']) if linux_process_state(*state) => *state,
        Some([b' ', _, b' ']) => return Err("invalid process state"),
        _ => return Err("command terminator is not followed by a valid state field"),
    };

    let pid = parse_u32(&record[..opening]).ok_or("invalid pid")?;
    if pid == 0 || pid > i32::MAX as u32 {
        return Err("pid is outside the Linux process-id range");
    }
    if expected_pid.is_some_and(|expected| expected != pid) {
        return Err("pid does not match the procfs path");
    }

    let fields = record[closing + 2..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() <= STARTTIME_INDEX {
        return Err("record has no starttime field");
    }

    let ppid = parse_u32(fields[1]).ok_or("invalid parent pid")?;
    let pgrp = parse_u32(fields[2]).ok_or("invalid process group")?;
    if ppid > i32::MAX as u32 || pgrp > i32::MAX as u32 {
        return Err("parent pid or process group is outside the Linux process-id range");
    }
    Ok(ProcStat {
        pid,
        state,
        ppid,
        pgrp,
        utime_ticks: parse_u64(fields[UTIME_INDEX]).ok_or("invalid user CPU time")?,
        stime_ticks: parse_u64(fields[STIME_INDEX]).ok_or("invalid system CPU time")?,
        child_utime_ticks: parse_i64(fields[CUTIME_INDEX]).ok_or("invalid child user CPU time")?,
        child_stime_ticks: parse_i64(fields[CSTIME_INDEX])
            .ok_or("invalid child system CPU time")?,
        starttime_ticks: parse_u64(fields[STARTTIME_INDEX]).ok_or("invalid starttime")?,
    })
}

/// Find exactly one named `/proc/PID/status` field without treating opaque `Name:` bytes as text.
///
fn process_gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH))
}

/// Read one process identity. Only the two kernel "process is gone" errors map to absence.
pub(crate) fn read(pid: u32) -> Result<Option<ProcStat>, ProcStatError> {
    read_at(Path::new(&format!("/proc/{pid}/stat")), pid)
}

fn read_at(path: &Path, expected_pid: u32) -> Result<Option<ProcStat>, ProcStatError> {
    let record = match std::fs::read(path) {
        Ok(record) => record,
        Err(error) if process_gone(&error) => return Ok(None),
        Err(source) => {
            return Err(ProcStatError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    parse(&record, Some(expected_pid))
        .map(Some)
        .map_err(|detail| ProcStatError::Malformed {
            path: path.to_path_buf(),
            detail,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn record(pid: u32, command: &[u8]) -> Vec<u8> {
        let mut value = format!("{pid} (").into_bytes();
        value.extend_from_slice(command);
        value.extend_from_slice(b") S 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0\n");
        value
    }

    #[test]
    fn opaque_command_bytes_cannot_shift_identity_fields() {
        let hostile = b"name\xff) Z 999 888 777\nPPid:\t123\rmore (parens)";
        let parsed = parse(&record(4242, hostile), Some(4242)).unwrap();
        assert_eq!(parsed.pid, 4242);
        assert_eq!(parsed.state, b'S');
        assert_eq!(parsed.ppid, 7);
        assert_eq!(parsed.pgrp, 9);
        assert_eq!(parsed.utime_ticks, 13);
        assert_eq!(parsed.stime_ticks, 14);
        assert_eq!(parsed.child_utime_ticks, 15);
        assert_eq!(parsed.child_stime_ticks, 16);
        assert_eq!(parsed.starttime_ticks, 22);
    }

    #[test]
    fn parser_rejects_identity_mismatch_and_malformed_ascii_tail() {
        assert_eq!(
            parse(&record(42, b"worker"), Some(41)).unwrap_err(),
            "pid does not match the procfs path"
        );
        assert!(parse(b"42 (worker) S 1 nope", Some(42)).is_err());
        assert!(parse(b"42 (worker)\xff S 1 2", Some(42)).is_err());

        // `rposition(')')` deliberately skips opaque `)` bytes in comm, but a malformed trailing
        // delimiter must not turn the first numeric field into a plausible process state.
        let mut shifted = record(42, b"worker");
        shifted.extend_from_slice(b") 7 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22");
        assert_eq!(
            parse(&shifted, Some(42)).unwrap_err(),
            "invalid process state"
        );
    }

    #[test]
    fn parser_accepts_every_linux_process_state_and_no_other_byte() {
        for state in b"RSDZTtXxKWPI" {
            let mut value = record(42, b"worker");
            let closing = value.iter().rposition(|byte| *byte == b')').unwrap();
            value[closing + 2] = *state;
            assert_eq!(parse(&value, Some(42)).unwrap().state, *state);
        }
        for state in b"Q0)\x7f" {
            let mut value = record(42, b"worker");
            let closing = value.iter().rposition(|byte| *byte == b')').unwrap();
            value[closing + 2] = *state;
            assert_eq!(
                parse(&value, Some(42)).unwrap_err(),
                "invalid process state"
            );
        }
    }

    #[test]
    fn read_at_accepts_non_utf8_and_only_missing_process_errors_mean_absent() {
        let mut path = std::env::temp_dir();
        let mut name = format!("dagrun-procstat-{}-invalid-", std::process::id()).into_bytes();
        name.push(0xff);
        path.push(std::ffi::OsString::from_vec(name));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, record(73, b"opaque\xff\n) text")).unwrap();
        assert_eq!(read_at(&path, 73).unwrap().unwrap().ppid, 7);
        std::fs::write(&path, b"malformed").unwrap();
        assert!(matches!(
            read_at(&path, 73),
            Err(ProcStatError::Malformed { .. })
        ));
        std::fs::remove_file(&path).unwrap();
        assert!(read_at(&path, 73).unwrap().is_none());
        assert!(!process_gone(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "opaque"
        )));
        assert!(!process_gone(&io::Error::new(
            io::ErrorKind::InvalidData,
            "opaque"
        )));
    }
}
