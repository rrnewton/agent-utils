//! Dispatch-shaped handlers for the `image` family (#1095, mu-rsimg).
//!
//! Each handler takes injected values plus the privileged seam and
//! returns `(exit, stdout, stderr)` byte-identical to the Python
//! edition on the same inputs, so the shared core can wire it with
//! one dispatch arm. Handlers are unwired until that arm lands.
//! Status needs no privilege: its runner argument is accepted for
//! the family shape and never invoked. Config loading is the
//! caller's: the Rust loader never activates (mounts) images, so
//! unlike the Python CLI path this handler never mounts as a side
//! effect of reading status (the deliberate divergence recorded in
//! the milestone-1 inventory); on identical post-load inputs its
//! bytes match the Python handler.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::image_mounts::MountEntry;
use crate::image_ops::PrivilegedRunner;
use crate::image_record::{all_images, ImageRecord};
use crate::image_status::{usage_report, VfsFacts};

/// One handler result: process exit code and the exact bytes the
/// Python edition would print to stdout and stderr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HandlerOutcome {
    pub(crate) exit: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Resolves, per image file and mount point, the mounted backend
/// (`kernel`/`fuse`) and the statvfs facts when mounted.
pub(crate) type MountFactsFn<'a> = dyn Fn(&Path, &Path) -> (Option<String>, Option<VfsFacts>) + 'a;

fn refusal(message: &str, command: &str) -> HandlerOutcome {
    HandlerOutcome {
        exit: 3,
        stdout: String::new(),
        stderr: format!(
            "REFUSED: {message}\nREMEDY: run 'wrkslots {command} --help', correct the named condition, then rerun 'wrkslots {command}' with the required arguments\n"
        ),
    }
}

/// Escape a string the way Python `json.dumps` does by default
/// (`ensure_ascii=True`): quotes, backslash, control characters,
/// and every non-ASCII scalar as `\uXXXX` (with surrogate pairs).
fn py_json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch if (ch as u32) < 0x7f => out.push(ch),
            ch => {
                let mut buffer = [0_u16; 2];
                for unit in ch.encode_utf16(&mut buffer) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

fn py_json_value(value: &Value, indent: usize) -> String {
    // Only the shapes `usage_report` produces reach this renderer:
    // flat objects with string keys and scalar/array/object values.
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let pad = "  ".repeat(indent + 1);
            let close_pad = "  ".repeat(indent);
            let parts: Vec<String> = map
                .iter()
                .map(|(key, item)| {
                    format!(
                        "{pad}{}: {}",
                        py_json_string(key),
                        py_json_value(item, indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{close_pad}}}", parts.join(",\n"))
        }
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let pad = "  ".repeat(indent + 1);
            let close_pad = "  ".repeat(indent);
            let parts: Vec<String> = items
                .iter()
                .map(|item| format!("{pad}{}", py_json_value(item, indent + 1)))
                .collect();
            format!("[\n{}\n{close_pad}]", parts.join(",\n"))
        }
        Value::String(text) => py_json_string(text),
        other => other.to_string(),
    }
}

/// The JSON rendering of `image status`, in the Python dict's
/// insertion order (top level and per-report), which a sorted map
/// would not reproduce.
fn render_status_json_ordered(new_slot_representation: &str, reports: &[Value]) -> String {
    let image_parts: Vec<String> = reports
        .iter()
        .map(|report| {
            let entry = |label: &str| -> String {
                let Some(entry) = report.get(label) else {
                    return "null".to_string();
                };
                let keys = [
                    "image",
                    "ceiling_bytes",
                    "host_allocated_bytes",
                    "error",
                    "mounted",
                    "backend",
                    "used_bytes",
                    "free_bytes",
                    "used_inodes",
                ];
                let parts: Vec<String> = keys
                    .iter()
                    .filter_map(|key| {
                        entry.get(key).map(|value| {
                            format!("      {}: {}", py_json_string(key), py_json_value(value, 3))
                        })
                    })
                    .collect();
                format!("{{\n{}\n    }}", parts.join(",\n"))
            };
            format!(
                "  {{\n    \"name\": {},\n    \"slot_type\": {},\n    \"location\": {},\n    \"phase\": {},\n    \"slot\": {},\n    \"state\": {}\n  }}",
                py_json_value(report.get("name").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("slot_type").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("location").unwrap_or(&Value::Null), 1),
                py_json_value(report.get("phase").unwrap_or(&Value::Null), 1),
                entry("slot"),
                entry("state"),
            )
        })
        .collect();
    format!(
        "{{\n  \"new_slot_representation\": {},\n  \"images\": [{}]\n}}\n",
        py_json_string(new_slot_representation),
        if image_parts.is_empty() {
            String::new()
        } else {
            format!("\n{}\n  ", image_parts.join(",\n"))
        }
    )
}

