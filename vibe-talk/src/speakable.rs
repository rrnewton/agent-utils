//! Turning one channel message into something worth listening to.
//!
//! # Why this exists
//!
//! Read-aloud sent the message's raw text to the vendor, so the voice said "asterisk asterisk
//! deploy asterisk asterisk", spelled out nineteen-digit snowflakes, and read ISO timestamps
//! character by character. All three are noise that a listener cannot use and cannot skip.
//!
//! # Why it is code and not a prompt
//!
//! Every one of these transformations is deterministic, and asking a model to do them would cost a
//! round trip and tokens per message to get a *less* reliable answer. Nothing here calls anything.
//!
//! # The three jobs, in order
//!
//! 1. **Markdown comes off.** Discord's flavour: fences, inline code, bold, italic, underline,
//!    strikethrough, spoilers, headers, quotes, links, mentions and custom emoji.
//! 2. **Times become relative.** A listener wants "three hours ago", not "2026-08-24T04:31:00Z".
//! 3. **Opaque strings become placeholders.** A hash is not information when spoken; it is forty
//!    seconds of alphabet. It becomes "hash code A", and the SAME hash later in the message is
//!    "hash code A" again, so the listener can tell "the same one" from "a different one" —
//!    which is the only thing about a hash that survives being read aloud.
//!
//! Order matters and is not arbitrary. Times are resolved BEFORE opaque strings, because a Unix
//! epoch and a snowflake are both long runs of digits and only one of them is a time.

use std::collections::BTreeMap;

use jiff::{Timestamp, Zoned};

use crate::clock::Zone;

/// Hex runs at least this long are hashes rather than words.
///
/// Twelve, not eight: a short git prefix is often quoted as seven or eight characters and is
/// occasionally still readable, while `deadbeef` is a word people actually write. Twelve is past
/// anything anybody says out loud on purpose.
const HASH_MIN_CHARS: usize = 12;

/// Digit runs at least this long are identifiers rather than quantities.
///
/// Ten covers a Unix epoch and a Discord snowflake. It deliberately does NOT cover a year, a port,
/// a build number or a count of anything — those are quantities a listener wants to hear.
const BIG_NUMBER_MIN_DIGITS: usize = 10;

/// Past this, a message is old enough that a relative reading stops helping.
const RELATIVE_LIMIT_DAYS: i64 = 365;

/// A fenced block this short is worth hearing; longer is worth NAMING.
///
/// Three lines is about a command and its output, or a one-line diff with context — the kind of
/// snippet whose whole point is the two words inside it. Past that, reading it aloud is a minute of
/// punctuation, and what the listener actually wants to know is that it is there and how big.
const CODE_SPOKEN_MAX_LINES: usize = 3;

/// How many first-column keys a table announcement will read out before it stops.
///
/// The keys are the useful part of a table when you cannot see it — they say what the table is
/// ABOUT. All of them would be the same wall of text the placeholder exists to remove.
const TABLE_KEYS_SPOKEN: usize = 4;

/// Rewrite a message so it can be read aloud.
///
/// `now_ms` is the instant to measure "ago" against, passed in rather than read from the clock so
/// the behaviour is testable without waiting for time to pass. `zone` is the operator's, and is
/// used only for the ABSOLUTE fallback — a relative reading is the same in every zone.
#[must_use]
pub fn for_speech(content: &str, now_ms: i64, zone: &Zone) -> String {
    let text = strip_markdown(content);
    let text = speak_times(&text, now_ms, zone);
    let text = name_opaque_strings(&text);
    collapse_whitespace(&text)
}

// --- markdown -----------------------------------------------------------------------------------

/// Take Discord's markdown off, keeping the words inside it.
fn strip_markdown(content: &str) -> String {
    let without_fences = drop_fenced_blocks(content);
    let without_tables = describe_tables(&without_fences);
    let mut out = String::with_capacity(without_tables.len());
    for (index, line) in without_tables.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&strip_line(line));
    }
    out
}

