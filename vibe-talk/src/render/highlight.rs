//! Code blocks highlighted on the server. `#531 server-side-syntax-highlight`.
//!
//! A fenced block that names a language this knows is written as its code with each token in a
//! `<span>` whose class says what KIND of token it is — [`CLASSES`], a short fixed list — and the
//! stylesheet colours the kinds, once for each scheme. The page receives no colour, so a block
//! looks right in the light and the dark theme alike, and the sanitizer allows exactly these
//! classes and nothing else.
//!
//! A block with no language, or one this does not know, is plain monospace text, exactly as it
//! was before: the language is never guessed from the code.
//!
//! # How a token gets its class
//!
//! syntect parses the code with a Sublime Text grammar, which names every token with a stack of
//! TextMate scopes (`source.rust meta.function keyword.other.rust`). Colour themes are written
//! against those scopes, and syntect already knows how to choose the rule a scope stack matches
//! best. So the kinds are written as a theme ([`RULES`]): each rule's "colour" is not a colour at
//! all but the number of its class, and the highlighter's answer for a token is read back as an
//! index into [`CLASSES`]. Zero is no class.
//!
//! # The grammars
//!
//! `build.rs` compiles them into one syntax set at build time: a subset of two-face's (bat's
//! collection) and the grammars in `syntaxes/`. [`LANGUAGES`] lists every language a fence can
//! name, and a test holds the list and the set to each other.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::str::FromStr;
use std::sync::LazyLock;

use comrak::adapters::SyntaxHighlighterAdapter;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, ScopeSelectors, StyleModifier, Theme, ThemeItem};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Every class a highlighted token may carry. The sanitizer allows these on a `span` and the
/// stylesheet colours them; both read this list. Short, because a block holds one for every token.
pub const CLASSES: [&str; 17] = [
    "hl-c",  // comment
    "hl-s",  // string, and inline code in Markdown
    "hl-n",  // number and other constant: `true`, `None`, a character
    "hl-k",  // keyword, and storage words such as `fn`, `let`, `static`
    "hl-t",  // type, class, trait
    "hl-f",  // function name, at its definition or a call of a known one
    "hl-e",  // escape and placeholder inside a string: `\n`, `{}`
    "hl-p",  // property: a mapping key, a tag, an attribute name
    "hl-m",  // macro, attribute, annotation, decorator, preprocessor line
    "hl-v",  // special variable: `self`, `this`, a shell `$VAR`
    "hl-gi", // diff: inserted line
    "hl-gd", // diff: deleted line
    "hl-gh", // diff: file header and hunk range
    "hl-h",  // Markdown heading
    "hl-b",  // Markdown bold
    "hl-i",  // Markdown italic
    "hl-u",  // Markdown link address
];

/// Which class a scope selector gives a token: an index into [`CLASSES`] plus one, zero for none.
/// Where two rules match, syntect takes the more specific: `keyword.operator` over `keyword`.
const RULES: &[(&str, u8)] = &[
    ("comment", 1),
    ("markup.quote", 1),
    ("string", 2),
    ("markup.raw", 2),
    ("constant", 3),
    ("support.constant", 3),
    ("keyword", 4),
    ("storage", 4),
    ("punctuation.definition.list_item", 4),
    // Operators and punctuation stay the text's own colour: colouring every `=` and `.` is noise.
    ("keyword.operator", 0),
    // Except the operators that are words: `and`, `not`, `in`, `instanceof`.
    ("keyword.operator.word", 4),
    ("keyword.operator.logical.python", 4),
    ("entity.name.type", 5),
    ("entity.name.class", 5),
    ("entity.name.struct", 5),
    ("entity.name.enum", 5),
    ("entity.name.union", 5),
    ("entity.name.trait", 5),
    ("entity.name.interface", 5),
    ("entity.other.inherited-class", 5),
    ("entity.name.impl", 5),
    ("entity.name.namespace", 5),
    ("entity.name.table", 5),
    ("support.type", 5),
    ("support.class", 5),
    ("support.other.module", 5),
    // Where a grammar marks type NAMES as storage, not only the words that declare them.
    ("source.haskell storage.type", 5),
    ("source.ocaml storage.type", 5),
    ("entity.name.function", 6),
    ("support.function", 6),
    ("variable.function", 6),
    ("constant.character.escape", 7),
    ("constant.other.placeholder", 7),
    ("entity.name.tag", 8),
    ("entity.other.attribute-name", 8),
    ("meta.mapping.key string", 8),
    ("support.type.property-name", 8),
    ("meta.preprocessor", 9),
    ("meta.annotation", 9),
    ("meta.attribute", 9),
    ("support.macro", 9),
    ("entity.name.macro", 9),
    ("entity.name.function.decorator", 9),
    ("variable.language", 10),
    ("source.shell variable.other", 10),
    ("markup.inserted", 11),
    ("markup.deleted", 12),
    ("meta.diff", 13),
    ("markup.heading", 14),
    ("markup.bold", 15),
    ("markup.italic", 16),
    ("markup.underline.link", 17),
];

