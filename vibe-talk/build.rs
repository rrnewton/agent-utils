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
//
// Then the asset table: `OUT_DIR/page_assets.rs`, which `src/http/assets.rs` includes. A Rust
// macro cannot find a file whose name it only learns at run time, so the table is generated with
// each hashed file's literal `include_bytes!` path.

/// Where `build.mjs` writes the page, below `OUT_DIR`.
const PAGE_DIR: &str = "web";

/// The command that installs the page build's tools, for every message that needs it.
const INSTALL: &str = "npm ci --prefix vibe-talk/contract \
    (validation's setup step runs scripts/provision-node-packages vibe-talk/contract)";

fn page(manifest: &Path, out: &Path) {
    let web = manifest.join("web");
    let lockfile = manifest.join("contract").join("package-lock.json");
    // A directory is scanned whole, so a file added to web/ or taken away reruns this as well.
    println!("cargo::rerun-if-changed={}", web.display());
    println!("cargo::rerun-if-changed={}", lockfile.display());
    println!("cargo::rerun-if-env-changed=VIBE_TALK_WEB_DIST");
    let built = out.join(PAGE_DIR);
    match std::env::var_os("VIBE_TALK_WEB_DIST") {
        Some(dist) if !dist.is_empty() => take_prebuilt(manifest, Path::new(&dist), &built),
        _ => run_page_build(manifest, &built),
    }
    let assets = read_assets(&built);
    let script = format!("assets/voice.{}.js", assets.page_build);
    assert!(
        assets.files.iter().any(|(path, _)| *path == script),
        "the page build's page_build names no script it wrote ({script})"
    );
    std::fs::write(out.join("page_assets.rs"), asset_table(&assets, &built))
        .expect("write the asset table");
}

/// `node web/build.mjs --out built`, failing with what to do when its prerequisites are missing.
fn run_page_build(manifest: &Path, built: &Path) {
    let esbuild = manifest
        .join("contract")
        .join("node_modules")
        .join("esbuild")
        .join("package.json");
    if !esbuild.is_file() {
        panic!(
            "\n\nThe page build needs the pinned esbuild in {}, which is not installed.\n\
             From the repository root, run:\n\n    {INSTALL}\n\n\
             (or set VIBE_TALK_WEB_DIST to a page build made from these sources).\n",
            esbuild
                .parent()
                .and_then(Path::parent)
                .map_or_else(String::new, |p| p.display().to_string())
        );
    }
    let output = std::process::Command::new("node")
        .current_dir(manifest)
        .arg("web/build.mjs")
        .arg("--out")
        .arg(built)
        .output()
        .unwrap_or_else(|error| {
            panic!("\n\nThe page build runs `node web/build.mjs`, and node could not be run: {error}\n")
        });
    if !output.status.success() {
        panic!(
            "\n\nThe page build failed (`node web/build.mjs`):\n{}{}\n",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// What `build.rs` needs from `assets.json`.
struct Assets {
    /// The script's hash: what a read reports as `page_build` once the shell names it.
    page_build: String,
    /// Every file the build wrote, by path relative to the build directory, and its digest.
    files: Vec<(String, String)>,
    /// Every input the build read, by repository-relative path, and its digest.
    inputs: Vec<(String, String)>,
}

fn read_assets(built: &Path) -> Assets {
    let path = built.join("assets.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    let pairs = |key: &str| -> Vec<(String, String)> {
        json[key]
            .as_object()
            .unwrap_or_else(|| panic!("{} has no {key}", path.display()))
            .iter()
            .map(|(name, digest)| {
                let digest = digest
                    .as_str()
                    .unwrap_or_else(|| panic!("{key}.{name} is not a digest"));
                (name.clone(), digest.to_owned())
            })
            .collect()
    };
    let page_build = json["page_build"]
        .as_str()
        .unwrap_or_else(|| panic!("{} has no page_build", path.display()))
        .to_owned();
    Assets {
        page_build,
        files: pairs("files"),
        inputs: pairs("inputs"),
    }
}

/// The first 128 bits of the SHA-256 of `bytes`, in lowercase hex: `build.mjs`'s and the server's.
fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    hash.as_ref()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Take a page built elsewhere (`VIBE_TALK_WEB_DIST`), only if these sources would build it:
/// its inputs are exactly every file below web/ and the lockfile, each with the digest it has here,
/// and every file it declares is present with the digest it declares. Then copy it to `built`.
fn take_prebuilt(manifest: &Path, dist: &Path, built: &Path) {
    let assets = read_assets(dist);
    let mut here: Vec<(String, String)> = files_below(&manifest.join("web"))
        .into_iter()
        .map(|relative| format!("vibe-talk/web/{relative}"))
        .chain(std::iter::once(
            "vibe-talk/contract/package-lock.json".to_owned(),
        ))
        .map(|name| {
            // Named from the repository root, read from this crate's directory: a container build
            // has the crate alone.
            let local = manifest.join(name.strip_prefix("vibe-talk/").expect("a vibe-talk path"));
            let bytes = std::fs::read(&local)
                .unwrap_or_else(|error| panic!("read {}: {error}", local.display()));
            (name, digest(&bytes))
        })
        .collect();
    here.sort();
    let mut theirs = assets.inputs.clone();
    theirs.sort();
    if here != theirs {
        let mut stale: Vec<&String> = here
            .iter()
            .filter(|pair| !theirs.contains(pair))
            .chain(theirs.iter().filter(|pair| !here.contains(pair)))
            .map(|(name, _)| name)
            .collect();
        stale.sort();
        stale.dedup();
        panic!(
            "\n\nVIBE_TALK_WEB_DIST={} was not built from these sources (differs in {stale:?}).\n\
             Rebuild it with `node web/build.mjs --out DIR`, or unset VIBE_TALK_WEB_DIST.\n",
            dist.display()
        );
    }
    for (path, want) in &assets.files {
        let bytes = std::fs::read(dist.join(path))
            .unwrap_or_else(|error| panic!("VIBE_TALK_WEB_DIST lacks {path}: {error}"));
        assert_eq!(
            &digest(&bytes),
            want,
            "VIBE_TALK_WEB_DIST's {path} is not the file it declares"
        );
    }
    if built.exists() {
        std::fs::remove_dir_all(built)
            .unwrap_or_else(|error| panic!("clear {}: {error}", built.display()));
    }
    for path in files_below(dist) {
        let target = built.join(&path);
        std::fs::create_dir_all(target.parent().expect("a file has a directory"))
            .unwrap_or_else(|error| panic!("create {}: {error}", target.display()));
        std::fs::copy(dist.join(&path), &target)
            .unwrap_or_else(|error| panic!("copy {path} from VIBE_TALK_WEB_DIST: {error}"));
    }
}

/// Every file below `root`, as a `/`-separated relative path, sorted.
fn files_below(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        for entry in std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        {
            let entry = entry.expect("a directory entry");
            let path = relative.join(entry.file_name());
            if root.join(&path).is_dir() {
                pending.push(path);
            } else {
                let parts: Vec<String> = path
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    found.sort();
    found
}

/// `OUT_DIR/page_assets.rs`: the page build's hashed files, each by its literal path.
fn asset_table(assets: &Assets, built: &Path) -> String {
    let mut table = String::from(
        "// GENERATED by build.rs from the page build's assets.json (`#624 voice-page-modularity`).\n\n",
    );
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
            panic!("the page build wrote {path}, whose type this table does not know")
        };
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
