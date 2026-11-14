//! The crate's one audited unsafe wrapper: `name_to_handle_at(2)`
//! for the stable directory identity Python's
//! `_fd_file_handle` reads through ctypes.
//!
//! This module is the only `#[allow(unsafe_code)]` in the crate
//! (the crate root denies unsafe code). It follows the pattern of
//! dagrun's documented libc calls in `cgroup.rs`: one small
//! function, a SAFETY comment at the call, checked buffer sizing,
//! and errno mapped into the same refusal text Python raises.

use std::os::unix::io::RawFd;

type Result<T> = std::result::Result<T, String>;

/// `MAX_HANDLE_SZ` (linux/fcntl.h), also Python's
/// `_FILE_HANDLE_BYTES_LIMIT`.
const MAX_HANDLE_BYTES: usize = 128;

/// The fixed header of `struct file_handle` before its flexible
/// handle array: `handle_bytes` (c_uint) then `handle_type` (c_int).
const HANDLE_HEADER_BYTES: usize = 8;

/// Python `_fd_file_handle`: the filesystem's stable handle for one
/// already-open object, as (handle type, lowercase handle hex).
///
/// The object is named through `/proc/self/fd/{fd}`, exactly as the
/// Python edition names it, so both editions read the same handle
/// for the same open directory.
#[allow(unsafe_code)]
pub(crate) fn fd_file_handle(fd: RawFd, label: &str) -> Result<(i32, String)> {
    let path = std::ffi::CString::new(format!("/proc/self/fd/{fd}"))
        .map_err(|_| format!("cannot determine stable directory identity for {label}"))?;
    // The kernel writes at most MAX_HANDLE_BYTES of handle past the
    // header, and reports the size it needs in handle_bytes when the
    // buffer is short (EOVERFLOW); a reported size above the cap is
    // refused rather than allocated, as Python refuses it.
    let mut buffer = vec![0u8; HANDLE_HEADER_BYTES + MAX_HANDLE_BYTES];
    let mut attempt = 0;
    loop {
        attempt += 1;
        buffer[0..4].copy_from_slice(&(MAX_HANDLE_BYTES as u32).to_ne_bytes());
        let mut mount_id: libc::c_int = 0;
        // SAFETY: `path` is a live NUL-terminated string for the
        // duration of the call; `buffer` holds the header plus
        // MAX_HANDLE_BYTES, the largest handle the kernel may write,
        // and its address is a valid `struct file_handle` layout
        // (two fixed fields followed by the handle array); `mount_id`
        // is a valid out-pointer. The kernel writes only within the
        // buffer and the two out values, per name_to_handle_at(2).
        let result = unsafe {
            libc::name_to_handle_at(
                libc::AT_FDCWD,
                path.as_ptr(),
                buffer.as_mut_ptr() as *mut libc::file_handle,
                &mut mount_id,
                libc::AT_SYMLINK_FOLLOW,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EOVERFLOW) && attempt == 1 {
                let needed = u32::from_ne_bytes(buffer[0..4].try_into().unwrap_or([0; 4])) as usize;
                if needed > 0 && needed <= MAX_HANDLE_BYTES {
                    continue;
                }
                return Err(format!("invalid stable directory identity for {label}"));
            }
            return Err(format!(
                "cannot determine stable directory identity for {label}: {error}"
            ));
        }
        let handle_bytes = u32::from_ne_bytes(buffer[0..4].try_into().unwrap_or([0; 4])) as usize;
        if handle_bytes == 0 || handle_bytes > MAX_HANDLE_BYTES {
            return Err(format!("invalid stable directory identity for {label}"));
        }
        let handle_type = i32::from_ne_bytes(buffer[4..8].try_into().unwrap_or([0; 4]));
        let hex = buffer[HANDLE_HEADER_BYTES..HANDLE_HEADER_BYTES + handle_bytes]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        return Ok((handle_type, hex));
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::io::AsRawFd as _;

    use super::fd_file_handle;

    #[test]
    fn directory_handle_matches_python_shape() {
        let dir = std::env::temp_dir();
        let file = std::fs::File::open(&dir).expect("open temp dir");
        let (handle_type, hex) =
            fd_file_handle(file.as_raw_fd(), "temp dir").expect("tmpfs/overlay handle");
        let _ = handle_type;
        assert!(!hex.is_empty() && hex.len() <= 256);
        assert!(hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    }

    #[test]
    fn handle_matches_python_for_same_directory() {
        let dir = std::env::temp_dir();
        let file = std::fs::File::open(&dir).expect("open temp dir");
        let mine = fd_file_handle(file.as_raw_fd(), "temp dir").expect("handle");
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("repo root")
            .to_path_buf();
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(
                "import json, os, sys\n\
                 sys.path.insert(0, \"py\")\n\
                 from wrkslots import cli\n\
                 fd = os.open(sys.argv[1], os.O_RDONLY)\n\
                 print(json.dumps(cli._fd_file_handle(fd, \"temp dir\")))\n",
            )
            .arg(&dir)
            .current_dir(repo_root)
            .output()
            .expect("python oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).expect("oracle json");
        assert_eq!(expected[0].as_i64(), Some(i64::from(mine.0)));
        assert_eq!(expected[1].as_str(), Some(mine.1.as_str()));
    }

    #[test]
    fn unsupported_object_refuses_like_python() {
        // A socket has no filesystem file handle; the kernel refuses
        // (EOPNOTSUPP), which Python maps to the same refusal line.
        let (one, _two) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let error = fd_file_handle(one.as_raw_fd(), "socket").expect_err("no handle");
        assert!(
            error.starts_with("cannot determine stable directory identity for socket"),
            "{error}"
        );
    }
}