/// Every language a fence may name, and the words that name it. The first word is the grammar's
/// own name for it, which is also how a test finds the grammar. Other words a grammar knows itself
/// — its file extensions, `rs` or `yml` — work too; these are the ones it does not.
pub const LANGUAGES: &[(&str, &[&str])] = &[
    ("Agda", &["agda"]),
    ("Bourne Again Shell (bash)", &["bash", "sh", "shell", "zsh"]),
    ("C", &["c", "h"]),
    ("C++", &["cpp", "c++", "cc", "cxx", "hpp"]),
    ("Diff", &["diff", "patch"]),
    ("Go", &["go", "golang"]),
    ("Haskell", &["haskell", "hs"]),
    ("JSON", &["json", "jsonc", "json5"]),
    ("JavaScript", &["javascript", "js", "mjs", "cjs"]),
    ("Lean 4", &["lean", "lean4"]),
    ("Markdown", &["markdown", "md"]),
    ("OCaml", &["ocaml", "ml", "mli"]),
    ("Python", &["python", "py", "python3", "py3"]),
    ("Rocq", &["rocq", "coq", "v"]),
    ("Rust", &["rust", "rs"]),
    ("SQL", &["sql"]),
    ("TOML", &["toml"]),
    ("TypeScript", &["typescript", "ts"]),
    ("TypeScriptReact", &["tsx", "jsx"]),
    ("YAML", &["yaml", "yml"]),
];

/// The compiled grammars, read on the first block that names a language.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(|| {
    syntect::dumps::from_binary(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/syntaxes.packdump"
    )))
});

/// [`LANGUAGES`]' words, lowercase, to the grammar each names.
static NAMES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    LANGUAGES
        .iter()
        .flat_map(|(grammar, words)| words.iter().map(move |word| (*word, *grammar)))
        .collect()
});

/// [`RULES`] as a theme whose colours are class numbers. See the module note.
static THEME: LazyLock<Theme> = LazyLock::new(|| Theme {
    scopes: RULES
        .iter()
        .map(|&(selector, class)| ThemeItem {
            scope: ScopeSelectors::from_str(selector).expect("every rule is a valid selector"),
            style: StyleModifier {
                foreground: Some(Color {
                    r: class,
                    g: 0,
                    b: 0,
                    a: 0xFF,
                }),
                background: None,
                font_style: None,
            },
        })
        .collect(),
    ..Theme::default()
});

/// The grammar a fence's language names, or `None` for no language or one not known.
///
/// The language is the info string's first word, as comrak hands it over, up to any comma, so
/// `rust,ignore` is Rust. Matched without regard to case.
#[must_use]
pub fn syntax_for(language: &str) -> Option<&'static SyntaxReference> {
    let word = language.split(',').next().unwrap_or_default().trim();
    if word.is_empty() {
        return None;
    }
    let lower = word.to_ascii_lowercase();
    match NAMES.get(lower.as_str()) {
        Some(grammar) => SYNTAXES.find_syntax_by_name(grammar),
        None => SYNTAXES
            .find_syntax_by_extension(&lower)
            .filter(|syntax| !syntax.name.is_empty()),
    }
}

