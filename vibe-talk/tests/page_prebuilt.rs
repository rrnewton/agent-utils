//! What `build.rs` refuses to embed: `build_page.rs`'s checks, run against real page builds and
//! deliberately broken copies of them. `#624 voice-page-modularity`.
//!
//! A packaging stage with no Node hands the Rust build a page it built elsewhere
//! (`VIBE_TALK_WEB_DIST`, the Containerfile's route). These hold that a page made from other
//! sources, by another esbuild, or altered since, is refused, and that a tools directory holding no
//! esbuild or another version than the lockfile pins is too; each with what to do about it. The
//! builds need the pinned tools installed: `npm ci --prefix contract` (validation's
//! `setup.contract-tools`).

#[path = "../build_page.rs"]
mod build_page;

use std::path::{Path, PathBuf};
use std::process::Command;

use vibe_talk::testing::TempDir;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh page build of this checkout, in `dir`.
fn page_build(dir: &TempDir) -> PathBuf {
    let out = dir.path().join("web");
    let output = Command::new("node")
        .current_dir(manifest())
        .arg("web/build.mjs")
        .arg("--out")
        .arg(&out)
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "the page build failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    out
}

/// `dist`'s assets.json, changed by `edit`.
fn edit_assets(dist: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = dist.join("assets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("JSON");
    edit(&mut json);
    std::fs::write(&path, json.to_string()).expect("write");
}

fn refused(dist: &Path) -> String {
    build_page::check_prebuilt(&manifest(), dist)
        .expect_err("a page these sources did not make was taken")
}

#[test]
fn a_page_these_sources_and_tools_built_is_taken() {
    let dir = TempDir::new("page-prebuilt-good");
    let dist = page_build(&dir);
    let assets = build_page::check_prebuilt(&manifest(), &dist).expect("taken");
    assert_eq!(assets.page_build.len(), 32);
    assert_eq!(
        assets.esbuild,
        build_page::esbuild_pin(&manifest()).expect("a pin")
    );
    build_page::check_installed_tools(&manifest()).expect("the pinned esbuild is installed");
}

#[test]
fn a_page_from_other_sources_is_refused_naming_what_differs() {
    let dir = TempDir::new("page-prebuilt-stale");
    let dist = page_build(&dir);
    edit_assets(&dist, |json| {
        json["inputs"]["vibe-talk/web/voice.js"] = "0".repeat(32).into();
    });
    let message = refused(&dist);
    assert!(
        message.contains("not built from these sources"),
        "{message}"
    );
    assert!(message.contains("vibe-talk/web/voice.js"), "{message}");

    // An input it never read, as when a file was added to web/ since: a difference too.
    let dir = TempDir::new("page-prebuilt-missing-input");
    let dist = page_build(&dir);
    edit_assets(&dist, |json| {
        json["inputs"]
            .as_object_mut()
            .expect("inputs")
            .remove("vibe-talk/web/voice.css");
    });
    assert!(refused(&dist).contains("vibe-talk/web/voice.css"));

    // And one these sources do not have.
    let dir = TempDir::new("page-prebuilt-extra-input");
    let dist = page_build(&dir);
    edit_assets(&dist, |json| {
        json["inputs"]["vibe-talk/web/gone.js"] = "0".repeat(32).into();
    });
    assert!(refused(&dist).contains("vibe-talk/web/gone.js"));
}

#[test]
fn a_page_built_by_another_esbuild_is_refused() {
    let dir = TempDir::new("page-prebuilt-tools");
    let dist = page_build(&dir);
    edit_assets(&dist, |json| json["tools"]["esbuild"] = "0.0.1".into());
    let message = refused(&dist);
    assert!(message.contains("esbuild 0.0.1"), "{message}");
}

#[test]
fn a_page_altered_or_missing_a_file_since_it_was_built_is_refused() {
    let dir = TempDir::new("page-prebuilt-tampered");
    let dist = page_build(&dir);
    let style = build_page::read_assets(&dist)
        .expect("assets.json")
        .files
        .into_iter()
        .map(|(path, _)| path)
        .find(|path| path.starts_with("assets/") && path.ends_with(".css"))
        .expect("a stylesheet");
    let mut bytes = std::fs::read(dist.join(&style)).expect("read");
    bytes.extend_from_slice(b"body{color:red}");
    std::fs::write(dist.join(&style), bytes).expect("write");
    let message = refused(&dist);
    assert!(message.contains("is not the file it declares"), "{message}");
    assert!(message.contains(&style), "{message}");

    let dir = TempDir::new("page-prebuilt-missing-file");
    let dist = page_build(&dir);
    std::fs::remove_file(dist.join("voice.html")).expect("remove");
    let message = refused(&dist);
    assert!(message.contains("lacks voice.html"), "{message}");
}

#[test]
fn tools_that_are_missing_or_not_the_pinned_version_are_refused_with_the_install_command() {
    // A copy of this crate's lockfile with no tools installed, then with another esbuild.
    let dir = TempDir::new("page-tools");
    let contract = dir.path().join("contract");
    std::fs::create_dir_all(&contract).expect("mkdir");
    std::fs::copy(
        manifest().join("contract/package-lock.json"),
        contract.join("package-lock.json"),
    )
    .expect("copy the lockfile");
    let missing = build_page::check_installed_tools(dir.path()).expect_err("no tools were taken");
    assert!(missing.contains("not installed"), "{missing}");
    assert!(missing.contains(build_page::INSTALL), "{missing}");

    let esbuild = contract.join("node_modules/esbuild");
    std::fs::create_dir_all(&esbuild).expect("mkdir");
    std::fs::write(esbuild.join("package.json"), r#"{"version": "0.0.1"}"#).expect("write");
    let other =
        build_page::check_installed_tools(dir.path()).expect_err("another esbuild was taken");
    let pin = build_page::esbuild_pin(dir.path()).expect("a pin");
    assert!(other.contains("0.0.1") && other.contains(&pin), "{other}");
    assert!(other.contains(build_page::INSTALL), "{other}");
}

#[test]
fn the_lockfile_pins_the_esbuild_the_package_names() {
    // One version in two files: package.json's exact pin and the lockfile's resolution of it.
    let package: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(manifest().join("contract/package.json")).expect("read"),
    )
    .expect("JSON");
    assert_eq!(
        package["devDependencies"]["esbuild"].as_str(),
        Some(
            build_page::esbuild_pin(&manifest())
                .expect("a pin")
                .as_str()
        )
    );
}