/// Replace a pipe table with a sentence about it.
///
/// A table of results is excellent to LOOK at and unbearable to hear: every cell arrives with no
/// structure around it, and by the third row the listener has lost the header. So it is announced —
/// its shape, and the first column's keys when those look like words somebody would recognise,
/// because the keys are what the table is ABOUT.
///
/// The keys are only offered when they read as English. A first column of hashes or numbers says
/// nothing when spoken, and a placeholder that recites them is the wall of text this removes.
fn describe_tables(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let run = table_run_at(&lines, i);
        match run {
            Some(end) => {
                out.push(say_table(&lines[i..end]));
                i = end;
            }
            None => {
                out.push(lines[i].to_owned());
                i += 1;
            }
        }
    }
    out.join("\n")
}

/// How far a table starting at `start` runs, or `None` if there is not one there.
///
/// Two rows minimum, and every row has to carry the same pipe shape. One line with a pipe in it is
/// a sentence with a pipe in it.
fn table_run_at(lines: &[&str], start: usize) -> Option<usize> {
    let mut end = start;
    while end < lines.len() && is_table_row(lines[end]) {
        end += 1;
    }
    if end - start >= 2 {
        Some(end)
    } else {
        None
    }
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
}

fn table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect()
}

/// Is this row the `|---|---|` separator rather than data?
fn is_table_rule(line: &str) -> bool {
    table_cells(line)
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
}

fn say_table(rows: &[&str]) -> String {
    let data: Vec<&str> = rows.iter().copied().filter(|r| !is_table_rule(r)).collect();
    let columns = data.first().map_or(0, |r| table_cells(r).len());
    // The header is not a row of data, when there was a rule under it saying so.
    let had_header = rows.iter().any(|r| is_table_rule(r));
    let body: &[&str] = if had_header && !data.is_empty() {
        &data[1..]
    } else {
        &data
    };
    let row_count = body.len();
    let col_plural = if columns == 1 { "" } else { "s" };
    let row_plural = if row_count == 1 { "" } else { "s" };
    let shape =
        format!("a table with {columns} column{col_plural} and {row_count} row{row_plural}");

    let keys: Vec<String> = body
        .iter()
        .filter_map(|r| table_cells(r).into_iter().next())
        .filter(|k| looks_like_a_word(k))
        .take(TABLE_KEYS_SPOKEN)
        .collect();
    if keys.len() < 2 || keys.len() < row_count.min(2) {
        return format!(" {shape}. ");
    }
    let more = if row_count > keys.len() {
        ", and others"
    } else {
        ""
    };
    format!(" {shape}, with keys {}{more}. ", keys.join(", "))
}

/// Does this cell read as something a person would recognise, rather than as an identifier?
fn looks_like_a_word(cell: &str) -> bool {
    let stripped: String = cell
        .chars()
        .filter(|c| !matches!(c, '*' | '`' | '_' | '~'))
        .collect();
    let stripped = stripped.trim();
    !stripped.is_empty()
        && stripped.chars().any(|c| c.is_ascii_alphabetic())
        && !is_hashlike(stripped)
        && !is_big_number(stripped)
        && stripped.chars().filter(char::is_ascii_alphabetic).count() * 2 >= stripped.len()
}