/// `code` written as HTML with each token in a class span, or `None` when the grammar fails on it
/// (the caller then writes it plain). Text escaped as comrak escapes it.
#[must_use]
pub fn highlighted(code: &str, syntax: &SyntaxReference) -> Option<String> {
    let mut lines = HighlightLines::new(syntax, &THEME);
    let mut out = Spans::default();
    for line in LinesWithEndings::from(code) {
        for (style, text) in lines.highlight_line(line, &SYNTAXES).ok()? {
            out.push(usize::from(style.foreground.r), text).ok()?;
        }
    }
    out.finish().ok()
}

/// The HTML [`highlighted`] writes, one token at a time.
///
/// Blank space is held back until the next token: if that is of the kind already open, the space
/// goes inside its span, so `let mut x` is one span and not two; otherwise it goes between them.
/// Blank space never opens a span of its own.
#[derive(Default)]
struct Spans {
    html: String,
    /// The class open now, as an index into [`CLASSES`] plus one; zero for none.
    open: usize,
    /// Blank space not yet written.
    held: String,
}

impl Spans {
    fn push(&mut self, class: usize, text: &str) -> fmt::Result {
        if text.trim().is_empty() {
            self.held.push_str(text);
            return Ok(());
        }
        let class = if class.checked_sub(1).is_some_and(|at| at < CLASSES.len()) {
            class
        } else {
            0
        };
        if class != self.open {
            self.close();
            comrak::html::escape(&mut self.html, &std::mem::take(&mut self.held))?;
            if let Some(name) = class.checked_sub(1).map(|at| CLASSES[at]) {
                self.html.push_str("<span class=\"");
                self.html.push_str(name);
                self.html.push_str("\">");
            }
            self.open = class;
        }
        comrak::html::escape(&mut self.html, &std::mem::take(&mut self.held))?;
        comrak::html::escape(&mut self.html, text)
    }

    fn close(&mut self) {
        if self.open != 0 {
            self.html.push_str("</span>");
            self.open = 0;
        }
    }

    fn finish(mut self) -> Result<String, fmt::Error> {
        self.close();
        comrak::html::escape(&mut self.html, &self.held)?;
        Ok(self.html)
    }
}

/// The adapter comrak writes every code block through. `pre` and `code` carry nothing: the
/// language is not shown, and a highlighted block differs from a plain one only inside `code`.
pub struct Highlighter;

impl SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn Write,
        lang: Option<&str>,
        code: &str,
    ) -> fmt::Result {
        match lang
            .and_then(syntax_for)
            .and_then(|syntax| highlighted(code, syntax))
        {
            Some(html) => output.write_str(&html),
            None => comrak::html::escape(output, code),
        }
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn Write,
        _attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        output.write_str("<pre>")
    }

    fn write_code_tag(
        &self,
        output: &mut dyn Write,
        _attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        output.write_str("<code>")
    }
}

#[cfg(test)]
mod tests {
    use super::{syntax_for, CLASSES, LANGUAGES, SYNTAXES};
    use crate::render::{to_html, Markup};

