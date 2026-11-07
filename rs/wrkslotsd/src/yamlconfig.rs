//! The strict YAML subset for `.wrkslots.yml`, reader half.
//!
//! A port of the document loader the Python edition uses: block mappings and
//! sequences nested by spaces, one-line flow collections, quoted and plain
//! scalars, comments, an optional leading `---`, and JSON documents (first
//! non-blank character `{`) with duplicate keys refused. Anything outside
//! the subset is refused with its 1-based line number instead of being
//! guessed at. Scalars follow YAML 1.2 (`yes`/`no`/`on`/`off` are strings).

use serde_json::{json, Map, Value};

/// One refused document; `line` is 1-based when known.
#[derive(Debug)]
pub(crate) struct YamlError {
    pub(crate) message: String,
    pub(crate) line: Option<usize>,
}

impl YamlError {
    fn at(message: impl Into<String>, line: usize) -> Self {
        Self {
            message: message.into(),
            line: Some(line),
        }
    }
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => write!(formatter, "{}", self.message),
        }
    }
}

type Result<T> = std::result::Result<T, YamlError>;

fn is_null(text: &str) -> bool {
    matches!(text, "null" | "Null" | "NULL" | "~" | "")
}

fn plain_scalar(text: &str, line: usize) -> Result<Value> {
    if is_null(text) {
        return Ok(Value::Null);
    }
    if matches!(text, "true" | "True" | "TRUE") {
        return Ok(Value::Bool(true));
    }
    if matches!(text, "false" | "False" | "FALSE") {
        return Ok(Value::Bool(false));
    }
    let bytes = text.as_bytes();
    let is_int = {
        let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    };
    if is_int {
        if let Ok(value) = text.parse::<i64>() {
            return Ok(json!(value));
        }
        return Ok(Value::String(text.to_owned()));
    }
    if let Some(hex) = text.strip_prefix("0x") {
        if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let Ok(value) = i64::from_str_radix(hex, 16) {
                return Ok(json!(value));
            }
        }
    }
    if let Some(oct) = text.strip_prefix("0o") {
        if !oct.is_empty() && oct.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            if let Ok(value) = i64::from_str_radix(oct, 8) {
                return Ok(json!(value));
            }
        }
    }
    if is_float_text(text) {
        if let Ok(value) = text.parse::<f64>() {
            if let Some(number) = serde_json::Number::from_f64(value) {
                return Ok(Value::Number(number));
            }
            return Ok(Value::String(text.to_owned()));
        }
    }
    if matches!(
        text,
        ".inf" | ".Inf" | ".INF" | "-.inf" | "-.Inf" | "-.INF" | "+.inf" | "+.Inf" | "+.INF"
    ) {
        return Ok(Value::String(text.to_owned()));
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return Ok(Value::String(text.to_owned()));
    }
    let first = bytes[0];
    if matches!(first, b'&' | b'*' | b'!') {
        return Err(YamlError::at(
            "anchors, aliases, and tags are not supported",
            line,
        ));
    }
    if matches!(first, b'|' | b'>') {
        return Err(YamlError::at(
            "block scalars (| and >) are not supported",
            line,
        ));
    }
    if matches!(first, b'%' | b'@' | b'`') {
        return Err(YamlError::at(
            format!(
                "a plain value may not begin with {:?}; quote it",
                first as char
            ),
            line,
        ));
    }
    if matches!(first, b'?' | b':' | b'-' | b',') && (text.len() == 1 || text.as_bytes()[1] == b' ')
    {
        return Err(YamlError::at(
            format!("unexpected {:?}; quote the value", first as char),
            line,
        ));
    }
    if text.contains(": ") || text.ends_with(':') {
        return Err(YamlError::at(
            "a plain value may not contain ': '; quote it",
            line,
        ));
    }
    if matches!(first, b']' | b'}') {
        return Err(YamlError::at(
            format!("unexpected {:?}", first as char),
            line,
        ));
    }
    Ok(Value::String(text.to_owned()))
}

fn is_float_text(text: &str) -> bool {
    // [-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (&unsigned[..index], Some(&unsigned[index + 1..])),
        None => (unsigned, None),
    };
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    if let Some(rest) = mantissa.strip_prefix('.') {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    let mut parts = mantissa.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match parts.next() {
        None => exponent.is_some(),
        Some(fraction) => fraction.bytes().all(|b| b.is_ascii_digit()),
    }
}

