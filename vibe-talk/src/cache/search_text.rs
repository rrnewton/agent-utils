//! `search_text`: what the full-text index holds for one message.
//!
//! The page searches what a message SAYS: its author, the text it draws (emphasis removed, link
//! labels such as `widgets#123`), and the addresses its links go to. The index chooses the
//! candidates, so it has to contain every one of those; re-matching in Rust cannot recover a
//! message the index never offered. So the column is the page's haystack, folded as the page folds
//! it (`#604 server-side-search`):
//!
//! ```text
//! fold(author + "\n" + drawn text + "\n" + link addresses, one a line)
//! ```
//!
//! [`crate::search_query::fold`] drops case, accents and the difference between runs of white
//! space, so the column, the query and the page's copy all compare the same way, and a trigram of
//! a folded term is a trigram of every message holding it. The raw content is not in it: the page
//! never searches Markdown source, so the server does not either. Capped at [`MAX_BYTES`]. How a body is rendered belongs to `crate::render`, which this module
//! does not depend on: the rendering comes in through [`SearchTextSource`], so the store can be
//! built and tested before the renderer is wired to it, and a renderer change is a new
//! [`SearchTextSource::version`] that the writer rebuilds the column for, in the background.

use crate::model::Message;

/// The most bytes of `search_text` kept per message. Slack allows 40,000 characters in one post;
/// past this cap a search can miss the message, and says its matching was by substring only.
pub const MAX_BYTES: usize = 8 * 1024;

/// A body as the page would draw it: its visible text and the addresses of its links.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// The text content of the drawn body: what a reader sees, with markup gone.
    pub text: String,
    /// Every link address in the drawn body, in order.
    pub hrefs: Vec<String>,
}

/// How a message body is rendered for the index.
pub trait SearchTextSource: Send + Sync {
    /// A label for the rendering rules. When it differs from the one a cache file was built
    /// with, the writer rebuilds every row's `search_text` with these rules.
    fn version(&self) -> String;

    /// Render one message from the provider `provider_key` the way the page draws it.
    fn rendered(&self, provider_key: &str, message: &Message) -> Rendered;
}

/// What the rules of [`build`] are called, beside the renderer's and the fold's in
/// [`stored_version`]: `haystack-1`, the page's haystack folded, with no raw content.
pub const BUILD_VERSION: &str = "haystack-1";

/// The version a file's `search_text` is stamped with: the renderer's rules, this module's and the
/// fold's. A change to any of them rebuilds the column in the background.
#[must_use]
pub fn stored_version(source: &dyn SearchTextSource) -> String {
    format!(
        "{}/{BUILD_VERSION}/{}",
        source.version(),
        crate::search_query::FOLD_VERSION
    )
}

/// The rendering before `crate::render` is wired in: the body exactly as written, and no links.
/// Every word of the content is still indexed; what it lacks is link labels and addresses that
/// appear only in the drawn form.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainText;

impl SearchTextSource for PlainText {
    fn version(&self) -> String {
        "plain-1".to_owned()
    }

    fn rendered(&self, _provider_key: &str, message: &Message) -> Rendered {
        Rendered {
            text: message.content.clone(),
            hrefs: Vec::new(),
        }
    }
}

/// Build the `search_text` column for one message from its author and its rendering.
#[must_use]
pub fn build(author: &str, rendered: &Rendered) -> String {
    let haystack = crate::search_query::haystack(author, &rendered.text, &rendered.hrefs);
    cap(crate::search_query::fold(&haystack)
        .trim_matches(' ')
        .to_owned())
}

/// Cut to at most [`MAX_BYTES`], at a character boundary.
fn cap(mut text: String) -> String {
    if text.len() > MAX_BYTES {
        let mut end = MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Whether a `search_text` was cut at [`MAX_BYTES`], so a search that relied on it can say its
/// matching past that point was not complete.
#[must_use]
pub fn was_capped(search_text: &str) -> bool {
    search_text.len() + 4 > MAX_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_is_the_pages_haystack_folded_and_not_the_markdown_source() {
        let rendered = Rendered {
            text: "Fixed widgets#123 Today\nat the Café".to_owned(),
            hrefs: vec!["https://Example.test/Issue/123".to_owned()],
        };
        let text = build("Ada Lovelace", &rendered);
        assert_eq!(
            text,
            "ada lovelace fixed widgets#123 today at the cafe https://example.test/issue/123"
        );
        assert_eq!(
            text,
            crate::search_query::fold(&text),
            "folding it again changes nothing"
        );
        let raw = build(
            "a",
            &Rendered {
                text: "bold".to_owned(),
                hrefs: Vec::new(),
            },
        );
        assert!(!raw.contains("**"), "the source's marks are no part of it");
    }

    #[test]
    fn the_text_is_capped_at_a_character_boundary() {
        let long = Rendered {
            text: "é".repeat(MAX_BYTES),
            hrefs: Vec::new(),
        };
        let text = build("a", &long);
        assert!(text.len() <= MAX_BYTES);
        assert!(text.len() > MAX_BYTES - 4);
        assert!(was_capped(&text));
        let short = Rendered {
            text: "short".to_owned(),
            hrefs: Vec::new(),
        };
        assert!(!was_capped(&build("a", &short)));
    }

    #[test]
    fn the_stored_version_names_the_renderer_the_haystack_and_the_fold() {
        assert_eq!(
            stored_version(&PlainText),
            format!("plain-1/haystack-1/{}", crate::search_query::FOLD_VERSION)
        );
    }

    #[test]
    fn plain_text_renders_the_body_as_written() {
        let message = crate::cache::fixtures::message("1", "u", "Hello *there*", 1_000);
        let rendered = PlainText.rendered("p", &message);
        assert_eq!(rendered.text, "Hello *there*");
        assert!(rendered.hrefs.is_empty());
        assert_eq!(PlainText.version(), "plain-1");
    }
}
