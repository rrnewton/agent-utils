//! Mount-table evidence for slot images (P8/P7, mu-rsimg).
//!
//! Pure counterparts of Python `slotimage`'s mountinfo half:
//! `_parse_mount_table`, `_top_entry`, `parse_device`, and the
//! classification core of `mount_state`. Nothing here reads
//! `/proc`, sysfs, or udev and nothing mounts: callers inject the
//! mount table and the loop-carrier facts they observed, so tests
//! run on fixture mountinfo text in temp directories only. The
//! loop identity rule is content, never rendered path (see the
//! Python module's warning at `slotimage.py:734`): a carrier is
//! proven by file identity or file-system UUID, not by comparing
//! backing-file path strings.

use std::path::{Component, Path, PathBuf};

use crate::image_record::{ImageRecord, DISCARD_PHASE};

/// One mountinfo row, mirroring Python `slotimage.MountEntry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountEntry {
    pub(crate) mount_point: PathBuf,
    pub(crate) fstype: String,
    pub(crate) source: String,
    pub(crate) device: String,
    pub(crate) root: String,
}

fn unescape_mount_field(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// Parse mountinfo text, mirroring Python `_parse_mount_table`
/// (`slotimage.py:712`): malformed rows are skipped, not refused.
pub(crate) fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if left_fields.len() < 5 || right_fields.len() < 2 {
            continue;
        }
        entries.push(MountEntry {
            mount_point: PathBuf::from(unescape_mount_field(left_fields[4])),
            fstype: right_fields[0].to_string(),
            source: unescape_mount_field(right_fields[1]),
            device: left_fields[2].to_string(),
            root: unescape_mount_field(left_fields[3]),
        });
    }
    entries
}

pub(crate) fn normalize_absolute(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The topmost entry at `mount_point`, mirroring Python
/// `_top_entry`: with stacked mounts the last matching row decides.
pub(crate) fn top_entry<'a>(
    mount_point: &Path,
    entries: &'a [MountEntry],
) -> Option<&'a MountEntry> {
    let target = normalize_absolute(mount_point);
    entries
        .iter()
        .rev()
        .find(|entry| normalize_absolute(&entry.mount_point) == target)
}

/// `major:minor` from a mountinfo device field, as a dev_t-style
/// combined number, mirroring Python `parse_device` consumers.
pub(crate) fn parse_device(text: &str) -> Option<u64> {
    let (major, minor) = text.split_once(':')?;
    let major: u64 = major.parse().ok()?;
    let minor: u64 = minor.parse().ok()?;
    Some((major << 8) | minor)
}

pub(crate) fn device_major(device: u64) -> u64 {
    device >> 8
}

/// Loop-carrier facts observed by the caller for one mount, kept
/// separate from the pure classification so no fixture needs a
/// real loop device. All fields describe the top mount's device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MountFacts {
    /// FUSE mounts: the mount source is the image file itself.
    pub(crate) source_is_image: bool,
    /// ext4 loop mounts: mounted fsid equals the image UUID's fsid.
    pub(crate) fsid_matches: Option<bool>,
    /// The loop device for the top mount is bound at all.
    pub(crate) loop_bound: bool,
    /// Content verdict: the bound device carries this image.
    pub(crate) carries: Option<bool>,
    /// Device name for the unverifiable message (`loop3`).
    pub(crate) loop_name: String,
    /// Backing rendering for the unverifiable message.
    pub(crate) loop_backing: String,
}

/// Classify what is mounted at a mount point, mirroring Python
/// `mount_state` (`slotimage.py:953`) given the observed facts.
/// Returns `(state, detail)` with state one of `absent`, `kernel`,
/// `fuse`, `other`, or `unverifiable`.
pub(crate) fn classify_mount(top: Option<&MountEntry>, facts: &MountFacts) -> (String, String) {
    let Some(top) = top else {
        return ("absent".to_string(), "nothing is mounted there".to_string());
    };
    let what = format!(
        "{} {} ({}, root {})",
        top.fstype, top.source, top.device, top.root
    );
    if top.fstype.starts_with("fuse") {
        return if facts.source_is_image {
            ("fuse".to_string(), what)
        } else {
            ("other".to_string(), what)
        };
    }
    let number = parse_device(&top.device);
    if top.fstype != "ext4" || number.map(|value| device_major(value) != 7).unwrap_or(true) {
        return ("other".to_string(), what);
    }
    if top.root != "/" {
        return (
            "other".to_string(),
            format!("{what}: a bind of part of a file system, not an image's own mount"),
        );
    }
    if let Some(matches) = facts.fsid_matches {
        return if matches {
            ("kernel".to_string(), what)
        } else {
            ("other".to_string(), what)
        };
    }
    if !facts.loop_bound {
        return (
            "unverifiable".to_string(),
            format!("{what}: the loop device is not bound"),
        );
    }
    match facts.carries {
        Some(true) => ("kernel".to_string(), what),
        Some(false) => ("other".to_string(), what),
        None => (
            "unverifiable".to_string(),
            format!(
                "{what}: cannot tell which image {} holds (its backing file renders as '{}', which does not resolve here, and udev has no file system UUID for it)",
                facts.loop_name, facts.loop_backing
            ),
        ),
    }
}

/// `kernel` or `fuse` when exactly this image is mounted, mirroring
/// Python `mounted_backend`; `None` otherwise.
pub(crate) fn mounted_backend(state: &str) -> Option<&str> {
    if state == "kernel" || state == "fuse" {
        Some(state)
    } else {
        None
    }
}

