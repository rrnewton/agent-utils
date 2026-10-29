//! Records build provenance for `agentctl --version` and `agentctl capabilities`.
//!
//! Three optional environment variables describe the source that a deliberate build (a release,
//! or a team's pinned tool directory) comes from. Each is passed to the crate only when it is set
//! and well-formed; a malformed value fails the build rather than publishing provenance nobody can
//! match:
//!
//! - `AGENTCTL_SOURCE_FP`: the 64-character lowercase hexadecimal SHA-256 that
//!   `common/bin/rs-source-fingerprint` prints (passed as `AGENTCTL_BUILD_SOURCE_FP`).
//! - `AGENTCTL_SOURCE_COMMIT`: the 40-character lowercase hexadecimal Git commit of the checkout
//!   (passed as `AGENTCTL_BUILD_SOURCE_COMMIT`).
//! - `AGENTCTL_SOURCE_DIRTY`: `0` or `1`, whether that checkout had uncommitted changes to tracked
//!   files (passed as `AGENTCTL_BUILD_SOURCE_DIRTY`); it requires `AGENTCTL_SOURCE_COMMIT`.
//!
//! Nothing is read from Git here and nothing time-dependent is recorded, so builds stay
//! reproducible and an unrelated commit never forces an ordinary development build to rebuild.

use std::env::{self, VarError};

const SOURCE_FINGERPRINT: &str = "AGENTCTL_SOURCE_FP";
const SOURCE_COMMIT: &str = "AGENTCTL_SOURCE_COMMIT";
const SOURCE_DIRTY: &str = "AGENTCTL_SOURCE_DIRTY";

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The variable's nonempty value; `None` when it is unset or empty.
fn read(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    match env::var(name) {
        Ok(value) if value.is_empty() => None,
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(value)) => panic!("{name} is not valid Unicode: {value:?}"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if let Some(value) = read(SOURCE_FINGERPRINT) {
        assert!(
            lowercase_hex(&value, 64),
            "{SOURCE_FINGERPRINT} must be a 64-character lowercase hexadecimal SHA-256, got {value:?}"
        );
        println!("cargo:rustc-env=AGENTCTL_BUILD_SOURCE_FP={value}");
    }
    let commit = read(SOURCE_COMMIT);
    if let Some(value) = &commit {
        assert!(
            lowercase_hex(value, 40),
            "{SOURCE_COMMIT} must be a 40-character lowercase hexadecimal Git commit, got {value:?}"
        );
        println!("cargo:rustc-env=AGENTCTL_BUILD_SOURCE_COMMIT={value}");
    }
    if let Some(value) = read(SOURCE_DIRTY) {
        assert!(
            value == "0" || value == "1",
            "{SOURCE_DIRTY} must be 0 or 1, got {value:?}"
        );
        assert!(
            commit.is_some(),
            "{SOURCE_DIRTY} describes {SOURCE_COMMIT}; set both or neither"
        );
        println!("cargo:rustc-env=AGENTCTL_BUILD_SOURCE_DIRTY={value}");
    }
}
