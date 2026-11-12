//! Python-compatible JSON values for ledger byte parity.
//!
//! Ledger identity, folding, and digests are defined over Python `json`
//! semantics: objects keep every written pair (duplicates included),
//! integers are arbitrary precision, floats format with Python `repr`,
//! and the canonical dump escapes non-ASCII text. `serde_json::Value`
//! loses each of those, so the core carries its own value type.
//!
//! Strings are held as Unicode scalar text. A lone surrogate escape in
//! stored input is refused at parse time rather than silently replaced:
//! valid ledger text never contains one, and refusing keeps a divergence
//! visible instead of folding different bytes as if they were equal.

use std::cmp::Ordering;
use std::fmt;

/// One JSON value with Python `json` semantics.
#[derive(Clone, Debug, PartialEq)]
pub enum PyValue {
    /// JSON `null` (Python `None`).
    Null,
    /// JSON `true` / `false`.
    Bool(bool),
    /// Arbitrary-precision integer, held as normalized decimal text
    /// (`-0` normalizes to `0`; no leading zeros, as JSON requires).
    Int(String),
    /// IEEE-754 binary64 float, including non-finite values from the
    /// Python-only literals `NaN`, `Infinity`, and `-Infinity`.
    Float(f64),
    /// Unicode text (unescaped).
    Str(String),
    /// JSON array, order preserved.
    Array(Vec<PyValue>),
    /// JSON object as written pairs: order and duplicate keys preserved.
    /// Dict-view lookups take the last value for a key.
    Object(Vec<(String, PyValue)>),
}

impl PyValue {
    /// Dict-view lookup: the last value written for `key`, if any.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        match self {
            PyValue::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Whether `key` is present (dict view).
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// The value as a string slice, when it is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as an object pair list, when it is an object.
    pub fn as_object(&self) -> Option<&Vec<(String, PyValue)>> {
        match self {
            PyValue::Object(pairs) => Some(pairs),
            _ => None,
        }
    }

    /// Whether the value is a JSON object.
    pub fn is_object(&self) -> bool {
        matches!(self, PyValue::Object(_))
    }

    /// Python truthiness for the shapes ledger checks test: `None`,
    /// `False`, numeric zero, and empty string/array/object are falsy.
    pub fn truthy(&self) -> bool {
        match self {
            PyValue::Null => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(text) => text != "0",
            PyValue::Float(f) => *f != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::Array(items) => !items.is_empty(),
            PyValue::Object(pairs) => !pairs.is_empty(),
        }
    }

    /// The integer value when it fits `i64` (booleans never count).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PyValue::Int(text) => text.parse::<i64>().ok(),
            _ => None,
        }
    }

    /// The integer value when it fits `u64` (booleans never count).
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            PyValue::Int(text) => text.parse::<u64>().ok(),
            _ => None,
        }
    }

    /// Whether this is an integer (never a boolean, never a float).
    pub fn is_int(&self) -> bool {
        matches!(self, PyValue::Int(_))
    }
}

/// Why a line could not be parsed as Python-compatible JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Human-readable reason.
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for ParseError {}

fn err(reason: impl Into<String>) -> ParseError {
    ParseError {
        reason: reason.into(),
    }
}

/// Parser options mirroring the two ledger parse modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// Refuse any repeated object key (stored codec lines and evidence rows).
    pub unique_keys: bool,
    /// Refuse non-finite numbers, including overflow such as `1e999`
    /// (evidence rows). Ordinary lines accept Python's literals.
    pub finite_only: bool,
}

/// Parse one JSON text with Python `json.loads` semantics.
pub fn parse(text: &str) -> Result<PyValue, ParseError> {
    parse_with(text, ParseOptions::default())
}

