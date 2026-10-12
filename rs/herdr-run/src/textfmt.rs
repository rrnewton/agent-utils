//! Text rules shared by both editions of herdr-run, written out rather than borrowed.
//!
//! Everything here exists because the obvious standard-library spelling gives an answer that
//! depends on the language, or on the Unicode version of the compiler in use: `{:?}` escapes by
//! Unicode table, `str::trim` and `char::is_whitespace` follow `White_Space`, `parse::<f64>`
//! defines its own grammar, and `serde_json` orders keys by a Cargo feature. Each function states
//! its rule explicitly, the other edition implements the identical rule, and the golden cases in
//! `testdata/text_cases.json` hold the two to it.

use std::io;

use serde_json::Value;

/// The six ASCII whitespace characters an environment value is trimmed of.
pub const ASCII_WHITESPACE: &[char] = &[' ', '\t', '\n', '\r', '\u{b}', '\u{c}'];

/// The Unicode `White_Space` property, written out so it never follows a compiler's tables.
pub const UNICODE_WHITESPACE: &[char] = &[
    '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{85}', '\u{a0}', '\u{1680}', '\u{2000}',
    '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}',
    '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
];

/// Longest completion marker accepted.
pub const MAX_EXIT_MARKER_BYTES: usize = 128;

/// Quote `value` for a diagnostic, escaping everything that is not printable ASCII.
///
/// Printable ASCII (U+0020..U+007E) passes through except `"` and `\`, which are escaped; newline,
/// carriage return, and tab use their short escapes; every other code point is written as
/// `\u{hex}`. No Unicode table is consulted.
#[must_use]
pub fn quote(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(character),
            other => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(other));
            }
        }
    }
    out.push('"');
    out
}

/// [`quote`] a value that may be absent; an absent value reads `None`.
#[must_use]
pub fn quote_optional(value: Option<&str>) -> String {
    value.map_or_else(|| "None".to_owned(), quote)
}

/// Describe an I/O error as `<strerror> (os error <errno>)`, the standard library's own form.
#[must_use]
pub fn os_error_text(error: &io::Error) -> String {
    error.to_string()
}

/// Trim the six ASCII whitespace characters from both ends, and nothing else.
#[must_use]
pub fn strip_ascii(value: &str) -> &str {
    value.trim_matches(ASCII_WHITESPACE)
}

/// Trim Unicode `White_Space` (the explicit list above) from both ends.
#[must_use]
pub fn strip_space(value: &str) -> &str {
    value.trim_matches(UNICODE_WHITESPACE)
}

/// Trim Unicode `White_Space` (the explicit list above) from the end.
#[must_use]
pub fn rstrip_space(value: &str) -> &str {
    value.trim_end_matches(UNICODE_WHITESPACE)
}

/// True when `value` is empty or holds only Unicode `White_Space`.
#[must_use]
pub fn is_blank(value: &str) -> bool {
    value
        .chars()
        .all(|character| UNICODE_WHITESPACE.contains(&character))
}

/// Render seconds for a message: integral values with no decimals, otherwise the shortest
/// round-trip decimal, never in exponent notation.
#[must_use]
pub fn number_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// Round to three decimal places by correctly rounded decimal formatting.
#[must_use]
pub fn round3(value: f64) -> f64 {
    format!("{value:.3}").parse().unwrap_or(value)
}

/// True when `value` is a whole number of milliseconds.
#[must_use]
pub fn whole_milliseconds(value: f64) -> bool {
    (value * 1000.0).round() / 1000.0 == value
}

/// Parse a run's `exit_code` completion marker, or return `None` when it is not one.
///
/// ASCII bytes only, at most 128 of them, ASCII whitespace trimmed, then `[+-]?[0-9]+` within the
/// signed 32-bit range.
#[must_use]
pub fn parse_exit_marker(payload: &[u8]) -> Option<i32> {
    if payload.len() > MAX_EXIT_MARKER_BYTES || !payload.is_ascii() {
        return None;
    }
    let text = strip_ascii(std::str::from_utf8(payload).ok()?);
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Leading zeros are allowed; parse through i64 so a long run of zeros cannot overflow first.
    let trimmed = digits.trim_start_matches('0');
    if trimmed.len() > 10 {
        return None;
    }
    let magnitude: i64 = if trimmed.is_empty() {
        0
    } else {
        trimmed.parse().ok()?
    };
    let value = if text.starts_with('-') {
        -magnitude
    } else {
        magnitude
    };
    i32::try_from(value).ok()
}

