//! Mention syntax made inert in a post to a provider that would notify someone with it.
//!
//! `#445 gchat-mention-defuse`. A Google Chat bridge refuses a post whose text holds syntax Google
//! Chat could expand into a mention — `<users/…>`, which notifies that person, and a bare `@word`
//! — with HTTP 400, rather than risk paging someone the writer only named. The owner's sends were
//! refused for it, again and again: text quoted from a Google Chat message reads `@Ada Lovelace`
//! where that message mentioned her, and a reply that addresses someone, more usual in a thread
//! than in the conversation itself, writes the same.
//!
//! So the text is defused before it is posted, and the post goes: a WORD JOINER (U+2060), which
//! draws as nothing and joins as nothing, goes after the `@` of each `@word` that would match and
//! after the `<` of each `<users/`. The reader sees the words as written, and nobody is notified.
//! Only where the channel's provider says it needs it ([`crate::chat::ChatClient::defuses_mentions`]).
//!
//! What counts is exactly what such a bridge refuses — the pattern
//! `(?<![A-Za-z0-9_@.])@[A-Za-z][A-Za-z0-9._-]*|<users/[^>]+>`, case-insensitively — so an e-mail
//! address (`ada@example.com`, its `@` after a letter) and Discord's own `<@123>` are left alone.
//! Case-insensitive the way Python's `re` is, which also takes `İ`, `ı`, `ſ` and the Kelvin sign
//! `K` for letters; see [`is_letter`].
//!
//! Defused once, never twice: a defused `@` is followed by the joiner, not a letter, so defusing
//! the result again changes nothing. That matters because a post is defused BEFORE it is cut in
//! parts — the joiners count toward the provider's limit — and the unsent text a 207 hands back
//! comes back through here when it is sent again, where its parts must be keyed exactly as before.

use std::borrow::Cow;

/// U+2060 WORD JOINER: no width, no break, and not a letter, so it parts `@` from the name.
pub const WORD_JOINER: char = '\u{2060}';

/// Whether `c` is a letter to the pattern: ASCII, plus the four characters Python's
/// case-insensitive matching folds onto one — `İ` and `ı` (i), `ſ` (s), `K` (k).
fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
}

/// Whether an `@` straight after `c` cannot open a mention: after a letter, a digit, `_`, `@` or
/// `.` it is part of a word, an address or a run of at-signs.
fn joins_previous(c: char) -> bool {
    is_letter(c) || c.is_ascii_digit() || matches!(c, '_' | '@' | '.')
}

/// Whether `c` may continue a mention's name after its first letter.
fn continues_name(c: char) -> bool {
    is_letter(c) || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
}

/// Whether `rest`, the text after a `<`, starts with `users/` in any case.
fn opens_user_token(rest: &str) -> bool {
    let mut chars = rest.chars();
    for want in "users/".chars() {
        match chars.next() {
            Some(c) if c.eq_ignore_ascii_case(&want) => {}
            Some('\u{17f}') if want == 's' => {}
            _ => return false,
        }
    }
    true
}

/// Each place a mention would start in `text`, as the byte offset of its `@` or `<`.
fn notifying_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    let mut previous: Option<char> = None;
    text.char_indices().filter_map(move |(at, c)| {
        let before = previous.replace(c);
        let rest = &text[at + c.len_utf8()..];
        let opens = match c {
            '@' => {
                !before.is_some_and(joins_previous) && rest.chars().next().is_some_and(is_letter)
            }
            // The pattern wants a `>` after at least one character; one that never closes is
            // defused all the same, which only adds a joiner nobody sees.
            '<' => opens_user_token(rest),
            _ => false,
        };
        opens.then_some(at)
    })
}

/// `text` with every mention a Google Chat bridge would refuse made inert, borrowed unchanged when
/// it holds none.
#[must_use]
pub fn defuse(text: &str) -> Cow<'_, str> {
    let mut starts = notifying_starts(text).peekable();
    if starts.peek().is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 3 * 4);
    let mut from = 0;
    for at in starts {
        // `@` and `<` are one byte each.
        out.push_str(&text[from..=at]);
        out.push(WORD_JOINER);
        from = at + 1;
    }
    out.push_str(&text[from..]);
    Cow::Owned(out)
}