/// Hook 3 (P7): the finish/remove mounted-state gate for one
/// image-backed slot, with the shared core's Python-identical
/// refusal text (first ported in `storage_state::unmounted_image_detail`).
/// Returns `None` when the gate passes (mounted by kernel or FUSE,
/// or not image-backed), else the refusal detail. `held_labels`
/// names loop devices still holding a discarding image.
pub(crate) fn mounted_gate_refusal(
    record: Option<&ImageRecord>,
    state: &str,
    detail: &str,
    held_labels: &[String],
) -> Option<String> {
    let record = record?;
    if record.phase != "ready" && record.phase != DISCARD_PHASE {
        return None;
    }
    if state == "kernel" || state == "fuse" {
        return None;
    }
    let slot_path = &record.location;
    let slot_image = record.image_file();
    if record.phase == DISCARD_PHASE {
        return Some(format!(
            "no Git check, salvage or repair runs against {}, which is an unmounted image's mount point: slot image {} was unmounted while a stale copy of it was being discarded{}; it is not mounted again: a mount would reuse a loop device that may hold the stale file system, and a stale copy released after the host's own file system may have written its old metadata into the image. remedy: stop the processes whose mount table holds that device; once no loop device holds the image, check the image file (e2fsck -fn) and run 'wrkslots image mount --coordinator-authorized', which mounts it from the file only",
            slot_path.display(),
            slot_image.display(),
            if held_labels.is_empty() {
                String::new()
            } else {
                format!(", and {} still holds it", held_labels.join(", "))
            },
        ));
    }
    Some(format!(
        "slot image {} is not mounted at {} ({detail}); no Git check, salvage or repair runs against an unmounted image's mount point. remedy: run 'wrkslots image mount' and resolve what it reports",
        slot_image.display(),
        slot_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_record::ImageRecord;

    const MOUNTINFO: &str = "\
24 1 0:22 / /proc rw,nosuid,nodev,noexec shared:5 - proc proc rw
36 24 7:3 / /srv/slots/slot-a rw,nosuid,nodev,noatime shared:27 - ext4 /dev/loop3 rw,discard
37 24 7:3 /sub /srv/slots/slot-a rw shared:28 - ext4 /dev/loop3 rw
38 24 0:50 / /srv/slots/slot-b rw - fuse2fs /tmp/fixture/slot.img rw
garbage row without separator
";

    fn entries() -> Vec<MountEntry> {
        parse_mountinfo(MOUNTINFO)
    }

    #[test]
    fn parses_and_stacks_topmost_first() {
        let rows = entries();
        assert_eq!(rows.len(), 4);
        // Stacked mounts: the later (topmost) row at slot-a decides.
        let top = top_entry(Path::new("/srv/slots/slot-a"), &rows).expect("top");
        assert_eq!(top.root, "/sub");
        let (state, detail) = classify_mount(Some(top), &MountFacts::default());
        assert_eq!(state, "other");
        assert!(
            detail.contains("a bind of part of a file system"),
            "{detail}"
        );
    }

    #[test]
    fn classifies_absent_fuse_and_kernel() {
        let rows = entries();
        let (state, detail) = classify_mount(
            top_entry(Path::new("/srv/none"), &rows),
            &MountFacts::default(),
        );
        assert_eq!(
            (state.as_str(), detail.as_str()),
            ("absent", "nothing is mounted there")
        );

        let fuse_facts = MountFacts {
            source_is_image: true,
            ..MountFacts::default()
        };
        let (state, _) = classify_mount(
            top_entry(Path::new("/srv/slots/slot-b"), &rows),
            &fuse_facts,
        );
        assert_eq!(state, "fuse");
        assert_eq!(mounted_backend(&state), Some("fuse"));

        let kernel_facts = MountFacts {
            fsid_matches: Some(true),
            ..MountFacts::default()
        };
        let single = [MountEntry {
            mount_point: PathBuf::from("/srv/slots/slot-a"),
            fstype: "ext4".to_string(),
            source: "/dev/loop3".to_string(),
            device: "7:3".to_string(),
            root: "/".to_string(),
        }];
        let (state, _) = classify_mount(single.first(), &kernel_facts);
        assert_eq!(state, "kernel");

        let unbound = MountFacts {
            loop_bound: false,
            ..MountFacts::default()
        };
        let (state, detail) = classify_mount(single.first(), &unbound);
        assert_eq!(state, "unverifiable");
        assert!(detail.contains("the loop device is not bound"), "{detail}");
    }

    #[test]
    fn gate_refuses_unmounted_and_passes_mounted() {
        let record = ImageRecord {
            directory: PathBuf::from("/srv/control/slot-images/agent/slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: PathBuf::from("/srv/slots/slot-a"),
            ceiling_bytes: 1,
            state_ceiling_bytes: 1,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        };
        assert_eq!(mounted_gate_refusal(Some(&record), "kernel", "", &[]), None);
        assert_eq!(mounted_gate_refusal(None, "absent", "", &[]), None);
        let refusal =
            mounted_gate_refusal(Some(&record), "absent", "nothing is mounted there", &[])
                .expect("refusal");
        assert!(refusal.contains("run 'wrkslots image mount'"), "{refusal}");
        assert!(refusal.contains("slot-a"), "{refusal}");
    }
}
