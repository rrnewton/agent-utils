//! Bounded parsing for Linux `/proc/PID/stat` process identities.
//!
//! The kernel's parenthesized `comm` field is opaque bytes, not UTF-8. Only
//! the ASCII fields following its final closing parenthesis are decoded here.
//! Read/validate parity with `py/agentctl/procstat.py`.

/// Index of `starttime` (field 22) among the fields after `comm` (field 3).
const STARTTIME_OFFSET: usize = 22 - 3;
const MAX_PROCESS_ID: i64 = (1 << 31) - 1;
const PROCESS_STATES: &[&[u8]] = &[
    b"R", b"S", b"D", b"Z", b"T", b"t", b"X", b"x", b"K", b"W", b"P", b"I",
];

/// Validated process identity fields from one Linux procfs record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStat {
    /// OS process id.
    pub pid: i64,
    /// Kernel start ticks (`starttime`, field 22).
    pub starttime: u64,
    /// Parent process id.
    pub ppid: i64,
    /// Process-group id.
    pub pgrp: i64,
    /// Session id.
    pub session: i64,
    /// One-letter process state.
    pub state: String,
}

/// Return validated process identity fields, or `None` for malformed input.
pub fn parse_process_stat(record: &[u8]) -> Option<ProcessStat> {
    let opening = record.windows(2).position(|window| window == b" (")?;
    let closing = record.iter().rposition(|byte| *byte == b')')?;
    if opening == 0 || closing <= opening + 1 || record.get(closing + 1) != Some(&b' ') {
        return None;
    }
    let fields: Vec<&[u8]> = record[closing + 2..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    if fields.len() <= STARTTIME_OFFSET || !PROCESS_STATES.contains(&fields[0]) {
        return None;
    }
    let pid = parse_i64(&record[..opening])?;
    let state = std::str::from_utf8(fields[0]).ok()?.to_owned();
    let ppid = parse_i64(fields[1])?;
    let pgrp = parse_i64(fields[2])?;
    let session = parse_i64(fields[3])?;
    let starttime = parse_u64(fields[STARTTIME_OFFSET])?;
    if !(1..=MAX_PROCESS_ID).contains(&pid)
        || !(0..=MAX_PROCESS_ID).contains(&ppid)
        || !(0..=MAX_PROCESS_ID).contains(&pgrp)
        || !(0..=MAX_PROCESS_ID).contains(&session)
    {
        return None;
    }
    Some(ProcessStat {
        pid,
        starttime,
        ppid,
        pgrp,
        session,
        state,
    })
}

fn parse_i64(field: &[u8]) -> Option<i64> {
    std::str::from_utf8(field).ok()?.parse::<i64>().ok()
}

fn parse_u64(field: &[u8]) -> Option<u64> {
    std::str::from_utf8(field).ok()?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fields 4..=52 after `comm`: state ppid pgrp session, then filler up to
    // starttime at offset 19, then trailing fields.
    fn record(pid: &str, comm: &[u8], state: &str, starttime: &str) -> Vec<u8> {
        let mut tail: Vec<String> = vec![
            state.to_owned(),
            "10".to_owned(),
            "20".to_owned(),
            "20".to_owned(),
        ];
        tail.extend(std::iter::repeat_n("0".to_owned(), 15));
        tail.push(starttime.to_owned());
        tail.extend(std::iter::repeat_n("0".to_owned(), 30));
        let mut raw = format!("{pid} (").into_bytes();
        raw.extend_from_slice(comm);
        raw.extend_from_slice(format!(") {}\n", tail.join(" ")).as_bytes());
        raw
    }

    #[test]
    fn parses_a_kernel_record() {
        let stat = parse_process_stat(&record("4242", b"my proc", "S", "987654")).unwrap();
        assert_eq!(stat.pid, 4242);
        assert_eq!(stat.state, "S");
        assert_eq!(stat.ppid, 10);
        assert_eq!(stat.pgrp, 20);
        assert_eq!(stat.session, 20);
        assert_eq!(stat.starttime, 987654);
    }

    #[test]
    fn comm_is_opaque_bytes_up_to_the_last_close_paren() {
        // Embedded `)`, spaces, and non-UTF-8 bytes inside comm.
        let stat = parse_process_stat(&record("7", b"a) b \xff\xfe c", "R", "42")).unwrap();
        assert_eq!(stat.pid, 7);
        assert_eq!(stat.starttime, 42);
    }

    #[test]
    fn accepts_every_linux_state_and_no_other() {
        for state in ["R", "S", "D", "Z", "T", "t", "X", "x", "K", "W", "P", "I"] {
            assert!(
                parse_process_stat(&record("7", b"x", state, "1")).is_some(),
                "state {state}"
            );
        }
        assert!(parse_process_stat(&record("7", b"x", "Q", "1")).is_none());
        assert!(parse_process_stat(&record("7", b"x", "SS", "1")).is_none());
    }

    #[test]
    fn rejects_malformed_records() {
        for raw in [
            b"".as_slice(),
            b"no parens at all",
            b"7 () S 0 0 0",
            b"7 (x)S 0 0 0",
            b" (x) S 0 0 0",
            b"7 (x) S 10 20 20",
        ] {
            assert!(parse_process_stat(raw).is_none(), "record: {raw:?}");
        }
    }

    #[test]
    fn rejects_out_of_range_numbers() {
        assert!(parse_process_stat(&record("0", b"x", "S", "1")).is_none());
        assert!(parse_process_stat(&record("-1", b"x", "S", "1")).is_none());
        assert!(parse_process_stat(&record("2147483648", b"x", "S", "1")).is_none());
        // Starttime beyond u64, and a negative starttime.
        assert!(parse_process_stat(&record("7", b"x", "S", "18446744073709551616")).is_none());
        assert!(parse_process_stat(&record("7", b"x", "S", "-1")).is_none());
        // Maxima are accepted.
        let stat =
            parse_process_stat(&record("2147483647", b"x", "S", "18446744073709551615")).unwrap();
        assert_eq!(stat.pid, 2147483647);
        assert_eq!(stat.starttime, u64::MAX);
    }
}
