//! JSON helpers shared by the archive readers: the Python
//! edition's `as_object`/`as_array`/`as_string`/`as_int`
//! validators (same refusal sentences), its `json.dumps(...,
//! indent=2, sort_keys=True)` output shape, and Python `repr`
//! for the values refusal sentences quote.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Require one JSON object.
pub fn as_object<'a>(
    value: &'a Value,
    where_: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{where_}: expected an object"))
}

/// Require one JSON array.
pub fn as_array<'a>(value: &'a Value, where_: &str) -> Result<&'a Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{where_}: expected an array"))
}

/// Require one JSON string.
pub fn as_string<'a>(value: &'a Value, where_: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{where_}: expected a string"))
}

/// Require one JSON integer (booleans are not integers).
pub fn as_int(value: &Value, where_: &str) -> Result<i64, String> {
    match value {
        Value::Bool(_) => Err(format!("{where_}: expected an integer")),
        _ => value
            .as_i64()
            .ok_or_else(|| format!("{where_}: expected an integer")),
    }
}

/// One integer or `null`.
pub fn optional_int(value: &Value, where_: &str) -> Result<Option<i64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    as_int(value, where_).map(Some)
}

/// Read and parse a JSON document at `path`, mirroring the
/// Python `read_json`. OS failures render in CPython's
/// `[Errno N] ...: 'path'` shape.
pub fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| os_error(&error, path))?;
    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    parse_json_lenient(&text)
}

/// Parse JSON the way CPython's `json` module does about the
/// non-standard constants: `NaN`, `Infinity`, and `-Infinity`
/// are accepted (as floats CPython's validators then refuse
/// where an integer is required). They are rewritten to
/// `null` outside string literals, which preserves every
/// downstream verdict: a field that must be an integer still
/// refuses, and a field nothing reads stays unread.
pub fn parse_json_lenient(text: &str) -> Result<Value, String> {
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(first) => {
            serde_json::from_str(&replace_json_constants(text)).map_err(|_| first.to_string())
        }
    }
}

fn replace_json_constants(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        // Copy one whole UTF-8 character, not one byte.
        let width = utf8_width(byte);
        if in_string {
            out.push_str(&text[index..index + width]);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += width;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push('"');
            index += 1;
            continue;
        }
        if text[index..].starts_with("-Infinity") {
            out.push_str("null");
            index += 9;
            continue;
        }
        if text[index..].starts_with("Infinity") {
            out.push_str("null");
            index += 8;
            continue;
        }
        if text[index..].starts_with("NaN") {
            out.push_str("null");
            index += 3;
            continue;
        }
        out.push_str(&text[index..index + width]);
        index += width;
    }
    out
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// `[Errno N] <strerror>: '<path>'`, CPython's OSError spelling.
pub fn os_error(error: &std::io::Error, path: &Path) -> String {
    let message = error.to_string();
    let message = message
        .strip_suffix(&format!(
            " (os error {})",
            error.raw_os_error().unwrap_or(0)
        ))
        .unwrap_or(&message);
    match error.raw_os_error() {
        Some(errno) => format!("[Errno {errno}] {message}: '{}'", path.display()),
        None => format!("{message}: '{}'", path.display()),
    }
}

/// Python `repr` of a JSON value, for refusal sentences that
/// quote a found value (`{found!r}`).
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Number(number) => py_number_str(number),
        Value::String(text) => py_repr_str(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}: {}", py_repr_str(key), py_repr(item)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn py_nonprintable(ch: char) -> bool {
    let code = ch as u32;
    matches!(code, 0x00..=0x1f | 0x7f..=0xa0)
        || matches!(code, 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff | 0xfff9..=0xfffb)
        || (0xd800..=0xdfff).contains(&code)
}

/// Python `repr` of a string: single quotes unless the string
/// holds a single quote and no double quote, escaped the way
/// CPython escapes them.
pub fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        if ch == quote {
            out.push('\\');
            out.push(ch);
            continue;
        }
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if py_nonprintable(ch) => {
                let code = ch as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
            ch => out.push(ch),
        }
    }
    out.push(quote);
    out
}

/// CPython `repr` of a float: shortest round-trip digits,
/// exponent form (signed, at least two exponent digits) when
/// the decimal exponent is below -4 or at least 16, fixed form
/// otherwise, with a trailing `.0` on integral fixed values.
pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_string();
    }
    // Rust's LowerExp prints the same shortest digits CPython
    // chooses, in `d.dddde<exp>` shape; re-render them under
    // CPython's fixed-vs-exponent rule.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|ch| *ch != '.').collect();
    let sign = if value < 0.0 { "-" } else { "" };
    if !(-4..16).contains(&exponent) {
        let fraction = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        return format!(
            "{sign}{}{fraction}e{}{:02}",
            &digits[..1],
            if exponent < 0 { "-" } else { "+" },
            exponent.abs()
        );
    }
    if exponent >= 0 {
        let width = (exponent + 1) as usize;
        let (integer, fraction) = if digits.len() > width {
            (digits[..width].to_string(), digits[width..].to_string())
        } else {
            (format!("{digits:0<width$}"), String::new())
        };
        if fraction.is_empty() {
            return format!("{sign}{integer}.0");
        }
        return format!("{sign}{integer}.{fraction}");
    }
    format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
}

