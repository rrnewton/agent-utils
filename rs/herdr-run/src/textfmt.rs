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

/// The six ASCII whitespace characters POSIX `isspace` names, trimmed from an exit-code marker.
pub const ASCII_WHITESPACE: &[char] = &[' ', '\t', '\n', '\r', '\u{b}', '\u{c}'];

/// The Unicode `White_Space` property, written out so it never follows a compiler's tables.
pub const UNICODE_WHITESPACE: &[char] = &[
    '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{85}', '\u{a0}', '\u{1680}', '\u{2000}',
    '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}',
    '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
];

/// Unicode general category `Cf` (format characters), written out as of Unicode 15.0: the soft
/// hyphen, zero-width spaces and joiners, bidirectional marks, embeddings, overrides and isolates,
/// the byte-order mark, interlinear annotation, tag characters, and the rest. None of them draws
/// anything, so a name that contains one looks exactly like a name that does not.
pub const FORMAT_CHARACTER_RANGES: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// Longest completion marker accepted.
pub const MAX_EXIT_MARKER_BYTES: usize = 128;

/// True for a character in [`FORMAT_CHARACTER_RANGES`].
#[must_use]
pub fn is_format_character(character: char) -> bool {
    let code = u32::from(character);
    FORMAT_CHARACTER_RANGES
        .iter()
        .any(|&(low, high)| (low..=high).contains(&code))
}

/// True for a format character or a non-ASCII `White_Space` character.
///
/// These are the characters a reader cannot see: a no-break space looks like a space, a line
/// separator like a line break or nothing, a zero-width space or a byte-order mark like nothing at
/// all, and a bidirectional override reorders what follows it.
#[must_use]
pub fn is_invisible(character: char) -> bool {
    is_format_character(character)
        || (!character.is_ascii() && UNICODE_WHITESPACE.contains(&character))
}

/// Describe the first character a name may not contain, or return `None`.
///
/// A name -- an agent, a workspace, an allowed program, a denied option -- is compared with what
/// somebody typed and shown to somebody reading, so it may contain no terminal control (C0, DEL,
/// C1) and no invisible character ([`is_invisible`]). (A Rust string cannot hold a surrogate.)
#[must_use]
pub fn name_violation(value: &str) -> Option<String> {
    value.chars().find_map(|character| {
        let code = u32::from(character);
        if matches!(code, 0x00..=0x1f | 0x7f..=0x9f) {
            Some(format!("control character U+{code:04X}"))
        } else if is_invisible(character) {
            Some(format!("invisible character U+{code:04X}"))
        } else {
            None
        }
    })
}