/// Replace fenced blocks with a description, or keep the short ones.
///
/// A SHORT block is usually the point of the message — a command, an error line — and silently
/// dropping it would remove the thing the listener was told to look at. A long one is a minute of
/// spoken punctuation, so it is announced instead: its language, if the fence declared one, and how
/// long it is, so the listener knows what they are choosing not to hear.
fn drop_fenced_blocks(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("```") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        match after.find("```") {
            Some(end) => {
                out.push_str(&say_code(&after[..end]));
                rest = &after[end + 3..];
            }
            // Unterminated: the rest is code, and reading it aloud is what this prevents.
            None => {
                out.push_str(&say_code(after));
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn say_code(block: &str) -> String {
    let mut lines = block.lines();
    // A fence's first line may be a language tag rather than code: ```rust
    let first = lines.next().unwrap_or_default().trim();
    let language = if !first.is_empty()
        && first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '#')
    {
        Some(first.to_owned())
    } else {
        None
    };
    let body: Vec<&str> = if language.is_some() {
        lines.collect()
    } else {
        block.lines().collect()
    };
    let count = body.iter().filter(|l| !l.trim().is_empty()).count();
    if count <= CODE_SPOKEN_MAX_LINES {
        // Short enough to be the message rather than an attachment to it. Spoken as words, with
        // the blank lines a fence always leaves behind dropped — joining them produces a spoken
        // full stop with nothing in front of it.
        let said: Vec<&str> = body
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        return format!(" {} ", said.join(". "));
    }
    let plural = if count == 1 { "" } else { "s" };
    match language {
        Some(name) => format!(" a {name} code snippet, {count} line{plural} long. "),
        None => format!(" a code snippet, {count} line{plural} long. "),
    }
}

fn strip_line(line: &str) -> String {
    let trimmed = line.trim_start();
    // Headers and quotes carry no sound. `- ` and `* ` bullets are left as words would be: the
    // pause between list items is what a reader hears anyway.
    let body = trimmed
        .trim_start_matches('>')
        .trim_start_matches('#')
        .trim_start();
    strip_inline(body)
}

/// Remove inline markers, resolve links to their text, and name the things that have no sound.
fn strip_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        // `[label](url)` reads as its label. A listener cannot use a URL and cannot write it down.
        if chars[i] == '[' {
            if let Some((label, consumed)) = link_at(&chars[i..]) {
                out.push_str(&label);
                i += consumed;
                continue;
            }
        }
        // Discord's angle-bracket forms: `<@123>`, `<#123>`, `<@&123>`, `<:name:123>`, `<t:…>`.
        // `<t:…>` is a TIME and is left for `speak_times`; the rest have no useful sound.
        if chars[i] == '<' {
            if let Some((said, consumed)) = angle_form_at(&rest) {
                out.push_str(&said);
                i += consumed;
                continue;
            }
        }
        // Emphasis markers, in descending length so `***` is not read as `*` twice. Spoilers and
        // strikethrough go the same way: the words inside them are still the message.
        let marker = ["***", "**", "~~", "||", "__", "*", "_", "`"]
            .into_iter()
            .find(|m| rest.starts_with(m));
        if let Some(marker) = marker {
            i += marker.chars().count();
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `[label](target)` → the label, and how many characters it took.
fn link_at(chars: &[char]) -> Option<(String, usize)> {
    let close = chars.iter().position(|&c| c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|&c| c == ')')? + close + 2;
    let label: String = chars[1..close].iter().collect();
    let said = if label.trim().is_empty() {
        "a link".to_owned()
    } else {
        label
    };
    Some((said, end + 1))
}

/// One of Discord's `<…>` forms, and how many characters it took.
fn angle_form_at(rest: &str) -> Option<(String, usize)> {
    let end = rest.find('>')?;
    let inner = &rest[1..end];
    let consumed = end + 1;
    // A timestamp is a TIME, not chrome. Left exactly as it is for the next pass.
    if inner.starts_with("t:") {
        return None;
    }
    let said = if inner.starts_with("@&") {
        "a role".to_owned()
    } else if inner.starts_with('@') {
        "a mention".to_owned()
    } else if inner.starts_with('#') {
        "a channel".to_owned()
    } else if inner.starts_with(':') || inner.starts_with("a:") {
        // `<:name:12345>` — the NAME is the only part with a sound.
        inner
            .split(':')
            .find(|part| !part.is_empty() && !part.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or("an emoji")
            .to_owned()
    } else if inner.starts_with("http://") || inner.starts_with("https://") {
        "a link".to_owned()
    } else {
        return None;
    };
    Some((said, consumed))
}

// --- times --------------------------------------------------------------------------------------

/// Replace every instant we can recognise with something a listener can place.
fn speak_times(text: &str, now_ms: i64, zone: &Zone) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let rest: String = bytes[i..].iter().collect();
        if let Some((said, consumed)) = discord_timestamp_at(&rest, now_ms, zone) {
            out.push_str(&said);
            i += consumed;
            continue;
        }
        if let Some((said, consumed)) = iso_timestamp_at(&rest, now_ms, zone) {
            out.push_str(&said);
            i += consumed;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Discord's own `<t:1700000000:R>` form. The style suffix is ignored: this decides how to say it.
fn discord_timestamp_at(rest: &str, now_ms: i64, zone: &Zone) -> Option<(String, usize)> {
    if !rest.starts_with("<t:") {
        return None;
    }
    let end = rest.find('>')?;
    let inner = &rest[3..end];
    let seconds: i64 = inner.split(':').next()?.parse().ok()?;
    Some((say_instant(seconds * 1000, now_ms, zone), end + 1))
}

/// A bare ISO-8601 instant sitting in the text.
fn iso_timestamp_at(rest: &str, now_ms: i64, zone: &Zone) -> Option<(String, usize)> {
    // Cheap gate before the parse: an instant starts with four digits and a dash.
    let head: String = rest.chars().take(10).collect();
    if head.len() < 10 || !looks_like_a_date(&head) {
        return None;
    }
    // Longest first, so a full instant is not truncated to its date.
    for take in (10..=30).rev() {
        let candidate: String = rest.chars().take(take).collect();
        if candidate.chars().count() < take {
            continue;
        }
        let trimmed = candidate.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        if let Ok(instant) = trimmed.parse::<Timestamp>() {
            return Some((
                say_instant(instant.as_millisecond(), now_ms, zone),
                trimmed.chars().count(),
            ));
        }
    }
    None
}

fn looks_like_a_date(head: &str) -> bool {
    let c: Vec<char> = head.chars().collect();
    c.len() >= 10
        && c[0..4].iter().all(char::is_ascii_digit)
        && c[4] == '-'
        && c[5..7].iter().all(char::is_ascii_digit)
        && c[7] == '-'
        && c[8..10].iter().all(char::is_ascii_digit)
}

/// How to say one instant, given when "now" is.
///
/// PRECISION IS PROPORTIONAL, and that is the whole rule. "Three hours and fifteen seconds ago" is
/// worse than "three hours ago" — the extra term is noise at that distance, and a listener has to
/// hold it in their head to discard it. So exactly ONE unit is ever spoken, the largest that
/// applies.
///
/// Past a year the relative reading stops helping: nobody hears "four hundred days ago" as a date.
/// Those are left as they were written, which the caller can still read, rather than converted
/// into something worse.
fn say_instant(then_ms: i64, now_ms: i64, zone: &Zone) -> String {
    let delta_ms = now_ms - then_ms;
    let future = delta_ms < 0;
    let secs = (delta_ms / 1000).abs();

    let days = secs / 86_400;
    if days > RELATIVE_LIMIT_DAYS {
        // Older than a relative reading can carry. Say the date in full, year included, because at
        // that distance the year is the part that matters.
        return absolute(then_ms, zone, true);
    }
    // Far enough back that "N days" stops being a date anybody can place.
    if days >= 7 {
        return absolute(then_ms, zone, false);
    }
    let (count, unit) = if secs < 60 {
        (secs, "second")
    } else if secs < 3_600 {
        (secs / 60, "minute")
    } else if secs < 86_400 {
        (secs / 3_600, "hour")
    } else {
        (days, "day")
    };
    let plural = if count == 1 { "" } else { "s" };
    if future {
        format!("in {count} {unit}{plural}")
    } else if count == 0 {
        "just now".to_owned()
    } else {
        format!("{count} {unit}{plural} ago")
    }
}

/// `August 3rd, a Tuesday` — and with the year when it is old enough to need one.
fn absolute(then_ms: i64, zone: &Zone, with_year: bool) -> String {
    let Ok(instant) = Timestamp::from_millisecond(then_ms) else {
        return String::new();
    };
    let local: Zoned = zone.at(instant);
    let month = local.strftime("%B").to_string();
    let day = local.day();
    let weekday = local.strftime("%A").to_string();
    let ordinal = ordinal_for(day);
    if with_year {
        let year = local.strftime("%Y").to_string();
        format!("{month} {day}{ordinal} {year}")
    } else {
        format!("{month} {day}{ordinal}, a {weekday}")
    }
}

fn ordinal_for(day: i8) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

// --- opaque strings -----------------------------------------------------------------------------

/// Replace hashes and long identifiers with placeholders, stable within one message.
///
/// The SAME value gets the same letter every time it appears, which is the one thing about an
/// opaque string that survives being spoken: a listener can hear that two mentions are the same
/// commit without hearing the commit.
fn name_opaque_strings(text: &str) -> String {
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    let mut numbers: BTreeMap<String, String> = BTreeMap::new();
    let mut out = String::with_capacity(text.len());

    for token in split_keeping_separators(text) {
        let word = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if word.is_empty() {
            out.push_str(&token);
            continue;
        }
        let replacement = if is_big_number(word) {
            Some(name_for(word, &mut numbers, "large number"))
        } else if is_hashlike(word) {
            Some(name_for(word, &mut hashes, "hash code"))
        } else {
            None
        };
        match replacement {
            Some(said) => out.push_str(&token.replacen(word, &said, 1)),
            None => out.push_str(&token),
        }
    }
    out
}

fn name_for(value: &str, seen: &mut BTreeMap<String, String>, kind: &str) -> String {
    if let Some(held) = seen.get(value) {
        return held.clone();
    }
    let letter = letter_for(seen.len());
    let said = format!("{kind} {letter}");
    seen.insert(value.to_owned(), said.clone());
    said
}

/// A, B, … Z, then AA. A message with twenty-seven distinct hashes in it has other problems.
fn letter_for(index: usize) -> String {
    let letter = char::from(b'A' + u8::try_from(index % 26).unwrap_or(0));
    if index < 26 {
        letter.to_string()
    } else {
        format!("{letter}{letter}")
    }
}

fn is_big_number(word: &str) -> bool {
    word.len() >= BIG_NUMBER_MIN_DIGITS && word.chars().all(|c| c.is_ascii_digit())
}

/// Hex-looking, long, and containing at least one digit.
///
/// The digit is what keeps an ordinary long word out: `unfortunately` is all letters and is a word;
/// `deadbeefcafe` is hex but so is `defaceable`, and requiring a digit costs almost no real hashes
/// while sparing the dictionary.
fn is_hashlike(word: &str) -> bool {
    word.len() >= HASH_MIN_CHARS
        && word.chars().all(|c| c.is_ascii_hexdigit())
        && word.chars().any(|c| c.is_ascii_digit())
}

fn split_keeping_separators(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            parts.push(ch.to_string());
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-24T12:00:00Z, so every "ago" below is measured from a fixed point.
    const NOW: i64 = 1_787_572_800_000;

    fn eastern() -> Zone {
        crate::clock::zone("America/New_York").expect("a bundled zone")
    }

    fn say(text: &str) -> String {
        for_speech(text, NOW, &eastern())
    }

    #[test]
    fn markdown_markers_are_not_read_out() {
        // The reported bug, verbatim: the voice said "asterisk asterisk" before every bold word.
        assert_eq!(say("the **deploy** is _done_"), "the deploy is done");
        assert_eq!(say("~~cancelled~~ and ||hidden||"), "cancelled and hidden");
        assert_eq!(say("## A heading"), "A heading");
        assert_eq!(say("> quoted text"), "quoted text");
        assert_eq!(say("run `make validate` now"), "run make validate now");
    }

    #[test]
    fn a_link_reads_as_its_words_and_never_as_its_url() {
        assert_eq!(
            say("see [the runbook](https://example.com/x)"),
            "see the runbook"
        );
        assert_eq!(say("<@123456789012345678> look"), "a mention look");
        assert_eq!(say("in <#987654321098765432>"), "in a channel");
    }

    #[test]
    fn a_short_snippet_is_spoken_and_a_long_one_is_named() {
        // Short: the snippet IS the message, and dropping it removes what the reader was shown.
        assert_eq!(say("```\ncargo test\n```"), "cargo test");
        // Long: a minute of spoken punctuation, so it is announced with its size instead.
        let long = "```rust\nlet a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\nlet e = 5;\n```";
        assert_eq!(say(long), "a rust code snippet, 5 lines long.");
        // ...and an unlabelled fence still says how big it is.
        let plain = "```\none\ntwo\nthree\nfour\n```";
        assert_eq!(say(plain), "a code snippet, 4 lines long.");
    }

    #[test]
    fn a_table_becomes_a_sentence_about_the_table() {
        let table = "\
| step | result |
|---|---|
| build | ok |
| test | ok |
| package | failed |";
        // Keys read, because they are what the table is ABOUT.
        assert_eq!(
            say(table),
            "a table with 2 columns and 3 rows, with keys build, test, package."
        );
    }

    #[test]
    fn a_table_of_identifiers_says_its_shape_and_stops() {
        // Reciting a first column of hashes is the wall of text the placeholder exists to remove.
        let table = "\
| commit | status |
|---|---|
| a1b2c3d4e5f6a7 | ok |
| f6e5d4c3b2a1f0 | ok |";
        assert_eq!(say(table), "a table with 2 columns and 2 rows.");
    }

    #[test]
    fn the_same_hash_twice_is_the_same_placeholder_and_a_different_one_is_not() {
        // The only thing about a hash that survives being spoken: whether it is the same one.
        let said = say("deployed a1b2c3d4e5f6a7 then reverted a1b2c3d4e5f6a7, not ffee0011223344");
        assert_eq!(
            said,
            "deployed hash code A then reverted hash code A, not hash code B"
        );
    }

    #[test]
    fn long_identifiers_are_named_and_ordinary_numbers_are_left_alone() {
        assert_eq!(say("id 1000000000000000009"), "id large number A");
        // A year, a port and a count are quantities somebody wants to hear.
        assert_eq!(
            say("in 2026 on port 8080, 42 times"),
            "in 2026 on port 8080, 42 times"
        );
    }

    #[test]
    fn precision_is_proportional_and_never_two_units_at_once() {
        // "3 hours and 15 seconds ago" is worse than "3 hours ago": the second term is noise at
        // that distance and the listener has to hold it to discard it.
        assert_eq!(say("at 2026-08-24T11:59:45Z"), "at 15 seconds ago");
        assert_eq!(say("at 2026-08-24T11:30:00Z"), "at 30 minutes ago");
        assert_eq!(say("at 2026-08-24T09:00:15Z"), "at 2 hours ago");
        assert_eq!(say("at 2026-08-22T12:00:00Z"), "at 2 days ago");
    }

    #[test]
    fn a_date_far_enough_back_is_said_as_a_date() {
        // Past a week, "N days ago" stops being something anybody can place.
        let said = say("at 2026-08-04T12:00:00Z");
        assert!(said.contains("August 4th"), "{said}");
        assert!(said.contains("a Tuesday"), "{said}");
        assert!(
            !said.contains("2026"),
            "the year is noise at this distance: {said}"
        );
    }

    #[test]
    fn past_a_year_the_year_comes_back() {
        let said = say("at 2024-03-05T12:00:00Z");
        assert!(said.contains("March 5th"), "{said}");
        assert!(
            said.contains("2024"),
            "a date this old needs its year: {said}"
        );
    }

    #[test]
    fn discords_own_timestamp_markup_is_resolved_too() {
        // <t:seconds:style>. The style is ignored: this decides how to say it.
        assert_eq!(say("shipped <t:1787569200:R>"), "shipped 1 hour ago");
    }

    #[test]
    fn a_time_is_resolved_before_a_long_number_can_claim_it() {
        // A Unix epoch and a snowflake are both long runs of digits, and only one is a time. If the
        // order were reversed this would read "large number A" and the time would be gone.
        assert_eq!(say("<t:1787569200:f>"), "1 hour ago");
    }

    #[test]
    fn the_absolute_reading_uses_the_operators_zone() {
        // 03:00Z on the 5th is still the 4th in New York, and saying the wrong day is worse than
        // saying a bare instant.
        let said = for_speech("at 2026-08-05T03:00:00Z", NOW, &eastern());
        assert!(said.contains("August 4th"), "{said}");
    }

    #[test]
    fn a_message_with_nothing_speakable_in_it_does_not_become_nonsense() {
        assert_eq!(say(""), "");
        assert_eq!(say("   \n\n  "), "");
        assert_eq!(say("**"), "");
    }
}