    /// One small sample per language, and the classes its rendering must hold.
    const SAMPLES: &[(&str, &str, &[&str])] = &[
        (
            "rust",
            "#[derive(Debug)]\nfn main() {\n    let s = \"hi\\n\"; // note\n}",
            &["hl-m", "hl-k", "hl-f", "hl-s", "hl-e", "hl-c"],
        ),
        (
            "python",
            "def f(x):\n    return \"s\" and None  # note",
            &["hl-k", "hl-f", "hl-s", "hl-n", "hl-c"],
        ),
        (
            "typescript",
            "const x: number = 1; // note",
            &["hl-k", "hl-t", "hl-n", "hl-c"],
        ),
        (
            "tsx",
            "const a = <div className=\"x\">hi</div>;",
            &["hl-k", "hl-p", "hl-s"],
        ),
        (
            "javascript",
            "function f() { return this.x || 'a'; }",
            &["hl-k", "hl-f", "hl-v", "hl-s"],
        ),
        ("bash", "echo \"$HOME\" # note", &["hl-f", "hl-s", "hl-v", "hl-c"]),
        ("json", "{\"a\": 1, \"b\": \"s\"}", &["hl-p", "hl-n", "hl-s"]),
        ("yaml", "key: value # note", &["hl-p", "hl-s", "hl-c"]),
        (
            "toml",
            "[package]\nname = \"x\" # note",
            &["hl-t", "hl-p", "hl-s", "hl-c"],
        ),
        (
            "diff",
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new",
            &["hl-gh", "hl-gd", "hl-gi"],
        ),
        (
            "c",
            "#include <stdio.h>\nint main(void) { return 0; }",
            &["hl-k", "hl-f", "hl-n", "hl-s"],
        ),
        ("cpp", "template <typename T> class A {};", &["hl-k", "hl-t"]),
        (
            "go",
            "func main() { fmt.Println(\"x\") }",
            &["hl-k", "hl-f", "hl-s"],
        ),
        ("sql", "SELECT a FROM t WHERE b = 'x';", &["hl-k", "hl-s"]),
        (
            "markdown",
            "# Title\n\n- **bold** and _it_ `code` [a](https://example.com)",
            &["hl-h", "hl-k", "hl-b", "hl-i", "hl-s", "hl-u"],
        ),
        (
            "haskell",
            "f :: Int -> Maybe Int\nf x = Just (x + 1) -- note\nmain = putStrLn \"hi\"",
            &["hl-f", "hl-k", "hl-t", "hl-n", "hl-s", "hl-c"],
        ),
        (
            "ocaml",
            "let rec f (x : int) = if x > 0 then \"a\" else \"b\" (* note *)",
            &["hl-k", "hl-f", "hl-t", "hl-s", "hl-c"],
        ),
        (
            "lean",
            "theorem foo : 1 + 1 = 2 := by rfl -- note",
            &["hl-k", "hl-f", "hl-n", "hl-c"],
        ),
        (
            "agda",
            "data Nat : Set where\n  zero : Nat -- note\n{- block -}",
            &["hl-k", "hl-t", "hl-n", "hl-c"],
        ),
        (
            "rocq",
            "Theorem t : forall n : nat, n = n.\nProof. intros. reflexivity. Qed. (* note (* nested *) *)",
            &["hl-k", "hl-f", "hl-t", "hl-c"],
        ),
    ];

    fn fenced(language: &str, code: &str) -> String {
        to_html(&format!("```{language}\n{code}\n```"), Markup::Discord)
    }

    /// Every class in `html`, in order of first appearance.
    fn classes(html: &str) -> Vec<&str> {
        let mut found = Vec::new();
        for piece in html.split("<span class=\"").skip(1) {
            let class = &piece[..piece.find('"').expect("a closing quote")];
            if !found.contains(&class) {
                found.push(class);
            }
        }
        found
    }

    #[test]
    fn every_language_draws_its_kinds_of_token() {
        for (language, code, want) in SAMPLES {
            let html = fenced(language, code);
            assert!(html.starts_with("<pre><code>"), "{language}: {html}");
            let have = classes(&html);
            for class in *want {
                assert!(have.contains(class), "{language}: no {class} in {html}");
            }
            for class in &have {
                assert!(CLASSES.contains(class), "{language}: {class} is not listed");
            }
        }
    }

    #[test]
    fn every_sample_language_is_one_of_the_languages() {
        for (grammar, words) in LANGUAGES {
            assert!(
                SAMPLES
                    .iter()
                    .any(|(language, ..)| words.contains(language)),
                "{grammar} has no sample"
            );
        }
    }

