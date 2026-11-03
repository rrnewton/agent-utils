//! Recount the generated count pins after merging a batch.
//!
//! The pin file lists expected test populations as `("name", count)` entries.
//! Two changes that each bump the same pin conflict textually even though
//! their intent composes: the batch value is the base value plus each
//! change's own delta against its merge base. With `recount_pins` on, the
//! queue merges the batch, then rewrites every pin in the candidate to
//! `base + sum(head - mergebase)` per change, matched per `(name,
//! occurrence index)` in file order. Only the numbers change; the
//! candidate's comments are kept.
//!
//! A change whose pins cannot be recounted (a pin missing in some version,
//! an occurrence-count mismatch across versions, or a resolved value below
//! the base) is skipped from the batch at construction and stays queued;
//! the refusal is recorded, never silent, and never fails the batch.

use std::collections::BTreeMap;

/// Repository-relative path of the generated pin file.
pub const PIN_PATH: &str = "ci/manifest-plan/src/validation_dag_static.rs";

/// One pin entry: its name and count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pin {
    /// Test-population name.
    pub name: String,
    /// Expected count.
    pub value: u64,
}

/// Pins of a file in order, with the byte span of each count literal so a
/// rewrite can replace only the numbers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinFile {
    /// Entries in file order.
    pub pins: Vec<Pin>,
    /// `(start, end)` byte span of each entry's count literal, aligned with `pins`.
    pub spans: Vec<(usize, usize)>,
}

/// Parse the `("name", count)` entries in file order.
pub fn parse_pins(text: &str) -> PinFile {
    let mut pins = Vec::new();
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        // An entry opens with `("` and a string, then `,`, then a number, then `)`.
        if bytes[i] == b'(' && bytes[i + 1] == b'"' {
            if let Some((name, num_at, num_len, next)) = parse_one(text, i) {
                pins.push(Pin {
                    name,
                    value: text[num_at..num_at + num_len].parse().unwrap_or(0),
                });
                spans.push((num_at, num_at + num_len));
                i = next;
                continue;
            }
        }
        i += 1;
    }
    PinFile { pins, spans }
}

fn parse_one(text: &str, open: usize) -> Option<(String, usize, usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = open + 2; // past `("`
    let name_start = i;
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let name = text[name_start..i].to_owned();
    i += 1; // past `"`
    if bytes.get(i) != Some(&b',') {
        return None;
    }
    i += 1;
    while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
        i += 1;
    }
    let num_at = i;
    while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    if i == num_at || bytes.get(i) != Some(&b')') {
        return None;
    }
    Some((name, num_at, i - num_at, i + 1))
}

/// Key of one entry: its name and occurrence index in file order.
pub fn keys(pins: &[Pin]) -> Vec<(String, usize)> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    pins.iter()
        .map(|pin| {
            let index = seen.entry(pin.name.as_str()).or_insert(0);
            let key = (pin.name.clone(), *index);
            *index += 1;
            key
        })
        .collect()
}

fn by_key(pins: &[Pin]) -> BTreeMap<(String, usize), u64> {
    keys(pins)
        .into_iter()
        .zip(pins.iter().map(|pin| pin.value))
        .collect()
}

/// Whether `text` still holds conflict markers.
pub fn has_conflict_markers(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
}

/// Validate one change's three versions and return its per-key deltas.
/// `None` texts mean the pin file is absent at that version. A change that
/// does not touch the pins yields empty deltas and no refusal.
pub fn change_deltas(
    base: Option<&str>,
    mergebase: Option<&str>,
    head: Option<&str>,
) -> Result<BTreeMap<(String, usize), i64>, String> {
    let head_text = head.unwrap_or("");
    let mb_text = mergebase.unwrap_or("");
    if head_text == mb_text {
        return Ok(BTreeMap::new());
    }
    let (Some(base_text), Some(mb), Some(hd)) = (base, mergebase, head) else {
        let missing = if base.is_none() {
            "base"
        } else if mergebase.is_none() {
            "merge base"
        } else {
            "head"
        };
        return Err(format!("pin file missing in {missing} ({PIN_PATH})"));
    };
    let base_pins = parse_pins(base_text).pins;
    let mb_pins = parse_pins(mb).pins;
    let head_pins = parse_pins(hd).pins;
    let base_map = by_key(&base_pins);
    let mb_map = by_key(&mb_pins);
    let head_map = by_key(&head_pins);
    // Occurrence counts per name must agree across all three versions.
    let mut names: Vec<String> = Vec::new();
    for pin in base_pins.iter().chain(&mb_pins).chain(&head_pins) {
        if !names.contains(&pin.name) {
            names.push(pin.name.clone());
        }
    }
    for name in &names {
        let count = |pins: &[Pin]| pins.iter().filter(|p| &p.name == name).count();
        let (b, m, h) = (count(&base_pins), count(&mb_pins), count(&head_pins));
        if b != m || m != h {
            return Err(format!(
                "occurrence-count mismatch for pin {:?}: base {b}, merge base {m}, head {h}",
                name
            ));
        }
    }
    let mut deltas = BTreeMap::new();
    for (key, mb_value) in &mb_map {
        let head_value = head_map.get(key).copied().unwrap_or(0);
        let base_value = base_map.get(key).copied().unwrap_or(0);
        let _ = base_value;
        let delta = head_value as i64 - *mb_value as i64;
        if delta != 0 {
            deltas.insert(key.clone(), delta);
        }
    }
    // A pin present only in the head (a new name) has no base to recount from.
    for key in head_map.keys() {
        if !mb_map.contains_key(key) {
            return Err(format!(
                "pin {:?} (occurrence {}) is new in the head with no merge-base value",
                key.0, key.1
            ));
        }
    }
    Ok(deltas)
}