fn quoted_scalar(text: &str, line: usize) -> Result<String> {
    if text.starts_with('"') {
        let value: Value = serde_json::from_str(text).map_err(|error| {
            YamlError::at(format!("invalid double-quoted string: {error}"), line)
        })?;
        return value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| YamlError::at("invalid double-quoted string", line));
    }
    if text.len() < 2 || !text.ends_with('\'') {
        return Err(YamlError::at("unterminated single-quoted string", line));
    }
    let inner = &text[1..text.len() - 1];
    let collapsed = inner.replace("''", "");
    if collapsed.contains('\'') {
        return Err(YamlError::at("stray quote in single-quoted string", line));
    }
    Ok(inner.replace("''", "'"))
}

fn scalar(text: &str, line: usize) -> Result<Value> {
    match text.as_bytes()[0] {
        b'"' | b'\'' => Ok(Value::String(quoted_scalar(text, line)?)),
        b'[' | b'{' => flow(text, line),
        _ => plain_scalar(text, line),
    }
}

fn skip_spaces(text: &str, mut index: usize) -> usize {
    while text.as_bytes().get(index) == Some(&b' ') {
        index += 1;
    }
    index
}

fn flow(text: &str, line: usize) -> Result<Value> {
    let (value, end) = flow_value(text, 0, line)?;
    if !text[end..].trim().is_empty() {
        return Err(YamlError::at(
            format!(
                "unexpected text after a flow collection: {:?}",
                text[end..].trim()
            ),
            line,
        ));
    }
    Ok(value)
}

fn flow_value(text: &str, index: usize, line: usize) -> Result<(Value, usize)> {
    let bytes = text.as_bytes();
    let index = skip_spaces(text, index);
    if index >= text.len() {
        return Err(YamlError::at(
            "multi-line flow collections are not supported; close it on one line",
            line,
        ));
    }
    match bytes[index] {
        b'[' => {
            let mut items = Vec::new();
            let mut index = skip_spaces(text, index + 1);
            if bytes.get(index) == Some(&b']') {
                return Ok((Value::Array(items), index + 1));
            }
            loop {
                let (item, next) = flow_value(text, index, line)?;
                items.push(item);
                index = skip_spaces(text, next);
                match bytes.get(index) {
                    None => {
                        return Err(YamlError::at(
                            "multi-line flow collections are not supported; close it on one line",
                            line,
                        ))
                    }
                    Some(b']') => return Ok((Value::Array(items), index + 1)),
                    Some(b',') => index += 1,
                    Some(other) => {
                        return Err(YamlError::at(
                            format!(
                                "expected ',' or ']' in a flow sequence, found {:?}",
                                *other as char
                            ),
                            line,
                        ))
                    }
                }
            }
        }
        b'{' => {
            let mut mapping = Map::new();
            let mut index = skip_spaces(text, index + 1);
            if bytes.get(index) == Some(&b'}') {
                return Ok((Value::Object(mapping), index + 1));
            }
            loop {
                let (key, next) = flow_value(text, index, line)?;
                index = skip_spaces(text, next);
                if bytes.get(index) != Some(&b':') {
                    return Err(YamlError::at("expected ':' after a flow mapping key", line));
                }
                let key = key
                    .as_str()
                    .ok_or_else(|| YamlError::at("flow mapping keys must be strings", line))?
                    .to_owned();
                if mapping.contains_key(&key) {
                    return Err(YamlError::at(format!("duplicate key {key:?}"), line));
                }
                let (item, next) = flow_value(text, index + 1, line)?;
                mapping.insert(key, item);
                index = skip_spaces(text, next);
                match bytes.get(index) {
                    None => {
                        return Err(YamlError::at(
                            "multi-line flow collections are not supported; close it on one line",
                            line,
                        ))
                    }
                    Some(b'}') => return Ok((Value::Object(mapping), index + 1)),
                    Some(b',') => index += 1,
                    Some(other) => {
                        return Err(YamlError::at(
                            format!(
                                "expected ',' or '}}' in a flow mapping, found {:?}",
                                *other as char
                            ),
                            line,
                        ))
                    }
                }
            }
        }
        quote @ (b'"' | b'\'') => {
            let mut end = index + 1;
            loop {
                match bytes.get(end) {
                    None => return Err(YamlError::at("unterminated quoted string", line)),
                    Some(b'\\') if quote == b'"' => end += 2,
                    Some(other) if *other == quote => {
                        if quote == b'\'' && bytes.get(end + 1) == Some(&b'\'') {
                            end += 2;
                            continue;
                        }
                        break;
                    }
                    Some(_) => end += 1,
                }
            }
            Ok((
                Value::String(quoted_scalar(&text[index..=end], line)?),
                end + 1,
            ))
        }
        _ => {
            let mut end = index;
            while let Some(byte) = bytes.get(end) {
                if matches!(byte, b',' | b']' | b'}') {
                    break;
                }
                if *byte == b':'
                    && (end + 1 >= text.len()
                        || matches!(bytes[end + 1], b' ' | b',' | b']' | b'}'))
                {
                    break;
                }
                end += 1;
            }
            let token = text[index..end].trim();
            if token.is_empty() {
                return Err(YamlError::at("empty entry in a flow collection", line));
            }
            Ok((plain_scalar(token, line)?, end))
        }
    }
}