    /// The list and the compiled set name the same languages: every listed one is in the set, and
    /// every grammar in the set that a fence can reach is listed, but for the ones only included
    /// by others and Plain Text.
    #[test]
    fn the_list_and_the_grammars_agree() {
        for (grammar, words) in LANGUAGES {
            for word in *words {
                let found = syntax_for(word).unwrap_or_else(|| panic!("{word} finds nothing"));
                assert_eq!(found.name, *grammar, "{word}");
            }
        }
        let included = [
            "commands-builtin-shell-bash",
            "Regular Expressions (Javascript)",
            "Regular Expressions (Python)",
            "Plain Text",
        ];
        let reachable: Vec<&str> = SYNTAXES
            .syntaxes()
            .iter()
            .map(|syntax| syntax.name.as_str())
            .filter(|name| !name.is_empty() && !included.contains(name))
            .collect();
        for name in &reachable {
            assert!(
                LANGUAGES.iter().any(|(grammar, _)| grammar == name),
                "{name} is compiled in but not listed"
            );
        }
        assert_eq!(reachable.len(), LANGUAGES.len());
    }

    #[test]
    fn a_language_is_matched_without_case_and_before_a_comma() {
        assert_eq!(syntax_for("Rust").map(|s| s.name.as_str()), Some("Rust"));
        assert_eq!(
            syntax_for("rust,ignore").map(|s| s.name.as_str()),
            Some("Rust")
        );
        assert_eq!(syntax_for("PY").map(|s| s.name.as_str()), Some("Python"));
    }

    #[test]
    fn an_unknown_or_missing_language_is_plain_code() {
        let code = "fn main() { let x = 1; }";
        let plain = "<pre><code>fn main() { let x = 1; }\n</code></pre>\n";
        assert_eq!(fenced("", code), plain);
        assert_eq!(fenced("klingon", code), plain);
        // Nothing is guessed from the code: a shebang names no language.
        assert_eq!(
            fenced("", "#!/bin/bash\necho hi"),
            "<pre><code>#!/bin/bash\necho hi\n</code></pre>\n"
        );
        // An indented block has no language at all.
        assert_eq!(
            to_html("text\n\n    let x = 1;", Markup::Discord),
            "<p>text</p>\n<pre><code>let x = 1;\n</code></pre>\n"
        );
        // A grammar left out of the set is not reachable by its extension.
        assert!(syntax_for("php").is_none());
        assert!(syntax_for("txt").is_none_or(|syntax| syntax.name == "Plain Text"));
    }