/// Format a finite `f64` exactly as Python's `repr(float)` does.
///
/// Shortest round-trip digits; fixed notation when the decimal exponent is in `-4..16`, otherwise
/// `d.ddde+XX`. JSON output uses this so every number prints identically in both editions.
#[must_use]
pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust {:e} always has an exponent");
    let exponent: i32 = exponent.parse().expect("integral exponent");
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let text = if point <= 0 {
            format!(
                "0.{}{digits}",
                "0".repeat(usize::try_from(-point).unwrap_or(0))
            )
        } else {
            let point = usize::try_from(point).unwrap_or(0);
            if digits.len() <= point {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        return format!("{sign}{text}");
    }
    let (first, rest) = digits.split_at(1);
    let mantissa = if rest.is_empty() {
        first.to_owned()
    } else {
        format!("{first}.{rest}")
    };
    let exponent_sign = if exponent < 0 { '-' } else { '+' };
    format!(
        "{sign}{mantissa}e{exponent_sign}{:02}",
        exponent.unsigned_abs()
    )
}

fn write_json_string(out: &mut String, value: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn write_json_number(out: &mut String, number: &serde_json::Number) {
    if let Some(integer) = number.as_i64() {
        out.push_str(&integer.to_string());
    } else if let Some(integer) = number.as_u64() {
        out.push_str(&integer.to_string());
    } else {
        out.push_str(&python_float_repr(number.as_f64().unwrap_or(f64::NAN)));
    }
}

fn sorted_entries(map: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
}

fn write_json(out: &mut String, value: &Value, indent: Option<usize>, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => write_json_number(out, number),
        Value::String(text) => write_json_string(out, text),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                if let Some(width) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(width * (level + 1)));
                }
                write_json(out, item, indent, level + 1);
            }
            if let Some(width) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(width * level));
            }
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in sorted_entries(map).into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                if let Some(width) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(width * (level + 1)));
                }
                write_json_string(out, key);
                out.push_str(": ");
                write_json(out, item, indent, level + 1);
            }
            if let Some(width) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(width * level));
            }
            out.push('}');
        }
    }
}

/// Serialize `value` the way `json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False)`
/// does: keys sorted at every level whatever `serde_json` features are enabled, floats in Python
/// `repr` form, and only `"`, `\`, and C0 controls escaped.
#[must_use]
pub fn json_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, Some(2), 0);
    out
}

/// Serialize `value` on one line, as `json.dumps(value, sort_keys=True, ensure_ascii=False)`.
#[must_use]
pub fn json_line(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, None, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes_everything_but_printable_ascii() {
        assert_eq!(quote("5\u{202e}"), "\"5\\u{202e}\"");
        assert_eq!(
            quote("a\"b\\c\n\t\u{1}é"),
            "\"a\\\"b\\\\c\\n\\t\\u{1}\\u{e9}\""
        );
    }

    #[test]
    fn python_float_repr_matches_python() {
        for (value, expected) in [
            (900.0, "900.0"),
            (1e-7, "1e-07"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.1, "0.1"),
            (12.345, "12.345"),
            (31_536_000.0, "31536000.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-0.0, "-0.0"),
            (1.844_674_407_370_955_2e19, "1.8446744073709552e+19"),
            (5e-324, "5e-324"),
            (1e100, "1e+100"),
        ] {
            assert_eq!(python_float_repr(value), expected, "{value}");
        }
    }

    #[test]
    fn exit_markers_are_ascii_signed_32_bit_integers() {
        assert_eq!(parse_exit_marker(b" 0\n"), Some(0));
        assert_eq!(parse_exit_marker(b"+5"), Some(5));
        assert_eq!(parse_exit_marker(b"-2147483648"), Some(i32::MIN));
        assert_eq!(parse_exit_marker(b"2147483648"), None);
        assert_eq!(parse_exit_marker(b"1_0"), None);
        assert_eq!(parse_exit_marker(b"0\x1c"), None);
        assert_eq!(parse_exit_marker("0\u{a0}".as_bytes()), None);
        assert_eq!(parse_exit_marker(b"0000000000000007"), Some(7));
    }
}