struct Line {
    number: usize,
    indent: usize,
    text: String,
}

fn strip_comment(text: &str, number: usize) -> Result<String> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < text.len() {
        let character = bytes[index];
        match quote {
            Some(b'"') => {
                if character == b'\\' {
                    index += 2;
                    continue;
                }
                if character == b'"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if character == b'\'' {
                    if bytes.get(index + 1) == Some(&b'\'') {
                        index += 2;
                        continue;
                    }
                    quote = None;
                }
            }
            _ => {
                if character == b'#' && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t')) {
                    return Ok(text[..index].trim_end().to_owned());
                }
                if matches!(character, b'"' | b'\'')
                    && (index == 0
                        || matches!(
                            bytes[index - 1],
                            b' ' | b'\t' | b'[' | b'{' | b',' | b':' | b'-'
                        ))
                {
                    quote = Some(character);
                }
            }
        }
        index += 1;
    }
    if quote.is_some() {
        return Err(YamlError::at("unterminated quoted string", number));
    }
    Ok(text.trim_end().to_owned())
}

fn logical_lines(text: &str) -> Result<Vec<Line>> {
    let mut lines = Vec::new();
    let mut seen_content = false;
    for (offset, raw) in text.split('\n').enumerate() {
        let number = offset + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let body = raw.trim_start_matches(' ');
        let leading = &raw[..raw.len() - body.len()];
        if body.starts_with('\t') || leading.contains('\t') {
            if body.trim().is_empty() {
                continue;
            }
            return Err(YamlError::at(
                "tabs are not allowed for indentation",
                number,
            ));
        }
        let content = strip_comment(body, number)?;
        if content.is_empty() {
            continue;
        }
        if content.starts_with('%') {
            return Err(YamlError::at("directives are not supported", number));
        }
        if matches!(content.as_str(), "---" | "...")
            || content.starts_with("--- ")
            || content.starts_with("... ")
        {
            if content == "---" && !seen_content && leading.is_empty() {
                continue;
            }
            return Err(YamlError::at(
                "multiple documents and document markers are not supported",
                number,
            ));
        }
        seen_content = true;
        lines.push(Line {
            number,
            indent: leading.len(),
            text: content,
        });
    }
    Ok(lines)
}

const PLAIN_FORBIDDEN_START: &[u8] = b"-?:,[]{}#&*!|>'\"%@`";

fn split_key(text: &str, number: usize) -> Result<Option<(String, String)>> {
    let bytes = text.as_bytes();
    if text.starts_with("? ") || text == "?" {
        return Err(YamlError::at("complex keys (?) are not supported", number));
    }
    if matches!(bytes[0], b'"' | b'\'') {
        let quote = bytes[0];
        let mut index = 1;
        loop {
            match bytes.get(index) {
                None => return Err(YamlError::at("unterminated quoted key", number)),
                Some(b'\\') if quote == b'"' => index += 2,
                Some(other) if *other == quote => {
                    if quote == b'\'' && bytes.get(index + 1) == Some(&b'\'') {
                        index += 2;
                        continue;
                    }
                    break;
                }
                Some(_) => index += 1,
            }
        }
        let rest = &text[index + 1..];
        if rest == ":" || rest.starts_with(": ") {
            let key = quoted_scalar(&text[..=index], number)?;
            return Ok(Some((key, rest[1..].trim().to_owned())));
        }
        return Ok(None);
    }
    if PLAIN_FORBIDDEN_START.contains(&bytes[0]) {
        return Ok(None);
    }
    // First ": " or trailing ':' outside any quotes (plain keys have none).
    let mut colon: Option<usize> = None;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' && (index + 1 == bytes.len() || bytes[index + 1] == b' ') {
            colon = Some(index);
            break;
        }
        index += 1;
    }
    let Some(colon) = colon else {
        return Ok(None);
    };
    let key = text[..colon].trim_end();
    if key.is_empty() || key.trim() != key {
        return Ok(None);
    }
    if matches!(key.as_bytes()[0], b'&' | b'*' | b'!') {
        return Err(YamlError::at(
            "anchors, aliases, and tags are not supported",
            number,
        ));
    }
    let rest = if colon + 1 == text.len() {
        String::new()
    } else {
        text[colon + 2..].trim().to_owned()
    };
    Ok(Some((key.to_owned(), rest)))
}

