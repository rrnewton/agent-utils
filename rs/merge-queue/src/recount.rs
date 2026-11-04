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
//! A pin the change introduces (absent at its merge base and at the base)
//! takes the head's value, and a pin the change leaves as it was at its merge
//! base keeps whatever the base did to it since (even a pin the base added or
//! removed). A change whose pins cannot be recounted (the pin file missing in
//! some version, a pin it changes whose occurrence count differs across
//! versions, or a resolved value below the base) is skipped from the batch at
//! construction and stays queued; the refusal is recorded, never silent, and
//! never fails the batch.
//!
//! A decrease (a head value below the merge base's) is refused unless the change's head commit
//! declares exactly that decrease for exactly that pin with a [`DECREASE_TRAILER`] trailer,
//! because a lower count is as likely a head pin stale against its merge base as tests the
//! change means to drop.
//!
//! A change stacked on another in the same batch (the other's head is an ancestor of its head)
//! carries the other's pin changes too, so its delta is its own minus the other's
//! ([`subtract`]): the batch counts the shared changes once, and the stacked change declares
//! only the decreases it makes itself. A change whose head is contained in an earlier change of
//! the batch adds nothing. A change stacked on one that already landed as a rebased copy is
//! measured from that change's head instead of its merge base, which predates the copy while the
//! base already counts its edits; so is a change stacked on one in a batch beneath it in the
//! window, whose candidate (carrying a copy of that change) its batch is built on. A trailer
//! never changes a count: the counts come from the pin values in the commits, and a trailer on a
//! stacked change for a decrease the other change makes matches nothing and is ignored.

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

/// Pin values of `text` by `(name, occurrence index)`; empty for no file.
pub fn values(text: Option<&str>) -> BTreeMap<(String, usize), u64> {
    text.map_or_else(BTreeMap::new, |text| by_key(&parse_pins(text).pins))
}

/// Validate one change's three versions and return its per-key deltas.
/// `None` texts mean the pin file is absent at that version. A change that
/// does not touch the pins yields empty deltas and no refusal.
///
/// Per pin name, with `b`, `m` and `h` its occurrence counts at the base, the merge base and
/// the head: a name whose occurrences the head left exactly as at the merge base yields
/// nothing, whatever the base did to it since; with `b == m == h`, each occurrence's delta is
/// `head - mergebase`; a name absent at both the base and the merge base is new in the head and
/// its delta is the head's value. Any other count change is ambiguous and refuses.
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
    let mb_map = by_key(&mb_pins);
    let head_map = by_key(&head_pins);
    let mut names: Vec<String> = Vec::new();
    for pin in base_pins.iter().chain(&mb_pins).chain(&head_pins) {
        if !names.contains(&pin.name) {
            names.push(pin.name.clone());
        }
    }
    let mut deltas = BTreeMap::new();
    for name in &names {
        let count = |pins: &[Pin]| pins.iter().filter(|p| &p.name == name).count();
        let (b, m, h) = (count(&base_pins), count(&mb_pins), count(&head_pins));
        let occurrences = |map: &BTreeMap<(String, usize), u64>, n: usize| -> Vec<u64> {
            (0..n)
                .map(|index| map.get(&(name.clone(), index)).copied().unwrap_or(0))
                .collect()
        };
        let (mb_values, head_values) = (occurrences(&mb_map, m), occurrences(&head_map, h));
        if m == h && mb_values == head_values {
            // The change leaves this pin alone: whatever the base did to it since stands.
            continue;
        }
        if b == m && m == h {
            for (index, (mb_value, head_value)) in mb_values.iter().zip(&head_values).enumerate() {
                let delta = *head_value as i64 - *mb_value as i64;
                if delta != 0 {
                    deltas.insert((name.clone(), index), delta);
                }
            }
            continue;
        }
        if b == 0 && m == 0 {
            // A pin the change introduces: there is nothing to add it to, so it is the head's.
            for (index, head_value) in head_values.iter().enumerate() {
                deltas.insert((name.clone(), index), *head_value as i64);
            }
            continue;
        }
        return Err(format!(
            "occurrence-count mismatch for pin {name:?}: base {b}, merge base {m}, head {h}"
        ));
    }
    Ok(deltas)
}