/// How Python prints one JSON number: floats through
/// `py_float_repr`, integers as their exact literal digits
/// (arbitrary precision is preserved by the parser).
pub fn py_number_str(number: &serde_json::Number) -> String {
    let raw = number.to_string();
    if raw.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        if let Some(float) = number.as_f64() {
            return py_float_repr(float);
        }
    }
    raw
}

/// `json.dumps(value, indent=2, sort_keys=True)`: two-space
/// indent, keys sorted, ASCII-only string escapes.
pub fn dump_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out, true);
    out
}

/// `json.dumps(value, ensure_ascii=False, indent=2,
/// sort_keys=True)`: the query envelope's shape.
pub fn dump_canonical(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out, false);
    out.push('\n');
    out
}

/// `json.dumps(value, ensure_ascii=False, sort_keys=True)`:
/// one line, `, ` and `: ` separators.
pub fn dump_oneline(value: &Value) -> String {
    let mut out = String::new();
    write_oneline(value, &mut out);
    out
}

fn write_oneline(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&py_number_str(number)),
        Value::String(text) => write_json_string_as(text, out, false),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_oneline(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_json_string_as(key, out, false);
                out.push_str(": ");
                write_oneline(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn write_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_value(value: &Value, depth: usize, out: &mut String, ascii_only: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&py_number_str(number)),
        Value::String(text) => write_json_string_as(text, out, ascii_only),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                write_indent(out, depth + 1);
                write_value(item, depth + 1, out, ascii_only);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push_str("{\n");
            for (index, key) in keys.iter().enumerate() {
                write_indent(out, depth + 1);
                write_json_string_as(key, out, ascii_only);
                out.push_str(": ");
                write_value(&map[*key], depth + 1, out, ascii_only);
                if index + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, depth);
            out.push('}');
        }
    }
}

/// A JSON string literal; with `ascii_only`, escaped the way
/// CPython's `ensure_ascii=True` escapes it, otherwise the way
/// `ensure_ascii=False` leaves non-ASCII text in place.
fn write_json_string_as(text: &str, out: &mut String, ascii_only: bool) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 || (ascii_only && (ch as u32) == 0x7f) => {
                out.push_str(&format!("\\u{:04x}", ch as u32))
            }
            ch if !ascii_only && (ch as u32) >= 0x80 => out.push(ch),
            ch if (ch as u32) < 0x80 => out.push(ch),
            ch => {
                let code = ch as u32;
                if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let rest = code - 0x10000;
                    out.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (rest >> 10),
                        0xdc00 + (rest & 0x3ff)
                    ));
                }
            }
        }
    }
    out.push('"');
}

/// Resolve a path the way Python's `Path.resolve()` does:
/// absolute, with symlinks resolved for the deepest existing
/// prefix and the remainder appended lexically normalized.
pub fn resolve_path(path: &Path) -> std::path::PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| Path::new("/").to_path_buf())
            .join(path)
    };
    let part_of = |value: &std::ffi::OsStr| value.to_os_string();
    let mut current: Vec<std::ffi::OsString> = absolute
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(part_of(name)),
            std::path::Component::ParentDir => Some(std::ffi::OsString::from("..")),
            _ => None,
        })
        .collect();
    // Component walk substituting symlinks as they are met --
    // including broken links, whose targets Python resolves
    // lexically -- the way Path.resolve() does it.
    let mut resolved = PathBuf::from("/");
    let mut index = 0usize;
    let mut substituted = 0u32;
    while index < current.len() {
        let name = current[index].clone();
        if name == ".." {
            resolved.pop();
            index += 1;
            continue;
        }
        resolved.push(&name);
        let is_link =
            std::fs::symlink_metadata(&resolved).is_ok_and(|meta| meta.file_type().is_symlink());
        if is_link && substituted <= 40 {
            if let Ok(target) = std::fs::read_link(&resolved) {
                substituted += 1;
                let mut joined: Vec<std::ffi::OsString> = Vec::new();
                if !target.is_absolute() {
                    if let Some(parent) = resolved.parent() {
                        joined.extend(parent.components().filter_map(|part| match part {
                            std::path::Component::Normal(name) => Some(part_of(name)),
                            _ => None,
                        }));
                    }
                }
                joined.extend(target.components().filter_map(|part| match part {
                    std::path::Component::Normal(name) => Some(part_of(name)),
                    std::path::Component::ParentDir => Some(std::ffi::OsString::from("..")),
                    _ => None,
                }));
                joined.extend(current.iter().skip(index + 1).cloned());
                current = joined;
                resolved = PathBuf::from("/");
                index = 0;
                continue;
            }
        }
        index += 1;
    }
    normalize(&resolved)
}

fn normalize(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