struct Parser {
    lines: Vec<Line>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Line> {
        self.lines.get(self.position)
    }

    fn parse_document(&mut self) -> Result<Value> {
        let Some(first) = self.peek() else {
            return Ok(Value::Null);
        };
        if first.indent != 0 {
            return Err(YamlError::at(
                "the document must start at column 1",
                first.number,
            ));
        }
        let value = self.parse_block(0)?;
        if let Some(leftover) = self.peek() {
            return Err(YamlError::at(
                "unexpected indentation or content",
                leftover.number,
            ));
        }
        Ok(value)
    }

    fn parse_block(&mut self, indent: usize) -> Result<Value> {
        let line = self.peek().expect("block line").clone_text();
        if line.0 == "-" || line.0.starts_with("- ") {
            return self.parse_sequence(indent);
        }
        if split_key(&line.0, line.1)?.is_some() {
            return self.parse_mapping(indent, None);
        }
        self.position += 1;
        let value = scalar(&line.0, line.1)?;
        if let Some(following) = self.peek() {
            if following.indent > indent {
                return Err(YamlError::at(
                    "multi-line values are not supported",
                    following.number,
                ));
            }
        }
        Ok(value)
    }

    fn parse_mapping(&mut self, indent: usize, first: Option<(String, usize)>) -> Result<Value> {
        let mut mapping = Map::new();
        let mut pending = first;
        loop {
            let (text, number) = if let Some(first) = pending.take() {
                first
            } else {
                let Some(line) = self.peek() else {
                    return Ok(Value::Object(mapping));
                };
                if line.indent < indent {
                    return Ok(Value::Object(mapping));
                }
                if line.indent > indent {
                    return Err(YamlError::at("unexpected indentation", line.number));
                }
                if line.text == "-" || line.text.starts_with("- ") {
                    return Err(YamlError::at(
                        "a sequence item cannot follow mapping keys at the same level",
                        line.number,
                    ));
                }
                let owned = (line.text.clone(), line.number);
                self.position += 1;
                owned
            };
            let Some((key, rest)) = split_key(&text, number)? else {
                return Err(YamlError::at(
                    format!("expected 'key: value', found {text:?}"),
                    number,
                ));
            };
            if mapping.contains_key(&key) {
                return Err(YamlError::at(format!("duplicate key {key:?}"), number));
            }
            let value = self.parse_value(&rest, indent, number, true)?;
            mapping.insert(key, value);
        }
    }