/// The first mention in `text` such a bridge would refuse, as written, for telling the reader
/// what the refusal was about: `@Ada` (without a full stop the name ran into), or the whole
/// `<users/…>` token, cut short when it is long.
#[must_use]
pub fn first_notifying(text: &str) -> Option<String> {
    let at = notifying_starts(text).next()?;
    let rest = &text[at..];
    if let Some(name) = rest.strip_prefix('@') {
        let end = name
            .char_indices()
            .find(|(_, c)| !continues_name(*c))
            .map_or(rest.len(), |(index, _)| index + 1);
        return Some(rest[..end].trim_end_matches(['.', '-']).to_owned());
    }
    let token = rest.find('>').map_or(rest, |end| &rest[..=end]);
    if token.chars().count() <= 40 {
        return Some(token.to_owned());
    }
    let cut: String = token.chars().take(39).collect();
    Some(format!("{cut}…"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WJ: &str = "\u{2060}";

    #[test]
    fn a_bare_at_name_gets_a_joiner_after_its_at_sign() {
        assert_eq!(defuse("@claude look"), format!("@{WJ}claude look"));
        assert_eq!(
            defuse("thanks (@Ada.Lovelace) and @bob-2"),
            format!("thanks (@{WJ}Ada.Lovelace) and @{WJ}bob-2")
        );
        assert_eq!(
            defuse("> @Ada Lovelace said"),
            format!("> @{WJ}Ada Lovelace said")
        );
        assert_eq!(defuse("line\n@here"), format!("line\n@{WJ}here"));
        assert_eq!(defuse("`@code`"), format!("`@{WJ}code`"));
    }

    #[test]
    fn a_user_token_gets_a_joiner_after_its_bracket_whatever_its_case() {
        assert_eq!(defuse("cc <users/123>"), format!("cc <{WJ}users/123>"));
        assert_eq!(defuse("<USERS/abc>"), format!("<{WJ}USERS/abc>"));
        assert_eq!(defuse("<u\u{17f}ers/1>"), format!("<{WJ}u\u{17f}ers/1>"));
    }

    #[test]
    fn what_the_bridge_lets_through_is_left_alone() {
        for text in [
            "ada@example.com",
            "a.@b",
            "x_@y",
            "@@twice",
            "@ space",
            "@1digit",
            "@",
            "<@123> and <@!456> and <#789>",
            "<user/1> <users> <usersX/1>",
            "nothing here",
            "",
        ] {
            assert!(
                matches!(defuse(text), Cow::Borrowed(_)),
                "{text:?} became {:?}",
                defuse(text)
            );
        }
    }

    #[test]
    fn python_folded_letters_count_as_letters_on_both_sides() {
        // Opens a mention: the first character after `@` folds to a letter.
        assert_eq!(defuse("@\u{17f}am"), format!("@{WJ}\u{17f}am"));
        assert_eq!(defuse("@\u{212a}elvin"), format!("@{WJ}\u{212a}elvin"));
        // Does not: the character before `@` folds to one.
        assert!(matches!(defuse("\u{131}@word"), Cow::Borrowed(_)));
        // A letter outside those is not one to the pattern, before or after.
        assert_eq!(defuse("é@word"), format!("é@{WJ}word"));
        assert!(matches!(defuse("@éclair"), Cow::Borrowed(_)));
    }

    #[test]
    fn defusing_twice_is_defusing_once() {
        let text = "@a @b <users/1> (@c) x@y <@1>";
        let once = defuse(text).into_owned();
        assert_eq!(defuse(&once), once.as_str());
        assert!(first_notifying(&once).is_none(), "{once:?}");
        // The words read the same with the joiners taken out.
        assert_eq!(once.replace(WORD_JOINER, ""), text);
    }

    #[test]
    fn the_first_mention_is_named_as_written() {
        assert_eq!(first_notifying("hi @Ada. ok").as_deref(), Some("@Ada"));
        assert_eq!(
            first_notifying("x@y and @bob_2-").as_deref(),
            Some("@bob_2")
        );
        assert_eq!(
            first_notifying("cc <users/123> @x").as_deref(),
            Some("<users/123>")
        );
        let long = format!("<users/{}>", "9".repeat(60));
        let named = first_notifying(&long).expect("named");
        assert_eq!(named.chars().count(), 40);
        assert!(named.ends_with('…'));
        assert_eq!(first_notifying("ada@example.com"), None);
    }
}
