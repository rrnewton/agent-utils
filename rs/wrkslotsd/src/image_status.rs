//! Pure `image status` core (P8, mu-rsimg, unwired until dispatch).
//!
//! Mirrors Python `slotimage.usage` (`slotimage.py:1673`) and the
//! rendering half of `_cmd_image_status` (`imagecmd.py:865`). File
//! stats are read from the paths given (temp fixtures in tests);
//! mount backends and statvfs facts are injected by the caller via
//! `image_mounts`, so nothing here mounts or reads `/proc`.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::image_record::ImageRecord;

const GIB: f64 = 1_073_741_824.0;

/// statvfs-derived facts for one mounted image, mirroring the
/// `used/free/inodes` fields Python computes in `usage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VfsFacts {
    pub(crate) used_bytes: u64,
    pub(crate) free_bytes: u64,
    pub(crate) used_inodes: u64,
}

fn file_entry(
    image_file: &Path,
    mount_point: &Path,
    backend_for: &impl Fn(&Path, &Path) -> Option<String>,
    vfs_for: &impl Fn(&Path) -> Option<VfsFacts>,
) -> Value {
    use std::os::unix::fs::MetadataExt as _;
    let mut entry = Map::new();
    entry.insert("image".to_string(), json!(image_file.to_string_lossy()));
    match std::fs::metadata(image_file) {
        Ok(meta) => {
            entry.insert("ceiling_bytes".to_string(), json!(meta.len()));
            entry.insert(
                "host_allocated_bytes".to_string(),
                json!(meta.blocks() * 512),
            );
        }
        Err(error) => {
            // Python renders `str(OSError)` as
            // `[Errno N] <strerror>: '<path>'`; match it byte for byte.
            let errno = error.raw_os_error().unwrap_or_default();
            let message = error.to_string();
            let stripped = message
                .strip_suffix(&format!(" (os error {errno})"))
                .unwrap_or(&message);
            entry.insert(
                "error".to_string(),
                json!(format!(
                    "[Errno {errno}] {stripped}: '{}'",
                    image_file.display()
                )),
            );
        }
    }
    match backend_for(image_file, mount_point) {
        Some(backend) => {
            entry.insert("mounted".to_string(), json!(true));
            entry.insert("backend".to_string(), json!(backend));
            if let Some(vfs) = vfs_for(mount_point) {
                entry.insert("used_bytes".to_string(), json!(vfs.used_bytes));
                entry.insert("free_bytes".to_string(), json!(vfs.free_bytes));
                entry.insert("used_inodes".to_string(), json!(vfs.used_inodes));
            }
        }
        None => {
            entry.insert("mounted".to_string(), json!(false));
        }
    }
    Value::Object(entry)
}

/// One image's usage report, mirroring Python `slotimage.usage`.
pub(crate) fn usage_report(
    record: &ImageRecord,
    backend_for: &impl Fn(&Path, &Path) -> Option<String>,
    vfs_for: &impl Fn(&Path) -> Option<VfsFacts>,
) -> Value {
    json!({
        "name": record.slot,
        "slot_type": record.slot_type,
        "location": record.location.to_string_lossy(),
        "phase": record.phase,
        "slot": file_entry(&record.image_file(), &record.location, backend_for, vfs_for),
        "state": file_entry(&record.state_image(), &record.state_mount(), backend_for, vfs_for),
    })
}

fn gib(value: Option<&Value>) -> String {
    match value.and_then(Value::as_f64) {
        Some(bytes) => format!("{:.2}G", bytes / GIB),
        None => "-".to_string(),
    }
}

fn display(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        // Python renders booleans capitalized in the human form.
        Some(Value::Bool(flag)) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Some(other) => other.to_string(),
        None => "-".to_string(),
    }
}

/// The human rendering of `_cmd_image_status`, newline-terminated.
pub(crate) fn render_status_human(new_slot_representation: &str, reports: &[Value]) -> String {
    let mut out = format!("new slots: {new_slot_representation}\n");
    if reports.is_empty() {
        out.push_str("no image-backed slots\n");
    }
    for report in reports {
        out.push_str(&format!(
            "{}/{} at {} ({})\n",
            display(report.get("slot_type").cloned().as_ref()),
            display(report.get("name").cloned().as_ref()),
            display(report.get("location").cloned().as_ref()),
            display(report.get("phase").cloned().as_ref()),
        ));
        for label in ["slot", "state"] {
            let Some(entry) = report.get(label) else {
                continue;
            };
            out.push_str(&format!(
                "  {label:5} mounted={} backend={} host={} used={} ceiling={} inodes={}\n",
                display(entry.get("mounted")),
                display(entry.get("backend")),
                gib(entry.get("host_allocated_bytes")),
                gib(entry.get("used_bytes")),
                gib(entry.get("ceiling_bytes")),
                display(entry.get("used_inodes")),
            ));
        }
    }
    out
}

/// The JSON rendering of `_cmd_image_status --format json`.
pub(crate) fn render_status_json(new_slot_representation: &str, reports: &[Value]) -> String {
    let payload = json!({
        "new_slot_representation": new_slot_representation,
        "images": reports,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_record::write_record;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn usage_and_rendering_on_temp_images() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("wrkslotsd-image-status-{}-{n}", std::process::id()));
        let control = root.join("control");
        let record = ImageRecord {
            directory: crate::image_record::image_directory(&control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: root.join("slots").join("slot-a"),
            ceiling_bytes: 8 * 1_073_741_824,
            state_ceiling_bytes: 2 * 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        std::fs::create_dir_all(&record.directory).expect("image dir");
        write_record(&record).expect("write record");
        // Sparse temp images only: sized, never formatted or mounted.
        std::fs::File::create(record.image_file())
            .expect("slot image")
            .set_len(8 * 1_073_741_824)
            .expect("size slot");
        std::fs::File::create(record.state_image())
            .expect("state image")
            .set_len(2 * 1_073_741_824)
            .expect("size state");

        let no_backend = |_: &Path, _: &Path| -> Option<String> { None };
        let no_vfs = |_: &Path| -> Option<VfsFacts> { None };
        let report = usage_report(&record, &no_backend, &no_vfs);
        assert_eq!(report["name"], "slot-a");
        assert_eq!(report["slot"]["mounted"], false);
        assert_eq!(report["slot"]["ceiling_bytes"], 8 * 1_073_741_824_u64);

        let human = render_status_human("image", std::slice::from_ref(&report));
        assert!(human.starts_with("new slots: image\n"), "{human}");
        assert!(human.contains("agent/slot-a at "), "{human}");
        assert!(human.contains("ceiling=8.00G"), "{human}");
        assert!(render_status_human("worktree", &[]).contains("no image-backed slots"));

        let parsed: Value =
            serde_json::from_str(&render_status_json("image", &[report])).expect("json");
        assert_eq!(parsed["new_slot_representation"], "image");
        assert_eq!(parsed["images"][0]["slot_type"], "agent");

        let mounted = usage_report(
            &record,
            &|_: &Path, _: &Path| Some("kernel".to_string()),
            &|_: &Path| {
                Some(VfsFacts {
                    used_bytes: 1_073_741_824,
                    free_bytes: 2_147_483_648,
                    used_inodes: 42,
                })
            },
        );
        assert_eq!(mounted["slot"]["backend"], "kernel");
        assert_eq!(mounted["slot"]["used_inodes"], 42);

        std::fs::remove_dir_all(&root).expect("cleanup");
        let _ = PathBuf::new();
    }
}