    fn parse_value(
        &mut self,
        rest: &str,
        indent: usize,
        number: usize,
        allow_same_indent_sequence: bool,
    ) -> Result<Value> {
        if !rest.is_empty() {
            let value = scalar(rest, number)?;
            if let Some(following) = self.peek() {
                if following.indent > indent {
                    return Err(YamlError::at(
                        "multi-line values are not supported",
                        following.number,
                    ));
                }
            }
            return Ok(value);
        }
        if let Some(following) = self.peek() {
            if following.indent > indent {
                let next = following.indent;
                return self.parse_block(next);
            }
            if allow_same_indent_sequence
                && following.indent == indent
                && (following.text == "-" || following.text.starts_with("- "))
            {
                return self.parse_sequence(indent);
            }
        }
        Ok(Value::Null)
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Value> {
        let mut items = Vec::new();
        loop {
            let Some(line) = self.peek() else {
                return Ok(Value::Array(items));
            };
            if line.indent < indent {
                return Ok(Value::Array(items));
            }
            if line.indent > indent {
                return Err(YamlError::at("unexpected indentation", line.number));
            }
            if !(line.text == "-" || line.text.starts_with("- ")) {
                return Ok(Value::Array(items));
            }
            let (text, number) = (line.text.clone(), line.number);
            self.position += 1;
            let rest_full = &text[1..];
            let rest = rest_full.trim_start_matches(' ');
            if rest.is_empty() {
                if let Some(following) = self.peek() {
                    if following.indent > indent {
                        let next = following.indent;
                        items.push(self.parse_block(next)?);
                        continue;
                    }
                }
                items.push(Value::Null);
                continue;
            }
            let item_indent = indent + (text.len() - rest.len());
            if rest == "-" || rest.starts_with("- ") {
                return Err(YamlError::at(
                    "nested sequences on one line are not supported",
                    number,
                ));
            }
            if !matches!(rest.as_bytes()[0], b'[' | b'{') && split_key(rest, number)?.is_some() {
                items.push(self.parse_mapping(item_indent, Some((rest.to_owned(), number)))?);
                continue;
            }
            items.push(self.parse_value(rest, indent, number, false)?);
        }
    }
}

impl Line {
    fn clone_text(&self) -> (String, usize) {
        (self.text.clone(), self.number)
    }
}

fn loads_json(text: &str) -> Result<Value> {
    // Decode through a recursive visitor so duplicate keys are refused at
    // every depth, not merged.
    #[derive(Debug)]
    enum Strict {
        Null,
        Bool(bool),
        Int(i64),
        Float(f64),
        Text(String),
        Array(Vec<Strict>),
        Object(Vec<(String, Strict)>),
    }

    impl<'de> serde::Deserialize<'de> for Strict {
        fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct StrictVisitor;
            impl<'de> serde::de::Visitor<'de> for StrictVisitor {
                type Value = Strict;

                fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    formatter.write_str("any JSON value without duplicate keys")
                }

                fn visit_bool<E: serde::de::Error>(
                    self,
                    value: bool,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict::Bool(value))
                }

                fn visit_i64<E: serde::de::Error>(
                    self,
                    value: i64,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict::Int(value))
                }

                fn visit_u64<E: serde::de::Error>(
                    self,
                    value: u64,
                ) -> std::result::Result<Strict, E> {
                    i64::try_from(value)
                        .map(Strict::Int)
                        .map_err(|_| serde::de::Error::custom("integer out of range"))
                }

                fn visit_f64<E: serde::de::Error>(
                    self,
                    value: f64,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict::Float(value))
                }

                fn visit_str<E: serde::de::Error>(
                    self,
                    value: &str,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict::Text(value.to_owned()))
                }

                fn visit_string<E: serde::de::Error>(
                    self,
                    value: String,
                ) -> std::result::Result<Strict, E> {
                    Ok(Strict::Text(value))
                }

                fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                    Ok(Strict::Null)
                }

                fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                    Ok(Strict::Null)
                }

                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut access: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut items = Vec::new();
                    while let Some(item) = access.next_element::<Strict>()? {
                        items.push(item);
                    }
                    Ok(Strict::Array(items))
                }

                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut access: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut pairs: Vec<(String, Strict)> = Vec::new();
                    while let Some((key, value)) = access.next_entry::<String, Strict>()? {
                        if pairs.iter().any(|(existing, _)| *existing == key) {
                            return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                        }
                        pairs.push((key, value));
                    }
                    Ok(Strict::Object(pairs))
                }
            }
            deserializer.deserialize_any(StrictVisitor)
        }
    }

    fn convert(value: Strict) -> Value {
        match value {
            Strict::Null => Value::Null,
            Strict::Bool(flag) => Value::Bool(flag),
            Strict::Int(number) => json!(number),
            Strict::Float(number) => {
                serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
            }
            Strict::Text(text) => Value::String(text),
            Strict::Array(items) => Value::Array(items.into_iter().map(convert).collect()),
            Strict::Object(pairs) => Value::Object(
                pairs
                    .into_iter()
                    .map(|(key, value)| (key, convert(value)))
                    .collect(),
            ),
        }
    }

    let strict: Strict = serde_json::from_str(text)
        .map_err(|error| YamlError::at(format!("malformed JSON: {error}"), error.line()))?;
    Ok(convert(strict))
}