/// `wrkslots image status [--format human|json]`, mirroring Python
/// `_cmd_image_status` (`imagecmd.py:865`). `facts` resolves, per
/// image file and mount point, the mounted backend and statvfs
/// facts; production derives them from the live mount table, tests
/// inject fixture facts. `_runner` keeps the family signature;
/// status never invokes it.
pub(crate) fn image_status(
    control: &Path,
    slot_path: &dyn Fn(&str, &str) -> PathBuf,
    new_slot_representation: &str,
    format: &str,
    _entries: &[MountEntry],
    facts: &MountFactsFn<'_>,
    _runner: &dyn PrivilegedRunner,
) -> HandlerOutcome {
    let images: Vec<ImageRecord> = match all_images(control, slot_path) {
        Ok(images) => images,
        Err(message) => return refusal(&message, "image"),
    };
    let reports: Vec<Value> = images
        .iter()
        .map(|record| {
            // The core takes backend and vfs closures separately;
            // both resolve through the one injected facts closure,
            // keyed by the record's own image/mount pairs.
            let pairs = [
                (record.image_file(), record.location.clone()),
                (record.state_image(), record.state_mount()),
            ];
            usage_report(
                record,
                &|image_file: &Path, mount_point: &Path| facts(image_file, mount_point).0,
                &|mount_point: &Path| {
                    pairs
                        .iter()
                        .find(|(_, mount)| mount == mount_point)
                        .and_then(|(image_file, mount)| facts(image_file, mount).1)
                },
            )
        })
        .collect();
    if format == "json" {
        return HandlerOutcome {
            exit: 0,
            stdout: render_status_json_ordered(new_slot_representation, &reports),
            stderr: String::new(),
        };
    }
    HandlerOutcome {
        exit: 0,
        stdout: crate::image_status::render_status_human(new_slot_representation, &reports),
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_ops::RunOutcome;
    use crate::image_record::{image_directory, write_record};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct NoRunner;
    impl PrivilegedRunner for NoRunner {
        fn run(&self, _spec: &crate::image_ops::CommandSpec) -> Result<RunOutcome, String> {
            Err("status must not run commands".to_string())
        }
    }

    #[test]
    fn status_renders_python_bytes() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("wrkslotsd-imagecmd-{}-{n}", std::process::id()));
        let control = root.join("control");
        let record = ImageRecord {
            directory: image_directory(&control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: root.join("slots").join("slot-a"),
            ceiling_bytes: 8 * 1_073_741_824,
            state_ceiling_bytes: 2 * 1_073_741_824,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        std::fs::create_dir_all(&record.directory).expect("dir");
        write_record(&record).expect("record");
        std::fs::File::create(record.image_file())
            .expect("img")
            .set_len(8 * 1_073_741_824)
            .expect("size");
        std::fs::File::create(record.state_image())
            .expect("state")
            .set_len(2 * 1_073_741_824)
            .expect("size");

        let facts = |_: &Path, _: &Path| (None, None);
        let human = image_status(
            &control,
            &|_type, slot| root.join("slots").join(slot),
            "image",
            "human",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(human.exit, 0);
        assert_eq!(human.stderr, "");
        assert_eq!(
            human.stdout,
            format!(
                "new slots: image\nagent/slot-a at {} (ready)\n  slot  mounted=False backend=- host=0.00G used=- ceiling=8.00G inodes=-\n  state mounted=False backend=- host=0.00G used=- ceiling=2.00G inodes=-\n",
                root.join("slots").join("slot-a").display()
            )
        );

        let json = image_status(
            &control,
            &|_type, slot| root.join("slots").join(slot),
            "image",
            "json",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(json.exit, 0);
        let expected_json = format!(
            "{{\n  \"new_slot_representation\": \"image\",\n  \"images\": [\n  {{\n    \"name\": \"slot-a\",\n    \"slot_type\": \"agent\",\n    \"location\": \"{}\",\n    \"phase\": \"ready\",\n    \"slot\": {{\n      \"image\": \"{}\",\n      \"ceiling_bytes\": 8589934592,\n      \"host_allocated_bytes\": 0,\n      \"mounted\": false\n    }},\n    \"state\": {{\n      \"image\": \"{}\",\n      \"ceiling_bytes\": 2147483648,\n      \"host_allocated_bytes\": 0,\n      \"mounted\": false\n    }}\n  }}\n  ]\n}}\n",
            root.join("slots").join("slot-a").display(),
            record.image_file().display(),
            record.state_image().display(),
        );
        assert_eq!(json.stdout, expected_json);

        // Empty registry renders the Python empty forms.
        let empty_control = root.join("empty-control");
        std::fs::create_dir_all(&empty_control).expect("empty control");
        let empty = image_status(
            &empty_control,
            &|_type, slot| root.join("slots").join(slot),
            "worktree",
            "human",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(empty.stdout, "new slots: worktree\nno image-backed slots\n");
        let empty_json = image_status(
            &empty_control,
            &|_type, slot| root.join("slots").join(slot),
            "worktree",
            "json",
            &[],
            &facts,
            &NoRunner,
        );
        assert_eq!(
            empty_json.stdout,
            "{\n  \"new_slot_representation\": \"worktree\",\n  \"images\": []\n}\n"
        );

        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
