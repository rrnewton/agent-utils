//! Byte-safe field lookup in Linux `/proc/PID/status` records.

/// Find the one value of the field `name` in a `/proc/PID/status` record, trimmed of spaces
/// and tabs, or `None` when the record has no such field.
///
/// Linux terminates status records with literal LF bytes. Splitting on any broader Unicode or
/// ASCII line-boundary set would let a carriage return, vertical tab, or form feed inside `Name:`
/// manufacture a field. Requiring uniqueness also rejects a literal newline plus a forged field
/// inside `Name:` because the genuine kernel field still appears later in the record.
pub fn status_field<'a>(record: &'a [u8], name: &[u8]) -> Result<Option<&'a [u8]>, &'static str> {
    if name.is_empty()
        || !name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err("invalid status field name");
    }
    let mut found = None;
    for line in record.split(|byte| *byte == b'\n') {
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let Some(value) = rest.strip_prefix(b":") else {
            continue;
        };
        if found.is_some() {
            return Err("duplicate status field");
        }
        let start = value
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t'))
            .unwrap_or(value.len());
        let end = value
            .iter()
            .rposition(|byte| !matches!(byte, b' ' | b'\t'))
            .map_or(start, |index| index + 1);
        found = Some(&value[start..end]);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_fields_use_literal_lf_and_reject_name_injection() {
        let ordinary = b"Name:\topaque\xff\rCpus_allowed_list:\t99\x0bCpus_allowed_list:\t98\x0cstill-name\nState:\tS\nCpus_allowed_list:\t0-3,8\n";
        assert_eq!(
            status_field(ordinary, b"Cpus_allowed_list").unwrap(),
            Some(b"0-3,8".as_slice())
        );

        let forged =
            b"Name:\topaque\xff\nCpus_allowed_list:\t99\nState:\tS\nCpus_allowed_list:\t0-3,8\n";
        assert_eq!(
            status_field(forged, b"Cpus_allowed_list").unwrap_err(),
            "duplicate status field"
        );
        assert_eq!(status_field(ordinary, b"NSpid").unwrap(), None);
    }
}