/// Decode a configuration document; returns `(value, format)`.
pub(crate) fn load_document(text: &str) -> Result<(Value, &'static str)> {
    let stripped = text.trim_start();
    if stripped.starts_with('{') {
        return Ok((loads_json(text)?, "json"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines = logical_lines(text)?;
    let mut parser = Parser { lines, position: 0 };
    Ok((parser.parse_document()?, "yaml"))
}

// ------------------------------------------------------------------ writing

const YAML11_WORDS: &[&str] = &[
    "yes", "Yes", "YES", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "y", "Y", "n",
    "N",
];

fn wrap_comment(text: &str, prefix: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        if words.is_empty() {
            lines.push(prefix.trim_end().to_string());
            continue;
        }
        let mut current = prefix.to_string();
        for word in words {
            if current != prefix && current.chars().count() + 1 + word.chars().count() > 96 {
                lines.push(current);
                current = prefix.to_string();
            }
            if current != prefix {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(current);
    }
    lines
}

fn comment_block(text: &str, indent: usize) -> Vec<String> {
    wrap_comment(text, &format!("{}# ", " ".repeat(indent)))
}

fn is_scalar_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// The canonical YAML text of one scalar (the Python `scalar_text`).
pub(crate) fn scalar_text(value: &Value, flow: bool) -> std::result::Result<String, String> {
    match value {
        Value::Null => Ok("null".to_string()),
        Value::Bool(true) => Ok("true".to_string()),
        Value::Bool(false) => Ok("false".to_string()),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Ok(integer.to_string());
            }
            if let Some(unsigned) = number.as_u64() {
                return Ok(unsigned.to_string());
            }
            let float = number.as_f64().unwrap_or(f64::NAN);
            if float.is_nan() {
                return Ok(".nan".to_string());
            }
            if float.is_infinite() {
                return Ok(if float < 0.0 { "-.inf" } else { ".inf" }.to_string());
            }
            if float == float.trunc() && float.abs() < 1e16 {
                return Ok(format!("{float:.1}"));
            }
            Ok(format!("{float}"))
        }
        Value::String(text) => {
            let plain_ok = !text.is_empty()
                && text.trim() == text.as_str()
                && !YAML11_WORDS.contains(&text.as_str())
                && !(flow
                    && text
                        .chars()
                        .any(|ch| matches!(ch, ',' | '[' | ']' | '{' | '}')))
                && !text
                    .chars()
                    .any(|ch| (ch as u32) < 32 || matches!(ch, '#' | '"' | '\''));
            if plain_ok {
                if let Ok(parsed) = plain_scalar(text, 0) {
                    if parsed == Value::String(text.clone())
                        && !PLAIN_FORBIDDEN_START.contains(&text.as_bytes()[0])
                    {
                        return Ok(text.clone());
                    }
                }
            }
            Ok(serde_json::to_string(text).unwrap_or_default())
        }
        _ => Err(format!(
            "cannot write {} as a YAML scalar",
            match value {
                Value::Array(_) => "list",
                Value::Object(_) => "dict",
                _ => "value",
            }
        )),
    }
}

fn key_text(key: &str) -> String {
    let text = scalar_text(&Value::String(key.to_string()), false).unwrap_or_default();
    if text == key && !key.contains(':') {
        text
    } else {
        serde_json::to_string(key).unwrap_or_default()
    }
}

/// Write `value` as literate YAML (the Python `yamlconfig.emit`).
pub(crate) fn emit_document(
    value: &Value,
    header: &str,
    footer: &str,
    comments: &std::collections::HashMap<Vec<String>, String>,
    order: &dyn Fn(&[String], &[String]) -> Vec<String>,
) -> std::result::Result<String, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "cannot write a non-mapping document to YAML".to_string())?;
    let mut out: Vec<String> = Vec::new();
    if !header.is_empty() {
        out.extend(comment_block(header, 0));
        out.push(String::new());
    }
    emit_mapping(object, &[], 0, comments, order, &mut out)?;
    if !footer.is_empty() {
        out.push(String::new());
        out.extend(comment_block(footer, 0));
    }
    Ok(out.join("\n") + "\n")
}