/// Show `value` in a message as it is, or [`quote`] it when it holds an unsafe character.
///
/// A path or a name made of ordinary characters reads best unquoted; one that holds a terminal
/// control or an invisible character is quoted, so nothing can reach a terminal raw from a
/// herdr-run message.
#[must_use]
pub fn display(value: &str) -> String {
    if name_violation(value).is_none() {
        value.to_owned()
    } else {
        quote(value)
    }
}

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
/// round-trip decimal ([`shortest_digits`]), never in exponent notation.
#[must_use]
pub fn number_text(value: f64) -> String {
    if !value.is_finite() {
        return python_float_repr(value);
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    let (negative, digits, exponent) = shortest_digits(value);
    let sign = if negative { "-" } else { "" };
    let point = exponent + 1;
    if point <= 0 {
        let zeros = "0".repeat(usize::try_from(-point).unwrap_or(0));
        return format!("{sign}0.{zeros}{digits}");
    }
    let point = usize::try_from(point).unwrap_or(0);
    if digits.len() <= point {
        // Unreachable for a value with a fractional part, but written out rather than assumed.
        return format!("{sign}{digits}{}", "0".repeat(point - digits.len()));
    }
    format!("{sign}{}.{}", &digits[..point], &digits[point..])
}

/// The shortest decimal digits that read back as `value`, and the decimal exponent of the first.
///
/// Returns `(negative, digits, exponent)` with `value == ±0.d1d2d3… × 10^(exponent + 1)`. When
/// several digit strings of the shortest length read back as `value`, the one nearest to it is
/// chosen, and an exact tie goes to the even last digit: the rule Python's `repr` follows, so
/// `2**-25` is `2.9802322387695312e-08` in both editions. The standard library's `{}` and `{:e}`
/// break that tie upwards instead, which is why they are not used here. `value` must be finite.
#[must_use]
pub fn shortest_digits(value: f64) -> (bool, String, i32) {
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format_finite(value);
    let (negative, text) = text
        .strip_prefix('-')
        .map_or((false, text), |rest| (true, rest));
    let (mantissa, exponent) = text.split_once(['e', 'E']).map_or((text, 0), |(m, e)| {
        (
            m,
            e.parse::<i32>().expect("ryu writes an integral exponent"),
        )
    });
    let (integral, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = format!("{integral}{fraction}");
    let mut point = i32::try_from(integral.len()).unwrap_or(i32::MAX) + exponent;
    while digits.len() > 1 && digits.starts_with('0') {
        digits.remove(0);
        point -= 1;
    }
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    if digits == "0" {
        point = 1;
    }
    (negative, digits, point - 1)
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

/// Format a finite `f64` by the float-text rule both editions share.
///
/// Shortest round-trip digits ([`shortest_digits`]); fixed notation when the decimal exponent is
/// in `-4..16`, otherwise `d.ddde+XX`. JSON output uses this so every number prints identically in
/// both editions.
#[must_use]
pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let (negative, digits, exponent) = shortest_digits(value);
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

/// Deepest JSON nesting refused: a document whose arrays and objects nest this many levels deep is
/// not read, so 127 levels is the most accepted. This is `serde_json`'s own recursion limit, and
/// the other edition enforces the same number.
pub const JSON_MAX_DEPTH: usize = 128;

/// Parse RFC 8259 JSON strictly, or return `None`.
///
/// `NaN`/`Infinity` are refused, a float that overflows is refused, an integer outside the 64-bit
/// range becomes the nearest float (and is refused when even that overflows), a lone surrogate
/// escape is refused, and so is nesting [`JSON_MAX_DEPTH`] levels deep. The crate enables
/// `serde_json`'s `float_roundtrip`, so a float reads as the nearest `f64` whatever other crates
/// in a build select, which is what the other edition's reader does.
#[must_use]
pub fn strict_json(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
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

/// Serialize `value` in the JSON text form both editions share: two-space indentation, keys
/// sorted at every level whatever `serde_json` features are enabled, floats as
/// [`python_float_repr`] writes them, and only `"`, `\`, and C0 controls escaped.
#[must_use]
pub fn json_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, Some(2), 0);
    out
}

/// Serialize `value` on one line in the same form, with `, ` and `: ` separators.
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

    /// The text rules shared with the other edition, from the corpus both test suites load.
    const CASES: &str = include_str!("../testdata/text_cases.json");

    fn rows(key: &str) -> Vec<Vec<Value>> {
        let document: Value = serde_json::from_str(CASES).expect("golden text cases");
        serde_json::from_value(document[key].clone()).expect("rows")
    }

    #[test]
    fn golden_quote() {
        for row in rows("quote") {
            assert_eq!(quote(row[0].as_str().unwrap()), row[1], "{row:?}");
        }
    }

    #[test]
    fn golden_number_text() {
        for row in rows("number_text") {
            assert_eq!(number_text(row[0].as_f64().unwrap()), row[1], "{row:?}");
        }
    }

    #[test]
    fn golden_display() {
        for row in rows("display") {
            assert_eq!(display(row[0].as_str().unwrap()), row[1], "{row:?}");
        }
    }

    #[test]
    fn golden_name_violation() {
        for row in rows("name_violation") {
            let expected = row[1].as_str().map(str::to_owned);
            assert_eq!(
                name_violation(row[0].as_str().unwrap()),
                expected,
                "{row:?}"
            );
        }
    }

    /// Every JSON document in the corpus, read and written back in the shared line form, or
    /// refused (`null`).
    #[test]
    fn golden_strict_json() {
        for row in rows("json") {
            let text = row[0].as_str().unwrap();
            let read = strict_json(text).map(|value| json_line(&value));
            assert_eq!(read.as_deref(), row[1].as_str(), "{text:?}");
        }
        // The deepest nesting accepted and the first refused, built here rather than spelled out.
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(strict_json(&nested(JSON_MAX_DEPTH - 1)).is_some());
        assert!(strict_json(&nested(JSON_MAX_DEPTH)).is_none());
        let objects = |depth: usize| format!("{}1{}", "{\"a\": ".repeat(depth), "}".repeat(depth));
        assert!(strict_json(&objects(JSON_MAX_DEPTH - 1)).is_some());
        assert!(strict_json(&objects(JSON_MAX_DEPTH)).is_none());
    }

    #[test]
    fn shortest_digits_break_ties_to_even() {
        // 2**-25 is exactly ...953125e-8: the two 17-digit neighbours are equally near.
        assert_eq!(python_float_repr(2f64.powi(-25)), "2.9802322387695312e-08");
        assert_eq!(number_text(2f64.powi(-25)), "0.000000029802322387695312");
        assert_eq!(
            python_float_repr(11.565_933_227_539_062),
            "11.565933227539062"
        );
        assert_eq!(shortest_digits(0.0), (false, "0".to_owned(), 0));
        assert_eq!(shortest_digits(-0.0), (true, "0".to_owned(), 0));
        assert_eq!(shortest_digits(1e16), (false, "1".to_owned(), 16));
        assert_eq!(shortest_digits(1.5e-7), (false, "15".to_owned(), -7));
    }

    #[test]
    fn golden_exit_marker() {
        for row in rows("exit_marker") {
            let payload: Vec<u8> = serde_json::from_value(row[0].clone()).unwrap();
            let expected = row[1].as_i64().map(|code| i32::try_from(code).unwrap());
            assert_eq!(parse_exit_marker(&payload), expected, "{row:?}");
        }
    }

    #[test]
    fn golden_float_text() {
        for row in rows("float_repr") {
            assert_eq!(
                python_float_repr(row[0].as_f64().unwrap()),
                row[1],
                "{row:?}"
            );
        }
    }

    #[test]
    fn json_writer_sorts_keys_whatever_the_insertion_order() {
        let mut forward = serde_json::Map::new();
        forward.insert("b".to_owned(), Value::from(1));
        forward.insert("a".to_owned(), serde_json::json!({"z": 0.5, "y": [1.0]}));
        let mut backward = serde_json::Map::new();
        backward.insert("a".to_owned(), serde_json::json!({"y": [1.0], "z": 0.5}));
        backward.insert("b".to_owned(), Value::from(1));
        let forward = Value::Object(forward);
        let backward = Value::Object(backward);
        assert_eq!(json_line(&forward), json_line(&backward));
        assert_eq!(
            json_line(&forward),
            r#"{"a": {"y": [1.0], "z": 0.5}, "b": 1}"#
        );
        assert_eq!(json_pretty(&forward), json_pretty(&backward));
    }
}