/// Whether applying `delta` to `running` would push any pin below its base
/// value; names the first such pin.
pub fn below_base(
    base: &BTreeMap<(String, usize), u64>,
    running: &BTreeMap<(String, usize), i64>,
    delta: &BTreeMap<(String, usize), i64>,
) -> Option<(String, usize, i64, u64)> {
    for (key, d) in delta {
        let base_value = base.get(key).copied().unwrap_or(0);
        let next = running.get(key).copied().unwrap_or(base_value as i64) + d;
        if next < base_value as i64 {
            return Some((key.0.clone(), key.1, next, base_value));
        }
    }
    None
}

/// Batch values: `base + sum(deltas)` per key.
pub fn candidate_values(
    base_pins: &[Pin],
    deltas: &[&BTreeMap<(String, usize), i64>],
) -> BTreeMap<(String, usize), u64> {
    let mut values: BTreeMap<(String, usize), i64> = by_key(base_pins)
        .into_iter()
        .map(|(key, value)| (key, value as i64))
        .collect();
    for delta in deltas {
        for (key, d) in *delta {
            *values.entry(key.clone()).or_insert(0) += d;
        }
    }
    values
        .into_iter()
        .map(|(key, value)| (key, value.max(0) as u64))
        .collect()
}

/// Rewrite `text`'s pin counts to `values`, replacing only the number
/// literals (comments and layout kept). `template` counts and `values` must
/// share keys; extra template entries keep their numbers.
pub fn rewrite_pins(text: &str, values: &BTreeMap<(String, usize), u64>) -> String {
    let parsed = parse_pins(text);
    let entry_keys = keys(&parsed.pins);
    let mut out = text.to_owned();
    // Replace last-first so earlier spans stay valid.
    for (index, (key, span)) in entry_keys.iter().zip(&parsed.spans).enumerate().rev() {
        let _ = index;
        if let Some(value) = values.get(key) {
            out.replace_range(span.0..span.1, &value.to_string());
        }
    }
    out
}

#[cfg(test)]
mod unit {
    use super::*;

    const SAMPLE: &str = "const X: &[(&str, u64)] = &[\n    (\"a.test\", 10),\n    // a note\n    (\"b.test\", 3),\n    (\"a.test\", 2),\n];\n";

    #[test]
    fn parses_entries_in_order_with_spans() {
        let file = parse_pins(SAMPLE);
        assert_eq!(file.pins.len(), 3);
        assert_eq!(
            file.pins[0],
            Pin {
                name: "a.test".into(),
                value: 10
            }
        );
        assert_eq!(
            keys(&file.pins),
            vec![
                ("a.test".to_owned(), 0),
                ("b.test".to_owned(), 0),
                ("a.test".to_owned(), 1),
            ]
        );
        // Only numbers change.
        let values: BTreeMap<(String, usize), u64> = [
            (("a.test".to_owned(), 0), 12u64),
            (("b.test".to_owned(), 0), 3u64),
            (("a.test".to_owned(), 1), 5u64),
        ]
        .into_iter()
        .collect();
        let out = rewrite_pins(SAMPLE, &values);
        assert!(out.contains("(\"a.test\", 12)"), "{out}");
        assert!(out.contains("// a note"), "{out}");
        assert!(out.contains("(\"a.test\", 5)"), "{out}");
    }

    #[test]
    fn deltas_and_candidate_sum_both_changes() {
        let base = "(\"p\", 100),\n";
        let mb = "(\"p\", 100),\n";
        let head_a = "(\"p\", 103),\n";
        let head_b = "(\"p\", 102),\n";
        let da = change_deltas(Some(base), Some(mb), Some(head_a)).unwrap();
        let db = change_deltas(Some(base), Some(mb), Some(head_b)).unwrap();
        let base_pins = parse_pins(base).pins;
        let values = candidate_values(&base_pins, &[&da, &db]);
        assert_eq!(values[&("p".to_owned(), 0)], 105);
    }

    #[test]
    fn occurrence_mismatch_and_missing_refuse() {
        let base = "(\"p\", 1),\n(\"p\", 2),\n";
        let mb = "(\"p\", 1),\n";
        let head = "(\"p\", 5),\n";
        assert!(change_deltas(Some(base), Some(mb), Some(head))
            .unwrap_err()
            .contains("occurrence-count mismatch"));
        assert!(change_deltas(None, Some(mb), Some(head))
            .unwrap_err()
            .contains("missing"));
        // Untouched pins never refuse, even with no pin file at all.
        assert!(change_deltas(None, None, None).unwrap().is_empty());
    }
}