fn emit_mapping(
    node: &Map<String, Value>,
    path: &[String],
    indent: usize,
    comments: &std::collections::HashMap<Vec<String>, String>,
    order: &dyn Fn(&[String], &[String]) -> Vec<String>,
    out: &mut Vec<String>,
) -> std::result::Result<(), String> {
    let keys: Vec<String> = node.keys().cloned().collect();
    let arranged = order(path, &keys);
    for (position, key) in arranged.iter().enumerate() {
        let Some(item) = node.get(key) else { continue };
        let mut here: Vec<String> = path.to_vec();
        here.push(key.clone());
        if let Some(note) = comments.get(&here) {
            if position > 0 && indent == 0 {
                out.push(String::new());
            }
            out.extend(comment_block(note, indent));
        }
        let prefix = format!("{}{}:", " ".repeat(indent), key_text(key));
        emit_entry(item, &here, indent, &prefix, comments, order, out)?;
    }
    Ok(())
}

fn emit_entry(
    item: &Value,
    here: &[String],
    indent: usize,
    prefix: &str,
    comments: &std::collections::HashMap<Vec<String>, String>,
    order: &dyn Fn(&[String], &[String]) -> Vec<String>,
    out: &mut Vec<String>,
) -> std::result::Result<(), String> {
    if is_scalar_value(item) {
        out.push(format!("{prefix} {}", scalar_text(item, false)?));
    } else if let Some(map) = item.as_object() {
        if map.is_empty() {
            out.push(format!("{prefix} {{}}"));
        } else {
            out.push(prefix.to_string());
            emit_mapping(map, here, indent + 2, comments, order, out)?;
        }
    } else if let Some(items) = item.as_array() {
        if items.is_empty() {
            out.push(format!("{prefix} []"));
        } else {
            out.push(prefix.to_string());
            emit_sequence(items, here, indent + 2, comments, order, out)?;
        }
    } else {
        return Err("cannot write value to YAML".to_string());
    }
    Ok(())
}

fn emit_sequence(
    items: &[Value],
    path: &[String],
    indent: usize,
    comments: &std::collections::HashMap<Vec<String>, String>,
    order: &dyn Fn(&[String], &[String]) -> Vec<String>,
    out: &mut Vec<String>,
) -> std::result::Result<(), String> {
    for item in items {
        if is_scalar_value(item) {
            out.push(format!(
                "{}- {}",
                " ".repeat(indent),
                scalar_text(item, false)?
            ));
        } else if let Some(map) = item.as_object() {
            if map.is_empty() {
                out.push(format!("{}- {{}}", " ".repeat(indent)));
            } else {
                let keys: Vec<String> = map.keys().cloned().collect();
                let inner = indent + 2;
                let first = &keys[0];
                let mut first_path: Vec<String> = path.to_vec();
                first_path.push(first.clone());
                let prefix = format!("{}- {}:", " ".repeat(indent), key_text(first));
                emit_entry(
                    &map[first],
                    &first_path,
                    inner,
                    &prefix,
                    comments,
                    order,
                    out,
                )?;
                for key in &keys[1..] {
                    let mut key_path: Vec<String> = path.to_vec();
                    key_path.push(key.clone());
                    let prefix = format!("{}{}:", " ".repeat(inner), key_text(key));
                    emit_entry(&map[key], &key_path, inner, &prefix, comments, order, out)?;
                }
            }
        } else if let Some(inner_items) = item.as_array() {
            if inner_items.is_empty() {
                out.push(format!("{}- []", " ".repeat(indent)));
            } else if inner_items.iter().all(is_scalar_value) {
                let parts: std::result::Result<Vec<String>, String> = inner_items
                    .iter()
                    .map(|part| scalar_text(part, true))
                    .collect();
                out.push(format!("{}- [{}]", " ".repeat(indent), parts?.join(", ")));
            } else {
                return Err("nested non-scalar sequences cannot be written".to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_block_and_flow() {
        let (value, format) = load_document(
            "schema: 2\nmachine: node-a\nslots:\n  - name: a\n    size: 0x10\n  - name: b\nflags: [true, no, 0755]\nempty:\n",
        )
        .expect("loads");
        assert_eq!(format, "yaml");
        assert_eq!(value["slots"][0]["size"], json!(16));
        assert_eq!(value["flags"], json!([true, "no", 755]));
        assert_eq!(value["empty"], Value::Null);
    }

    #[test]
    fn refuses_duplicates_and_tabs() {
        assert!(load_document("a: 1\na: 2\n").is_err());
        assert!(load_document("\ta: 1\n").is_err());
        let (value, format) = load_document("{\"a\": 1}").expect("json");
        assert_eq!(format, "json");
        assert_eq!(value["a"], json!(1));
    }
}