/// Parse one JSON text with explicit strictness options.
pub fn parse_with(text: &str, options: ParseOptions) -> Result<PyValue, ParseError> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        options,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(err(format!("trailing data at byte {}", parser.pos)));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    options: ParseOptions,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(err(format!(
                "expected {:?} at byte {}",
                byte as char, self.pos
            )))
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), ParseError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(err(format!("invalid literal at byte {}", self.pos)))
        }
    }

    fn value(&mut self) -> Result<PyValue, ParseError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(PyValue::Str(self.string()?)),
            Some(b't') => {
                self.literal("true")?;
                Ok(PyValue::Bool(true))
            }
            Some(b'f') => {
                self.literal("false")?;
                Ok(PyValue::Bool(false))
            }
            Some(b'n') => {
                self.literal("null")?;
                Ok(PyValue::Null)
            }
            Some(b'N') => {
                self.literal("NaN")?;
                self.finite(f64::NAN)
            }
            Some(b'I') => {
                self.literal("Infinity")?;
                self.finite(f64::INFINITY)
            }
            Some(b'-') if self.bytes[self.pos..].starts_with(b"-Infinity") => {
                self.literal("-Infinity")?;
                self.finite(f64::NEG_INFINITY)
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(err(format!("unexpected input at byte {}", self.pos))),
        }
    }

    fn finite(&self, value: f64) -> Result<PyValue, ParseError> {
        if self.options.finite_only && !value.is_finite() {
            return Err(err("non-finite number"));
        }
        Ok(PyValue::Float(value))
    }

    fn object(&mut self) -> Result<PyValue, ParseError> {
        self.expect(b'{')?;
        self.skip_ws();
        let mut pairs: Vec<(String, PyValue)> = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(PyValue::Object(pairs));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            if self.options.unique_keys && pairs.iter().any(|(k, _)| *k == key) {
                return Err(err(format!("repeats the key {key:?}")));
            }
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(PyValue::Object(pairs));
                }
                _ => return Err(err(format!("expected ',' or '}}' at byte {}", self.pos))),
            }
        }
    }

    fn array(&mut self) -> Result<PyValue, ParseError> {
        self.expect(b'[')?;
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(PyValue::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(PyValue::Array(items));
                }
                _ => return Err(err(format!("expected ',' or ']' at byte {}", self.pos))),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let byte = self
                .peek()
                .ok_or_else(|| err("unterminated unicode escape"))?;
            let digit = (byte as char)
                .to_digit(16)
                .ok_or_else(|| err("invalid unicode escape"))?;
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or_else(|| err("unterminated string"))?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| err("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            if (0xD800..0xDC00).contains(&first) {
                                if self.bytes[self.pos..].starts_with(b"\\u") {
                                    self.pos += 2;
                                    let second = self.hex4()?;
                                    if (0xDC00..0xE000).contains(&second) {
                                        let combined =
                                            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                                        out.push(
                                            char::from_u32(combined)
                                                .ok_or_else(|| err("invalid surrogate pair"))?,
                                        );
                                    } else {
                                        return Err(err("lone surrogate in string escape"));
                                    }
                                } else {
                                    return Err(err("lone surrogate in string escape"));
                                }
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(err("lone surrogate in string escape"));
                            } else {
                                out.push(
                                    char::from_u32(first)
                                        .ok_or_else(|| err("invalid unicode escape"))?,
                                );
                            }
                        }
                        _ => return Err(err("invalid escape")),
                    }
                }
                0x00..=0x1F => return Err(err("unescaped control character in string")),
                _ => {
                    let rest = &self.bytes[self.pos..];
                    let text = std::str::from_utf8(rest).map_err(|_| err("invalid UTF-8"))?;
                    let ch = text
                        .chars()
                        .next()
                        .ok_or_else(|| err("unterminated string"))?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn number(&mut self) -> Result<PyValue, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(err(format!("invalid number at byte {start}"))),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(err(format!("invalid number at byte {start}")));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(err(format!("invalid number at byte {start}")));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let token = std::str::from_utf8(&self.bytes[start..self.pos])
            .expect("number token is ASCII")
            .to_owned();
        if is_float {
            let value: f64 = token.parse().map_err(|_| err("invalid float"))?;
            self.finite(value)
        } else if token == "-0" {
            Ok(PyValue::Int("0".to_owned()))
        } else {
            Ok(PyValue::Int(token))
        }
    }
}

/// Format a float exactly as Python `repr` (and therefore `json.dumps`).
pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    // Shortest round-trip digits via Rust's exact formatting, then re-lay
    // them out with CPython's fixed-vs-exponent choice.
    let shortest = format!("{:e}", value.abs());
    let (mantissa, exponent) = shortest
        .split_once('e')
        .map(|(m, e)| (m.to_owned(), e.parse::<i32>().unwrap_or(0)))
        .unwrap_or_else(|| (shortest.clone(), 0));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if value < 0.0 { "-" } else { "" };
    // CPython repr: exponent form iff decimal exponent < -4 or >= 16.
    if !(-4..16).contains(&exponent) {
        let mantissa_text = if digits.len() == 1 {
            digits.to_owned()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let exp_sign = if exponent < 0 { "-" } else { "+" };
        return format!("{sign}{mantissa_text}e{exp_sign}{:02}", exponent.abs());
    }
    if exponent >= 0 {
        let point = (exponent + 1) as usize;
        if digits.len() <= point {
            format!("{sign}{}{}.0", digits, "0".repeat(point - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..point], &digits[point..])
        }
    } else {
        format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
    }
}

