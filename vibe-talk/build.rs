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

    page(&manifest, &out);
}

/// One grammar from `syntaxes/`, read for lines that end in their newline, as two-face's are.
fn load(path: &Path) -> SyntaxDefinition {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    SyntaxDefinition::load_from_str(&text, true, None)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

// ---- The page: `#624 voice-page-modularity` ------------------------------------------------------
//
// `web/build.mjs` builds the page the server embeds: the shell, one minified script and one
// stylesheet under content-hashed names, their source maps, and `assets.json` saying what was
// built from what. It runs here, into `OUT_DIR/web`, so a binary always embeds the page its own
// sources make; or, for a packaging stage with no Node, `VIBE_TALK_WEB_DIST` names a directory that
// stage built, which is checked against these sources and copied in. Nothing here downloads
// anything: the build's one tool, esbuild, is installed beforehand from `contract/package-lock.json`.
// What is refused, and why, is in `build_page.rs`, which `tests/page_prebuilt.rs` tests.
//
// Then the asset table: `OUT_DIR/page_assets.rs`, which `src/http/assets.rs` includes. A Rust
// macro cannot find a file whose name it only learns at run time, so the table is generated with
// each hashed file's literal `include_bytes!` path.

mod build_page;

/// Where `build.mjs` writes the page, below `OUT_DIR`.
const PAGE_DIR: &str = "web";

/// Stop the build with `message`, set apart from cargo's own output.
fn refuse(message: &str) -> ! {
    panic!("\n\n{message}\n")
}

fn page(manifest: &Path, out: &Path) {
    let web = manifest.join("web");
    let contract = manifest.join("contract");
    // A directory is scanned whole, so a file added to web/ or taken away reruns this as well. The
    // installed esbuild's own package.json too, so an `npm ci` after a refusal is noticed.
    println!("cargo::rerun-if-changed={}", web.display());
    println!("cargo::rerun-if-changed=build_page.rs");
    println!(
        "cargo::rerun-if-changed={}",
        contract.join("package-lock.json").display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        contract.join("node_modules/esbuild/package.json").display()
    );
    println!("cargo::rerun-if-env-changed=VIBE_TALK_WEB_DIST");
    let built = out.join(PAGE_DIR);
    let assets = match std::env::var_os("VIBE_TALK_WEB_DIST") {
        Some(dist) if !dist.is_empty() => take_prebuilt(manifest, Path::new(&dist), &built),
        _ => run_page_build(manifest, &built),
    };
    let script = format!("assets/voice.{}.js", assets.page_build);
    assert!(
        assets.files.iter().any(|(path, _)| *path == script),
        "the page build's page_build names no script it wrote ({script})"
    );
    std::fs::write(out.join("page_assets.rs"), asset_table(&assets, &built))
        .expect("write the asset table");
}

/// `node web/build.mjs --out built`, once the pinned tools are known to be installed.
fn run_page_build(manifest: &Path, built: &Path) -> build_page::Assets {
    build_page::check_installed_tools(manifest).unwrap_or_else(|message| refuse(&message));
    let output = std::process::Command::new("node")
        .current_dir(manifest)
        .arg("web/build.mjs")
        .arg("--out")
        .arg(built)
        .output()
        .unwrap_or_else(|error| {
            refuse(&format!(
                "The page build runs `node web/build.mjs`, and node could not be run: {error}"
            ))
        });
    if !output.status.success() {
        refuse(&format!(
            "The page build failed (`node web/build.mjs`):\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    build_page::read_assets(built).unwrap_or_else(|message| refuse(&message))
}

/// Take a page built elsewhere (`VIBE_TALK_WEB_DIST`) once `build_page::check_prebuilt` has said
/// these sources and tools made it, copying it to `built`.
fn take_prebuilt(manifest: &Path, dist: &Path, built: &Path) -> build_page::Assets {
    let assets =
        build_page::check_prebuilt(manifest, dist).unwrap_or_else(|message| refuse(&message));
    if built.exists() {
        std::fs::remove_dir_all(built)
            .unwrap_or_else(|error| refuse(&format!("clear {}: {error}", built.display())));
    }
    let files = build_page::files_below(dist).unwrap_or_else(|message| refuse(&message));
    for path in files {
        let target = built.join(&path);
        std::fs::create_dir_all(target.parent().expect("a file has a directory"))
            .unwrap_or_else(|error| refuse(&format!("create {}: {error}", target.display())));
        std::fs::copy(dist.join(&path), &target).unwrap_or_else(|error| {
            refuse(&format!("copy {path} from VIBE_TALK_WEB_DIST: {error}"))
        });
    }
    assets
}

/// `OUT_DIR/page_assets.rs`: the page build's hashed files, each by its literal path.
fn asset_table(assets: &build_page::Assets, built: &Path) -> String {
    let mut table = String::from(
        "// GENERATED by build.rs from the page build's assets.json (`#624 voice-page-modularity`).\n\n",
    );
    let script = format!("assets/voice.{}.js", assets.page_build);
    table.push_str(&format!(
        "/// The page script's hash: the page build a shell naming it runs, and every read reports.\n\
         pub const PAGE_BUILD: &str = {:?};\n\n\
         /// The page build's shell, which names the hashed script and stylesheet.\n\
         pub const SHELL: &str = include_str!({:?});\n\n\
         /// The page build's script: its path, and its bytes (also what the plain /voice.js serves).\n\
         pub const SCRIPT_PATH: &str = {:?};\n\
         pub const SCRIPT: &[u8] = include_bytes!({:?});\n\n",
        assets.page_build,
        built.join("voice.html").to_str().expect("OUT_DIR is UTF-8"),
        format!("/{script}"),
        built.join(&script).to_str().expect("OUT_DIR is UTF-8"),
    ));
    table.push_str("/// Every hashed file the build wrote: its path, its content type, and its bytes.\npub const HASHED: &[(&str, &str, &[u8])] = &[\n");
    for (path, _) in assets
        .files
        .iter()
        .filter(|(path, _)| path.starts_with("assets/"))
    {
        let content_type = if path.ends_with(".js") {
            "text/javascript; charset=utf-8"
        } else if path.ends_with(".css") {
            "text/css; charset=utf-8"
        } else if path.ends_with(".map") {
            "application/json; charset=utf-8"
        } else {
            refuse(&format!(
                "the page build wrote {path}, whose type this table does not know"
            ))
        };
        // The script's bytes once, under SCRIPT, rather than embedded a second time.
        if *path == script {
            table.push_str(&format!("    (SCRIPT_PATH, {content_type:?}, SCRIPT),\n"));
            continue;
        }
        let file = built.join(path);
        table.push_str(&format!(
            "    ({:?}, {content_type:?}, include_bytes!({:?})),\n",
            format!("/{path}"),
            file.to_str().expect("OUT_DIR is UTF-8")
        ));
    }
    table.push_str("];\n");
    table
}