    #[test]
    fn markup_in_highlighted_code_stays_text() {
        let html = fenced(
            "js",
            "const s = \"<script>alert('&amp;')</script>\"; // <b>not bold</b>",
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
        assert!(
            html.contains("&lt;script&gt;alert('&amp;amp;')&lt;/script&gt;"),
            "{html}"
        );
        assert!(html.contains("&lt;b&gt;not bold&lt;/b&gt;"), "{html}");
        // A quote, unescaped in text as the sanitizer writes it.
        assert!(
            html.contains("<span class=\"hl-s\">\"&lt;script&gt;"),
            "{html}"
        );
    }

    #[test]
    fn blank_space_opens_no_span_and_joins_one_kind() {
        let html = fenced("rust", "pub fn f() {}");
        assert!(
            html.contains("<span class=\"hl-k\">pub fn</span> <span class=\"hl-f\">f</span>"),
            "{html}"
        );
    }

    #[test]
    fn a_body_highlighting_would_grow_too_far_keeps_its_code_block() {
        // A span for every one-character token: far past the growth bound highlighted.
        let code = format!("[{}1]", "1,".repeat(2_000));
        let html = fenced("json", &code);
        assert_eq!(html, format!("<pre><code>{code}\n</code></pre>\n"));
    }

    #[test]
    fn highlighting_is_inside_the_render_bounds() {
        // A 16 KiB body, the most rendered, of dense Rust: highlighted, and in a debug build still
        // well inside a second. The release-build figure is in the README's rendering section.
        let line = "    let value = compute(\"text\", 42) + other.field; // note\n";
        let code: String = line.repeat(16 * 1024 / line.len() - 1);
        let text = format!("```rust\n{code}```");
        assert!(text.len() <= crate::render::MAX_RENDERED_BYTES);
        let started = std::time::Instant::now();
        let html = to_html(&text, Markup::Discord);
        assert!(html.contains("hl-k"), "{}", &html[..200]);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    /// The numbers in the README's rendering section. A report, not a check: run it in a release
    /// build, `cargo test --release --lib render_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "a timing report for a release build"]
    fn render_timing() {
        use std::time::{Duration, Instant};

        fn median(mut runs: Vec<Duration>) -> Duration {
            runs.sort();
            runs[runs.len() / 2]
        }
        fn uncached(text: &str) -> String {
            crate::render::render_visiting(text, Markup::Discord, &[], |_, _| ()).0
        }

        // Two hundred lines of real Rust: this file's own.
        let source: String = include_str!("highlight.rs")
            .lines()
            .skip(120)
            .take(200)
            .map(|line| format!("{line}\n"))
            .collect();
        let rust = format!("```rust\n{source}```");
        let unknown = format!("```klingon\n{source}```");

        let started = Instant::now();
        let first = uncached(&rust);
        println!(
            "200-line Rust block, first in the process: {:?}",
            started.elapsed()
        );
        let time = |text: &str| {
            median(
                (0..21)
                    .map(|_| {
                        let started = Instant::now();
                        std::hint::black_box(uncached(text));
                        started.elapsed()
                    })
                    .collect(),
            )
        };
        println!(
            "200-line Rust block, highlighted: {:?}; unknown language, plain: {:?}; {} bytes of \
             text, {} of HTML",
            time(&rust),
            time(&unknown),
            rust.len(),
            first.len()
        );
        // The most one body can cost: 16 KiB, the longest rendered, all of it dense code.
        let line = "    let value = compute(\"text\", 42) + other.field; // note\n";
        let dense = format!("```rust\n{}```", line.repeat(16 * 1024 / line.len() - 1));
        println!("16 KiB body of dense Rust, highlighted: {:?}", time(&dense));
        let _ = crate::render::to_html(&rust, Markup::Discord);
        let started = Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(crate::render::to_html(&rust, Markup::Discord));
        }
        println!(
            "200-line Rust block, from the render cache: {:?}",
            started.elapsed() / 1000
        );

        // A timeline window: a hundred messages, one in ten carrying a twenty-line code block.
        let window: Vec<String> = (0..100)
            .map(|at| {
                if at % 10 == 0 {
                    let code: String = source
                        .lines()
                        .skip(at)
                        .take(20)
                        .map(|l| format!("{l}\n"))
                        .collect();
                    format!("Here is the change {at}:\n\n```rust\n{code}```\nIt **builds**.")
                } else {
                    format!(
                        "Message {at}: the gate passed on https://example.com/runs/{at}, and \
                         **two** steps were _skipped_.\n- one\n- two"
                    )
                }
            })
            .collect();
        let fill = |render: &dyn Fn(&str) -> String| {
            median(
                (0..11)
                    .map(|_| {
                        let started = Instant::now();
                        for text in &window {
                            std::hint::black_box(render(text));
                        }
                        started.elapsed()
                    })
                    .collect(),
            )
        };
        let plain_window: Vec<String> = window
            .iter()
            .map(|text| text.replace("```rust", "```klingon"))
            .collect();
        let unhighlighted = median(
            (0..11)
                .map(|_| {
                    let started = Instant::now();
                    for text in &plain_window {
                        std::hint::black_box(uncached(text));
                    }
                    started.elapsed()
                })
                .collect(),
        );
        println!(
            "100-message window: rendered, unhighlighted (as before) {:?}; rendered, highlighted \
             {:?}; from the render cache {:?}",
            unhighlighted,
            fill(&uncached),
            fill(&|text| crate::render::to_html(text, Markup::Discord)),
        );
    }
}
