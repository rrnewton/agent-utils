//! The page build's checks, shared by `build.rs` and `tests/page_prebuilt.rs`.
//! `#624 voice-page-modularity`.
//!
//! `build.rs` runs `web/build.mjs`, or takes a page built elsewhere (`VIBE_TALK_WEB_DIST`), and
//! embeds the result. What it must refuse is decided here, as functions that return what is wrong
//! rather than panicking, so the refusals are tested like any other code: a tools directory that is
//! missing or holds another esbuild than the lockfile pins, and a prebuilt page that these sources,
//! or these tools, did not make.

// Each of the two users calls part of this file.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The command that installs the page build's tools, for every message that needs it.
pub const INSTALL: &str = "npm ci --prefix vibe-talk/contract \
    (validation's setup step runs scripts/provision-node-packages vibe-talk/contract)";

/// What `build.rs` needs from a page build's `assets.json`.
#[derive(Debug)]
pub struct Assets {
    /// The script's hash: what a read reports as `page_build` once the shell names it.
    pub page_build: String,
    /// Every file the build wrote, by path relative to the build directory, and its digest.
    pub files: Vec<(String, String)>,
    /// Every input the build read, by repository-relative path, and its digest.
    pub inputs: Vec<(String, String)>,
    /// The esbuild version that built it.
    pub esbuild: String,
}

/// The `assets.json` of the page build in `built`.
pub fn read_assets(built: &Path) -> Result<Assets, String> {
    let path = built.join("assets.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    let pairs = |key: &str| -> Result<Vec<(String, String)>, String> {
        let object = json[key]
            .as_object()
            .ok_or_else(|| format!("{} has no {key}", path.display()))?;
        object
            .iter()
            .map(|(name, digest)| {
                let digest = digest
                    .as_str()
                    .ok_or_else(|| format!("{key}.{name} in {} is not a digest", path.display()))?;
                Ok((name.clone(), digest.to_owned()))
            })
            .collect()
    };
    let text_of = |value: &serde_json::Value, what: &str| -> Result<String, String> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} has no {what}", path.display()))
    };
    Ok(Assets {
        page_build: text_of(&json["page_build"], "page_build")?,
        files: pairs("files")?,
        inputs: pairs("inputs")?,
        esbuild: text_of(&json["tools"]["esbuild"], "tools.esbuild")?,
    })
}

/// The first 128 bits of the SHA-256 of `bytes`, in lowercase hex: `build.mjs`'s and the server's.
pub fn digest(bytes: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    hash.as_ref()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Every file below `root`, as a `/`-separated relative path, sorted.
pub fn files_below(root: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("read {}: {error}", directory.display()))?;
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
    Ok(found)
}

/// The esbuild version `contract/package-lock.json` pins.
pub fn esbuild_pin(manifest: &Path) -> Result<String, String> {
    let path = manifest.join("contract").join("package-lock.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    json["packages"]["node_modules/esbuild"]["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} pins no esbuild", path.display()))
}

/// That the installed tools are the ones the lockfile pins: an esbuild is installed in
/// `contract/node_modules`, and it is the pinned version. A lockfile that moved since the last
/// `npm ci` is caught here rather than building with the old tool.
pub fn check_installed_tools(manifest: &Path) -> Result<(), String> {
    let modules = manifest.join("contract").join("node_modules");
    let package = modules.join("esbuild").join("package.json");
    let pin = esbuild_pin(manifest)?;
    let installed = match std::fs::read_to_string(&package) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|json| json["version"].as_str().map(str::to_owned)),
        Err(_) => {
            return Err(format!(
                "The page build needs the pinned esbuild {pin} in {}, which is not installed.\n\
                 From the repository root, run:\n\n    {INSTALL}\n\n\
                 (or set VIBE_TALK_WEB_DIST to a page build made from these sources).",
                modules.display()
            ));
        }
    };
    match installed {
        Some(version) if version == pin => Ok(()),
        other => Err(format!(
            "The page build needs esbuild {pin}, as contract/package-lock.json pins, but {} holds {}.\n\
             From the repository root, run:\n\n    {INSTALL}\n",
            modules.display(),
            other.unwrap_or_else(|| "an unreadable version".to_owned())
        )),
    }
}

/// A page built elsewhere (`VIBE_TALK_WEB_DIST`), only if these sources and tools would build it:
/// its inputs are exactly every file below web/ and the lockfile, each with the digest it has here;
/// it was built by the esbuild the lockfile pins; and every file it declares is present with the
/// digest it declares. Returns its `assets.json`.
pub fn check_prebuilt(manifest: &Path, dist: &Path) -> Result<Assets, String> {
    let assets = read_assets(dist)?;
    let mut here: Vec<(String, String)> = Vec::new();
    for relative in files_below(&manifest.join("web"))? {
        let bytes = std::fs::read(manifest.join("web").join(&relative))
            .map_err(|error| format!("read web/{relative}: {error}"))?;
        here.push((format!("vibe-talk/web/{relative}"), digest(&bytes)));
    }
    let lockfile = manifest.join("contract").join("package-lock.json");
    let bytes = std::fs::read(&lockfile)
        .map_err(|error| format!("read {}: {error}", lockfile.display()))?;
    here.push((
        "vibe-talk/contract/package-lock.json".to_owned(),
        digest(&bytes),
    ));
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
        return Err(format!(
            "VIBE_TALK_WEB_DIST={} was not built from these sources (differs in {stale:?}).\n\
             Rebuild it with `node web/build.mjs --out DIR`, or unset VIBE_TALK_WEB_DIST.",
            dist.display()
        ));
    }
    let pin = esbuild_pin(manifest)?;
    if assets.esbuild != pin {
        return Err(format!(
            "VIBE_TALK_WEB_DIST={} was built by esbuild {}, not the {pin} contract/package-lock.json pins.",
            dist.display(),
            assets.esbuild
        ));
    }
    for (path, want) in &assets.files {
        let bytes = std::fs::read(dist.join(path))
            .map_err(|error| format!("VIBE_TALK_WEB_DIST lacks {path}: {error}"))?;
        if &digest(&bytes) != want {
            return Err(format!(
                "VIBE_TALK_WEB_DIST's {path} is not the file it declares"
            ));
        }
    }
    Ok(assets)
}
