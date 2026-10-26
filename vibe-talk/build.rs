//! The grammars the code-block highlighter knows, compiled into one syntax set at build time.
//! `#531 server-side-syntax-highlight`; see `src/render/highlight.rs` for how they are used.
//!
//! The set is a SUBSET: two-face's grammars (bat's collection, a superset of syntect's own) cost
//! about a megabyte compressed, and most of them name languages nobody pastes into a chat. This
//! keeps the languages in [`KEEP`], adds the grammars in `syntaxes/` that neither collection has,
//! and writes the result where `src/render/highlight.rs` includes it. A grammar that embeds one
//! left out (Markdown's fenced blocks, say) draws that part as plain text, which is syntect's
//! documented answer to a missing embedded scope.
//!
//! Nothing here runs at serving time: the server deserializes the finished set once, on the first
//! code block it highlights.

use std::path::{Path, PathBuf};

use syntect::parsing::{SyntaxDefinition, SyntaxSetBuilder};

/// The two-face grammars kept, by name. A name that is missing fails the build, so a two-face
/// upgrade that renames one cannot silently drop a language.
const KEEP: &[&str] = &[
    "Bourne Again Shell (bash)",
    // Bash's own grammar includes this one for its builtin commands.
    "commands-builtin-shell-bash",
    "C",
    "C++",
    "Diff",
    "Go",
    "Haskell",
    "JSON",
    "JavaScript",
    "Lean 4",
    "Markdown",
    "OCaml",
    "Python",
    // Included by the Python and JavaScript grammars for the insides of regular expressions.
    "Regular Expressions (Javascript)",
    "Regular Expressions (Python)",
    "Rust",
    "SQL",
    "TOML",
    "TypeScript",
    "TypeScriptReact",
    "YAML",
];

/// The grammars in `syntaxes/`, which neither syntect nor two-face has. Their sources and licences
/// are in `syntaxes/README.md`.
const LOCAL: &[&str] = &["Agda.sublime-syntax", "Rocq.sublime-syntax"];

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    println!("cargo::rerun-if-changed=build.rs");

    // The grammars arrive LINKED: a reference from one context to another is already an index
    // into this list, so a grammar cannot simply be left out without every index after it
    // pointing at the wrong one. A grammar not kept stays in its place as a husk instead, its
    // contexts emptied and its name and extensions cleared so no fence finds it. A reference into
    // a husk reaches an empty context, which matches nothing: plain text.
    let mut builder = SyntaxSetBuilder::new();
    let mut found = Vec::new();
    for mut syntax in two_face::syntax::extra_newlines()
        .into_builder()
        .syntaxes()
        .iter()
        .cloned()
    {
        if KEEP.contains(&syntax.name.as_str()) || syntax.name == "Plain Text" {
            found.push(syntax.name.clone());
        } else {
            syntax.name = String::new();
            syntax.file_extensions.clear();
            syntax.first_line_match = None;
            syntax.hidden = true;
            syntax.variables.clear();
            for context in syntax.contexts.values_mut() {
                *context = syntect::parsing::syntax_definition::Context::new(false);
            }
        }
        builder.add(syntax);
    }
    let missing: Vec<&&str> = KEEP
        .iter()
        .filter(|name| !found.iter().any(|have| have == *name))
        .collect();
    assert!(
        missing.is_empty(),
        "two-face has no grammar named {missing:?}"
    );

    for file in LOCAL {
        let path = manifest.join("syntaxes").join(file);
        println!("cargo::rerun-if-changed={}", path.display());
        builder.add(load(&path));
    }

    let set = builder.build();
    let dump = syntect::dumps::dump_binary(&set);
    std::fs::write(out.join("syntaxes.packdump"), dump).expect("write the syntax set");
}

/// One grammar from `syntaxes/`, read for lines that end in their newline, as two-face's are.
fn load(path: &Path) -> SyntaxDefinition {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    SyntaxDefinition::load_from_str(&text, true, None)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}
