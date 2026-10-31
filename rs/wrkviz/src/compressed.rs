//! Upstream sources compressed in place (`X` read as
//! `X.zst` when that is what exists):
//! `py/wrkviz/compressed_sources.py` at parity. Decoding
//! shells to the `zstd` command, the fallback the Python
//! oracle's interpreter also takes.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The suffix an in-place compression appends.
pub const COMPRESSED_SUFFIX: &str = ".zst";

/// The path an in-place compression of *path* produced.
pub fn compressed_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(COMPRESSED_SUFFIX);
    path.with_file_name(name)
}

/// *path* with a compression suffix removed.
pub fn logical_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.ends_with(COMPRESSED_SUFFIX) && name.len() > COMPRESSED_SUFFIX.len() {
        path.with_file_name(&name[..name.len() - COMPRESSED_SUFFIX.len()])
    } else {
        path.to_path_buf()
    }
}

/// Whether *path* names the compressed spelling.
pub fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().ends_with(COMPRESSED_SUFFIX))
        .unwrap_or(false)
}

fn lexists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The physical file holding *path*'s bytes, preferring the
/// plain spelling.
pub fn resolve_source(path: &Path) -> Option<PathBuf> {
    if lexists(path) {
        return Some(path.to_path_buf());
    }
    let sibling = compressed_sibling(path);
    if lexists(&sibling) {
        return Some(sibling);
    }
    None
}

fn decompress(data: &[u8], where_: &Path) -> Result<Vec<u8>, String> {
    let mut child = Command::new("zstd")
        .args(["--decompress", "--stdout", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            format!(
                "cannot read {}: it is zstd-compressed and this interpreter has no zstd \
                 decoder (Python 3.14's compression.zstd, the zstandard package, or the zstd \
                 command)",
                where_.display()
            )
        })?;
    let mut stdin = child.stdin.take().unwrap();
    let data = data.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let output = child
        .wait_with_output()
        .map_err(|error| crate::json::os_error(&error, where_))?;
    let _ = writer.join();
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "cannot decompress {}: zstd exited {}: {detail}",
            where_.display(),
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(output.stdout)
}

/// The logical bytes of a file whose physical bytes are
/// *data*.
pub fn decode_bytes(physical: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    if is_compressed(physical) {
        decompress(data, physical)
    } else {
        Ok(data.to_vec())
    }
}

/// The logical bytes of *path*.
pub fn read_source_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let Some(physical) = resolve_source(path) else {
        return Err(format!(
            "No such file or directory (os error 2): no such file (plain or {COMPRESSED_SUFFIX}): {}",
            path.display()
        ));
    };
    let data =
        std::fs::read(&physical).map_err(|error| crate::json::os_error(&error, &physical))?;
    decode_bytes(&physical, &data)
}

/// The first line of *path*'s logical bytes.
pub fn read_first_line(path: &Path) -> Result<Vec<u8>, String> {
    let Some(physical) = resolve_source(path) else {
        return Err(format!(
            "No such file or directory (os error 2): no such file (plain or {COMPRESSED_SUFFIX}): {}",
            path.display()
        ));
    };
    if !is_compressed(&physical) {
        let data =
            std::fs::read(&physical).map_err(|error| crate::json::os_error(&error, &physical))?;
        return Ok(match data.iter().position(|byte| *byte == b'\n') {
            Some(index) => data[..=index].to_vec(),
            None => data,
        });
    }
    let data = read_source_bytes(path)?;
    Ok(match data.iter().position(|byte| *byte == b'\n') {
        Some(index) => data[..=index].to_vec(),
        None => data,
    })
}

/// Whether *path* is present as a regular file in either
/// spelling (a symlink spelling does not count).
pub fn source_exists(path: &Path) -> bool {
    resolve_source(path).is_some_and(|physical| {
        physical.is_file()
            && !std::fs::symlink_metadata(&physical)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
    })
}

/// Logical paths matching `agent-*.jsonl` below *root*
/// (recursively), `.zst` spellings counted as their plain
/// name, sorted.
pub fn iter_logical_glob_agents(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    collect_agents(root, &mut found);
    found.sort();
    found.dedup();
    found
}

fn collect_agents(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        // A directory whose name matches is still a glob
        // match (the oracle's `rglob` yields it, and its
        // regular-file check then refuses); recursion into
        // it also happens.
        if name.starts_with("agent-") && name.ends_with(".jsonl") {
            found.push(path.clone());
        }
        if name.starts_with("agent-") && name.ends_with(".jsonl.zst") {
            found.push(logical_path(&path));
        }
        if is_dir {
            collect_agents(&path, found);
        }
    }
}

/// Logical paths matching `rollout-*.jsonl` below *root*,
/// `.zst` spellings counted as their plain name, sorted.
pub fn iter_logical_glob_rollouts(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    collect_rollouts(root, &mut found);
    found.sort();
    found.dedup();
    found
}

fn collect_rollouts(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if is_dir {
            collect_rollouts(&path, found);
            continue;
        }
        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
            found.push(path.clone());
        }
        if name.starts_with("rollout-") && name.ends_with(".jsonl.zst") {
            found.push(logical_path(&path));
        }
    }
}