/// Escape one string as Python `json.dumps(..., ensure_ascii=True)` does,
/// including the surrounding quotes.
pub fn dumps_str_ascii(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x80 => out.push(c),
            c if (c as u32) <= 0xFFFF => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => {
                let value = c as u32 - 0x10000;
                out.push_str(&format!(
                    "\\u{:04x}\\u{:04x}",
                    0xD800 + (value >> 10),
                    0xDC00 + (value & 0x3FF)
                ));
            }
        }
    }
    out.push('"');
    out
}

/// Escape one string as serde_json does (raw UTF-8), with quotes.
pub fn dumps_str_utf8(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Which string escaping a dump uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    /// Python `ensure_ascii=True`.
    Ascii,
    /// Raw UTF-8 (serde_json style).
    Utf8,
}

fn dump_into(value: &PyValue, escape: Escape, pairs_aware: bool, out: &mut String) {
    match value {
        PyValue::Null => out.push_str("null"),
        PyValue::Bool(true) => out.push_str("true"),
        PyValue::Bool(false) => out.push_str("false"),
        PyValue::Int(text) => out.push_str(text),
        PyValue::Float(f) => out.push_str(&py_float_repr(*f)),
        PyValue::Str(s) => out.push_str(&match escape {
            Escape::Ascii => dumps_str_ascii(s),
            Escape::Utf8 => dumps_str_utf8(s),
        }),
        PyValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_into(item, escape, pairs_aware, out);
            }
            out.push(']');
        }
        PyValue::Object(pairs) => {
            let mut ordered: Vec<&(String, PyValue)> = if pairs_aware {
                pairs.iter().collect()
            } else {
                // Dict view: last value per key.
                let mut keys: Vec<&String> = Vec::new();
                for (k, _) in pairs {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
                keys.into_iter()
                    .map(|k| {
                        pairs
                            .iter()
                            .rev()
                            .find(|(kk, _)| kk == k)
                            .expect("key present")
                    })
                    .collect()
            };
            ordered.sort_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (i, (key, child)) in ordered.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&match escape {
                    Escape::Ascii => dumps_str_ascii(key),
                    Escape::Utf8 => dumps_str_utf8(key),
                });
                out.push(':');
                dump_into(child, escape, pairs_aware, out);
            }
            out.push('}');
        }
    }
}

/// Python `json.dumps(value, sort_keys=True, separators=(",", ":"))` over
/// the dict view (duplicate keys collapsed to last value), ASCII-escaped.
pub fn dumps_sorted(value: &PyValue) -> String {
    let mut out = String::new();
    dump_into(value, Escape::Ascii, false, &mut out);
    out
}

/// Written-pair identity: every pair at every depth, stable-sorted by key,
/// ASCII-escaped. This is the union's conflict-comparison encoding.
pub fn dumps_pairs(value: &PyValue) -> String {
    let mut out = String::new();
    dump_into(value, Escape::Ascii, true, &mut out);
    out
}

/// Sorted dump with raw UTF-8 strings (admission-context style).
pub fn dumps_sorted_utf8(value: &PyValue) -> String {
    let mut out = String::new();
    dump_into(value, Escape::Utf8, false, &mut out);
    out
}

/// Compare two key strings as Python compares `str` (code-point order,
/// which UTF-8 byte order preserves for scalar text).
pub fn py_str_cmp(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

/// Python `str(value)` for the coercions union grouping performs.
pub fn py_str(value: Option<&PyValue>) -> String {
    match value {
        None => String::new(),
        Some(PyValue::Str(s)) => s.clone(),
        Some(PyValue::Null) => "None".to_owned(),
        Some(PyValue::Bool(true)) => "True".to_owned(),
        Some(PyValue::Bool(false)) => "False".to_owned(),
        Some(PyValue::Int(text)) => text.clone(),
        Some(PyValue::Float(f)) => py_float_repr(*f),
        Some(other) => py_repr(other),
    }
}

/// Python `repr(value)` for the shapes refusal messages name.
pub fn py_repr(value: &PyValue) -> String {
    match value {
        PyValue::Str(s) => py_repr_str(s),
        PyValue::Null => "None".to_owned(),
        PyValue::Bool(true) => "True".to_owned(),
        PyValue::Bool(false) => "False".to_owned(),
        PyValue::Int(text) => text.clone(),
        PyValue::Float(f) => py_float_repr(*f),
        PyValue::Array(items) => {
            format!(
                "[{}]",
                items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
            )
        }
        PyValue::Object(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Deep-copy a value (values are owned; this is `Clone` with intent).
pub fn deep_copy(value: &PyValue) -> PyValue {
    value.clone()
}