/// Trailer a change's head commit carries to declare an intended pin decrease:
/// `Count-Pin-Decrease: <pin> <amount> <reason>`. `<pin>` is a pin name, for its first
/// occurrence, or `name:N` for occurrence `N`; `<amount>` is how much it goes down (written `2`
/// or `-2`). One trailer per pin.
pub const DECREASE_TRAILER: &str = "Count-Pin-Decrease";

/// One declared pin decrease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decrease {
    /// Pin name.
    pub name: String,
    /// Occurrence index of the pin in file order.
    pub occurrence: usize,
    /// How much the pin goes down.
    pub amount: u64,
    /// Why, as the trailer says.
    pub reason: String,
}

/// The decreases `message` declares, and every [`DECREASE_TRAILER`] line it could not read.
pub fn declared_decreases(message: &str) -> (Vec<Decrease>, Vec<String>) {
    let mut declared = Vec::new();
    let mut unreadable = Vec::new();
    for line in message.lines().map(str::trim) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case(DECREASE_TRAILER) {
            continue;
        }
        let mut parts = value.trim().splitn(3, char::is_whitespace);
        let parsed = match (parts.next(), parts.next(), parts.next()) {
            (Some(pin), Some(amount), Some(reason)) if !reason.trim().is_empty() => {
                let (name, occurrence) = match pin.rsplit_once(':') {
                    Some((name, index)) => (name, index.parse::<usize>().ok()),
                    None => (pin, Some(0)),
                };
                match (occurrence, amount.parse::<i64>()) {
                    (Some(occurrence), Ok(amount)) if amount != 0 && !name.is_empty() => {
                        Some(Decrease {
                            name: name.to_owned(),
                            occurrence,
                            amount: amount.unsigned_abs(),
                            reason: reason.trim().to_owned(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        match parsed {
            Some(decrease) => declared.push(decrease),
            None => unreadable.push(line.to_owned()),
        }
    }
    (declared, unreadable)
}

/// `delta` less `earlier`, per key, without the keys that come to zero: the delta of a change
/// stacked on one whose own delta is `earlier`.
pub fn subtract(
    delta: &BTreeMap<(String, usize), i64>,
    earlier: &BTreeMap<(String, usize), i64>,
) -> BTreeMap<(String, usize), i64> {
    let mut out = delta.clone();
    for (key, d) in earlier {
        *out.entry(key.clone()).or_insert(0) -= d;
    }
    out.retain(|_, d| *d != 0);
    out
}

/// Check every decrease in a change's `delta` against the decreases its head commit declares:
/// each needs a declaration of exactly its amount for exactly its pin. Returns the declarations
/// the change honours, or why it is refused, naming the values the delta is measured from and
/// to (`mb_values` at `reference`, "the merge base" or the head of the change it is stacked on,
/// and `head_values`) and the value the pin would resolve to on the batch so far (`running`,
/// defaulting to `base`).
#[allow(clippy::too_many_arguments)]
pub fn check_decreases(
    delta: &BTreeMap<(String, usize), i64>,
    base: &BTreeMap<(String, usize), u64>,
    running: &BTreeMap<(String, usize), i64>,
    mb_values: &BTreeMap<(String, usize), u64>,
    reference: &str,
    head_values: &BTreeMap<(String, usize), u64>,
    declared: &[Decrease],
    unreadable: &[String],
) -> Result<Vec<Decrease>, String> {
    let mut honoured = Vec::new();
    for ((name, index), d) in delta {
        if *d >= 0 {
            continue;
        }
        let key = (name.clone(), *index);
        let shown = |values: &BTreeMap<(String, usize), u64>| {
            values
                .get(&key)
                .map_or_else(|| "absent".to_owned(), u64::to_string)
        };
        let base_value = base.get(&key).copied().unwrap_or(0);
        let resolved = running.get(&key).copied().unwrap_or(base_value as i64) + d;
        let pin = if *index == 0 {
            name.clone()
        } else {
            format!("{name}:{index}")
        };
        let values = format!(
            "pin {name:?} (occurrence {index}) is {} at {reference} and {} at the head ({d:+}), so on the base's {base_value} it resolves to {resolved}",
            shown(mb_values),
            shown(head_values)
        );
        match declared
            .iter()
            .find(|decrease| decrease.name == *name && decrease.occurrence == *index)
        {
            Some(decrease) if decrease.amount == d.unsigned_abs() => {
                honoured.push(decrease.clone())
            }
            Some(decrease) => {
                return Err(format!(
                    "{values}, but the head commit declares a decrease of {} ({DECREASE_TRAILER}: {pin} {} {}); a decrease must match its declaration exactly",
                    decrease.amount, decrease.amount, decrease.reason
                ));
            }
            None => {
                let unread = if unreadable.is_empty() {
                    String::new()
                } else {
                    format!(" (unreadable trailer(s): {})", unreadable.join(" | "))
                };
                return Err(format!(
                    "{values}, below the base; an undeclared decrease is refused{unread}: declare an intended one with a `{DECREASE_TRAILER}: {pin} {} <reason>` trailer on the head commit, or rebase a head whose pin is stale against its merge base",
                    d.unsigned_abs()
                ));
            }
        }
    }
    Ok(honoured)
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
    fn a_pin_the_head_introduces_takes_the_head_value() {
        let base = "(\"p\", 100),\n";
        let mb = "(\"p\", 100),\n";
        let head = "(\"p\", 101),\n(\"new.cases\", 7),\n";
        let delta = change_deltas(Some(base), Some(mb), Some(head)).unwrap();
        assert_eq!(delta[&("new.cases".to_owned(), 0)], 7);
        assert_eq!(delta[&("p".to_owned(), 0)], 1);
        let values = candidate_values(&parse_pins(base).pins, &[&delta]);
        assert_eq!(values[&("new.cases".to_owned(), 0)], 7);
        assert_eq!(values[&("p".to_owned(), 0)], 101);
        // The merged candidate carries the new entry; its number is the head's.
        let out = rewrite_pins(head, &values);
        assert!(out.contains("(\"new.cases\", 7)"), "{out}");
    }

    #[test]
    fn a_pin_the_base_added_or_removed_since_does_not_refuse_a_change_that_leaves_it() {
        // The base added `new.cases` after this change's merge base; the change only bumps `p`.
        let base = "(\"p\", 100),\n(\"new.cases\", 7),\n";
        let mb = "(\"p\", 99),\n";
        let head = "(\"p\", 102),\n";
        let delta = change_deltas(Some(base), Some(mb), Some(head)).unwrap();
        assert_eq!(delta.len(), 1, "{delta:?}");
        assert_eq!(delta[&("p".to_owned(), 0)], 3);
        let values = candidate_values(&parse_pins(base).pins, &[&delta]);
        assert_eq!(values[&("new.cases".to_owned(), 0)], 7);
        assert_eq!(values[&("p".to_owned(), 0)], 103);
        // The base removed `gone` since; the change leaves it as it was.
        let base = "(\"p\", 100),\n";
        let mb = "(\"p\", 99),\n(\"gone\", 4),\n";
        let head = "(\"p\", 102),\n(\"gone\", 4),\n";
        let delta = change_deltas(Some(base), Some(mb), Some(head)).unwrap();
        assert_eq!(delta.len(), 1, "{delta:?}");
        // But a change that edits a pin whose occurrences the base changed is ambiguous.
        let head = "(\"p\", 102),\n(\"gone\", 5),\n";
        assert!(change_deltas(Some(base), Some(mb), Some(head))
            .unwrap_err()
            .contains("occurrence-count mismatch for pin \"gone\": base 0, merge base 1, head 1"));
        // And a pin both the base and the head introduced is too.
        let base = "(\"p\", 100),\n(\"twin\", 1),\n";
        let mb = "(\"p\", 99),\n";
        let head = "(\"p\", 99),\n(\"twin\", 2),\n";
        assert!(change_deltas(Some(base), Some(mb), Some(head))
            .unwrap_err()
            .contains("occurrence-count mismatch for pin \"twin\": base 1, merge base 0, head 1"));
    }

    #[test]
    fn a_decrease_needs_a_declaration_of_exactly_its_amount() {
        let base = values(Some("(\"p\", 10),\n(\"q\", 4),\n"));
        let mb = values(Some("(\"p\", 10),\n(\"q\", 4),\n"));
        let head = values(Some("(\"p\", 8),\n(\"q\", 5),\n"));
        let delta = change_deltas(
            Some("(\"p\", 10),\n(\"q\", 4),\n"),
            Some("(\"p\", 10),\n(\"q\", 4),\n"),
            Some("(\"p\", 8),\n(\"q\", 5),\n"),
        )
        .unwrap();
        let running = BTreeMap::new();
        let check = |message: &str| {
            let (declared, unreadable) = declared_decreases(message);
            check_decreases(
                &delta,
                &base,
                &running,
                &mb,
                "the merge base",
                &head,
                &declared,
                &unreadable,
            )
        };
        // Declared exactly (either sign): honoured, and only for the decreasing pin.
        for message in [
            "Move two tests\n\nCount-Pin-Decrease: p 2 two tests move to the privileged node\n",
            "x\n\ncount-pin-decrease: p -2 same, lower-case key\n",
        ] {
            let honoured = check(message).unwrap();
            assert_eq!(honoured.len(), 1, "{honoured:?}");
            assert_eq!(honoured[0].name, "p");
            assert_eq!(honoured[0].amount, 2);
        }
        // Undeclared: refused, naming both of the change's values and the trailer to add.
        let refused = check("No trailer\n").unwrap_err();
        assert!(
            refused.contains(
                "pin \"p\" (occurrence 0) is 10 at the merge base and 8 at the head (-2)"
            ),
            "{refused}"
        );
        assert!(
            refused.contains("Count-Pin-Decrease: p 2 <reason>"),
            "{refused}"
        );
        // Larger than declared, or smaller: refused.
        let refused = check("x\n\nCount-Pin-Decrease: p 1 one test only\n").unwrap_err();
        assert!(refused.contains("declares a decrease of 1"), "{refused}");
        let refused = check("x\n\nCount-Pin-Decrease: p 3 three tests\n").unwrap_err();
        assert!(refused.contains("declares a decrease of 3"), "{refused}");
        // A declaration for another pin, or another occurrence, does not cover it.
        assert!(check("x\n\nCount-Pin-Decrease: q 2 wrong pin\n").is_err());
        assert!(check("x\n\nCount-Pin-Decrease: p:1 2 wrong occurrence\n").is_err());
        // An unreadable trailer is named in the refusal.
        let refused = check("x\n\nCount-Pin-Decrease: p two\n").unwrap_err();
        assert!(refused.contains("unreadable trailer(s)"), "{refused}");
    }

    #[test]
    fn a_stacked_change_counts_only_its_own_delta() {
        // #1 adds pin "new" and lowers p by 2; #2, stacked on #1, adds 1 to p and bumps "new".
        let base = "(\"p\", 10),\n";
        let first = change_deltas(Some(base), Some(base), Some("(\"p\", 8),\n(\"new\", 3),\n"));
        let second = change_deltas(Some(base), Some(base), Some("(\"p\", 9),\n(\"new\", 4),\n"));
        let (first, second) = (first.unwrap(), second.unwrap());
        let own = subtract(&second, &first);
        assert_eq!(
            own,
            BTreeMap::from([(("new".to_owned(), 0), 1), (("p".to_owned(), 0), 1)])
        );
        // Base plus both is the stacked head's own values: the shared changes count once.
        let values = candidate_values(&parse_pins(base).pins, &[&first, &own]);
        assert_eq!(values[&("p".to_owned(), 0)], 9);
        assert_eq!(values[&("new".to_owned(), 0)], 4);
        // A change that adds nothing to the one it is stacked on has no delta.
        assert!(subtract(&first, &first).is_empty());
    }

    #[test]
    fn decrease_trailers_parse_pin_occurrence_amount_and_reason() {
        let (declared, unreadable) = declared_decreases(
            "Subject\n\nBody text: not a trailer\nCount-Pin-Decrease: test.cli:1 -3 moved to #[ignore]\nCount-Pin-Decrease: test.cli 2\nCount-Pin-Decrease: test.x 0 nothing\n",
        );
        assert_eq!(
            declared,
            vec![Decrease {
                name: "test.cli".into(),
                occurrence: 1,
                amount: 3,
                reason: "moved to #[ignore]".into(),
            }]
        );
        // No reason, or a zero amount, cannot be read.
        assert_eq!(unreadable.len(), 2, "{unreadable:?}");
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
