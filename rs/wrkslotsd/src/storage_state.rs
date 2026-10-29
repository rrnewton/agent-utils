//! Registry storage inconsistency parity (filesystem scope).
//!
//! Reproduces the filesystem half of the existing storage diagnosis:
//! slot-directory presence and safety, nested checkout entries, and
//! managed-directory enumeration. Git worktree registration/identity
//! checks live in the git storage diagnosis. An image-backed slot
//! whose image is not mounted reports `image-not-mounted` and is not
//! inspected further. Read-only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, Cfg};
use crate::global_state::discover_machines;

type Result<T> = std::result::Result<T, String>;

#[allow(clippy::too_many_arguments)]
fn note(
    kind: &str,
    scope: &str,
    slot: Option<&str>,
    slot_type: Option<&str>,
    machine: Option<&str>,
    checkout: Option<&str>,
    detail: String,
    remedy: &str,
) -> Value {
    json!({
        "kind": kind,
        "scope": scope,
        "slot": slot,
        "slot_type": slot_type,
        "machine": machine,
        "checkout": checkout,
        "detail": detail,
        "remedy": remedy,
    })
}

/// Format a filesystem name the way Python's `!r` formats a string.
pub(crate) fn py_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::new();
    output.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            ch if ch == quote => {
                output.push('\\');
                output.push(ch);
            }
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if (ch as u32) < 0x20 || (ch as u32) == 0x7f => {
                output.push_str(&format!("\\x{:02x}", ch as u32));
            }
            ch => output.push(ch),
        }
    }
    output.push(quote);
    output
}

pub(crate) fn managed_root(cfg: &Cfg) -> PathBuf {
    if cfg
        .worktrees
        .file_name()
        .is_some_and(|name| name == "slots")
    {
        cfg.worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        cfg.worktrees.clone()
    }
}

pub(crate) fn slot_root(cfg: &Cfg, slot_type: &str) -> PathBuf {
    if slot_type == "validate" {
        managed_root(cfg).join("validate")
    } else {
        cfg.worktrees.clone()
    }
}

/// Why an image-backed slot's image is not mounted at `slot_path`, or None.
///
/// Mirrors the Python `_unmounted_slot_image` and `slotimage` record /
/// mount-state validation: a corrupt or misplaced image record refuses
/// (never silently "not image-backed"), and a mount counts only when it
/// provably holds this slot's own image file — an unrelated filesystem
/// or a bind of part of one at the slot path still refuses, because the
/// empty mount point resolves Git to the enclosing repository.
pub(crate) fn unmounted_image_detail(
    cfg: &Cfg,
    slot: &str,
    slot_type: &str,
    slot_path: &Path,
) -> Option<String> {
    let image_dir = cfg.control.join("slot-images").join(slot_type).join(slot);
    let record_path = image_dir.join("IMAGE.json");
    if record_path.symlink_metadata().is_err() {
        return None;
    }
    let (phase, location) = match load_image_record(&record_path, slot, slot_type, slot_path) {
        Ok(record) => record,
        Err(message) => {
            return Some(format!(
                "cannot read the slot image record of {slot}: {message}"
            ));
        }
    };
    if phase != "ready" && phase != "discarding-stale-copy" {
        return None;
    }
    if Path::new(&location) != slot_path {
        return None;
    }
    let slot_image = image_dir.join("slot.img");
    let (state, detail) = mount_state(&slot_image, Path::new(&location));
    if state == "kernel" || state == "fuse" {
        return None;
    }
    if phase == "discarding-stale-copy" {
        let held = still_attached_labels(&slot_image);
        return Some(format!(
            "no Git check, salvage or repair runs against {}, which is an unmounted image's mount point: slot image {} was unmounted while a stale copy of it was being discarded{}; it is not mounted again: a mount would reuse a loop device that may hold the stale file system, and a stale copy released after the host's own file system may have written its old metadata into the image. remedy: stop the processes whose mount table holds that device; once no loop device holds the image, check the image file (e2fsck -fn) and run 'wrkslots image mount --coordinator-authorized', which mounts it from the file only",
            slot_path.display(),
            slot_image.display(),
            if held.is_empty() {
                String::new()
            } else {
                format!(", and {} still holds it", held.join(", "))
            },
        ));
    }
    Some(format!(
        "slot image {} is not mounted at {} ({detail}); no Git check, salvage or repair runs against an unmounted image's mount point. remedy: run 'wrkslots image mount' and resolve what it reports",
        slot_image.display(),
        slot_path.display()
    ))
}

/// Validate one IMAGE.json the way `slotimage.read_record` does,
/// returning (phase, location) or the oracle's refusal text.
fn load_image_record(
    record_path: &Path,
    slot: &str,
    slot_type: &str,
    slot_path: &Path,
) -> std::result::Result<(String, String), String> {
    let text = std::fs::read_to_string(record_path)
        .map_err(|error| format!("cannot read slot image record {record_path:?}: {error}"))?;
    let image: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read slot image record {record_path:?}: {error}"))?;
    if !image.is_object() {
        return Err(format!(
            "slot image record {} is not an object",
            record_path.display()
        ));
    }
    let schema = image.get("schema").unwrap_or(&Value::Null);
    if schema.as_str() != Some("wrkslots-slot-image/v1") {
        let shown = match schema {
            Value::String(text) => format!("'{text}'"),
            Value::Null => "None".to_string(),
            other => other.to_string(),
        };
        return Err(format!(
            "slot image record {} has unsupported schema {shown}",
            record_path.display()
        ));
    }
    let field_str = |key: &str| -> std::result::Result<String, String> {
        image
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "slot image record {} has an invalid '{key}'",
                    record_path.display()
                )
            })
    };
    let record_slot = field_str("slot")?;
    let record_type = field_str("slot_type")?;
    if record_slot != slot || record_type != slot_type {
        return Err(format!(
            "slot image record {} names another slot ({record_type}/{record_slot})",
            record_path.display()
        ));
    }
    for key in ["ceiling_bytes", "state_ceiling_bytes"] {
        if image.get(key).and_then(Value::as_i64).is_none() {
            return Err(format!(
                "slot image record {} has an invalid '{key}'",
                record_path.display()
            ));
        }
    }
    for key in ["backend", "created_at", "phase"] {
        field_str(key)?;
    }
    let location = field_str("location")?;
    if !Path::new(&location).is_absolute() {
        return Err(format!(
            "slot image record {} has a relative location",
            record_path.display()
        ));
    }
    // The oracle builds a Path (collapsing `.` and duplicate
    // separators at construction) and refuses when normpath still
    // differs — in practice a recorded `..` segment.
    if location.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "slot image record {} has a non-canonical location {location}",
            record_path.display()
        ));
    }
    // Allowed placements besides the slot's canonical directory: a
    // `.{slot}.<suffix>` fence sibling, or the conversion staging dir.
    let location_path = Path::new(&location);
    if location_path != slot_path {
        let name = location_path
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fence_suffix = name.strip_prefix(&format!(".{slot}."));
        let fence = fence_suffix
            .map(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            })
            .unwrap_or(false)
            && location_path.parent() == slot_path.parent();
        let staging = name.starts_with(&format!(".{slot}.convert-"))
            && name[".".len() + slot.len() + ".convert-".len()..]
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
            && name.len() == ".".len() + slot.len() + ".convert-".len() + 8
            && location_path.parent() == record_path.parent().and_then(Path::parent);
        if !fence && !staging {
            return Err(format!(
                "slot image record {} places slot {slot_type}/{slot} at {location}, not at {}; refusing to mount or remove anything there",
                record_path.display(),
                slot_path.display()
            ));
        }
    }
    Ok((field_str("phase")?, location))
}

fn normalize_absolute(path: &Path) -> PathBuf {
    use std::path::Component;
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

struct MountEntry {
    mount_point: String,
    fstype: String,
    source: String,
    device: String,
    root: String,
}

fn mount_entries() -> std::result::Result<Vec<MountEntry>, String> {
    if in_coordinator_box() {
        // The host mount table is read through the user manager by the
        // Python edition; from inside the box this reader can only see
        // a box-private copy, so fail closed as unverifiable rather
        // than trust a stale mount.
        return Err(
            "the host mount table is not readable from inside a coordinator box".to_string(),
        );
    }
    let text = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("cannot read /proc/self/mountinfo: {error}"))?;
    Ok(parse_mountinfo(&text))
}

fn in_coordinator_box() -> bool {
    match std::env::var("WRKSLOTS_SANDBOX").as_deref() {
        Ok("userns") | Ok("root") => std::env::var_os("WRKSLOTS_BOX").is_some(),
        _ => false,
    }
}

fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
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
            mount_point: unescape_mount_field(left_fields[4]),
            fstype: right_fields[0].to_string(),
            source: unescape_mount_field(right_fields[1]),
            device: left_fields[2].to_string(),
            root: unescape_mount_field(left_fields[3]),
        });
    }
    entries
}

fn unescape_mount_field(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

fn image_uuid(image_file: &Path) -> Option<[u8; 16]> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(image_file).ok()?;
    file.seek(SeekFrom::Start(1024)).ok()?;
    let mut block = [0_u8; 0x78];
    file.read_exact(&mut block).ok()?;
    if u16::from_le_bytes([block[0x38], block[0x39]]) != 0xEF53 {
        return None;
    }
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&block[0x68..0x78]);
    Some(uuid)
}

fn uuid_fsid(uuid: &[u8; 16]) -> u64 {
    let low = u64::from_le_bytes(uuid[0..8].try_into().expect("uuid low half"));
    let high = u64::from_le_bytes(uuid[8..16].try_into().expect("uuid high half"));
    low ^ high
}

fn uuid_text(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn parse_device(text: &str) -> Option<u64> {
    let (major, minor) = text.split_once(':')?;
    if !major.chars().all(|ch| ch.is_ascii_digit()) || !minor.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let major: u32 = major.parse().ok()?;
    let minor: u32 = minor.parse().ok()?;
    Some(make_device(major, minor))
}

/// Linux device-number encoding (the `makedev`/`major`/`minor`
/// macros) as pure arithmetic, so no unsafe libc calls are needed.
fn make_device(major: u32, minor: u32) -> u64 {
    let major = major as u64;
    let minor = minor as u64;
    ((major & 0xffff_f000) << 32)
        | ((major & 0xfff) << 8)
        | ((minor & 0xfff_ff00) << 12)
        | (minor & 0xff)
}

fn device_major(device: u64) -> u32 {
    (((device >> 32) & 0xffff_f000) | ((device >> 8) & 0xfff)) as u32
}

fn device_minor(device: u64) -> u32 {
    (((device >> 12) & 0xfff_ff00) | (device & 0xff)) as u32
}

fn mounted_fsid(path: &Path, device: u64) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path).ok()?;
    if meta.dev() != device || meta.ino() != 2 {
        return None;
    }
    rustix::fs::statvfs(path).ok().map(|stat| stat.f_fsid)
}

struct LoopEvidence {
    name: String,
    backing: String,
    deleted: bool,
    file_identity: Option<(u64, u64)>,
    fsid: Option<u64>,
    udev_uuid: Option<String>,
}

fn loop_evidence(entries: &[MountEntry]) -> std::collections::BTreeMap<u64, LoopEvidence> {
    let mut places: std::collections::BTreeMap<u64, Vec<String>> =
        std::collections::BTreeMap::new();
    for entry in entries {
        if entry.root != "/" {
            continue;
        }
        if let Some(number) = parse_device(&entry.device) {
            if device_major(number) == 7 {
                places
                    .entry(number)
                    .or_default()
                    .push(entry.mount_point.clone());
            }
        }
    }
    let mut found = std::collections::BTreeMap::new();
    let Ok(blocks) = std::fs::read_dir("/sys/block") else {
        return found;
    };
    for entry in blocks.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(digits) = name.strip_prefix("loop") else {
            continue;
        };
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let base = entry.path();
        let Ok(backing_raw) = std::fs::read_to_string(base.join("loop/backing_file")) else {
            continue;
        };
        let backing = backing_raw
            .strip_suffix('\n')
            .unwrap_or(&backing_raw)
            .to_string();
        let Ok(dev_text) = std::fs::read_to_string(base.join("dev")) else {
            continue;
        };
        let Some(number) = parse_device(dev_text.trim()) else {
            continue;
        };
        let deleted = backing.ends_with(" (deleted)");
        let mut fsid = None;
        for place in places.get(&number).into_iter().flatten() {
            if let Some(value) = mounted_fsid(Path::new(place), number) {
                fsid = Some(value);
                break;
            }
        }
        found.insert(
            number,
            LoopEvidence {
                name: format!("/dev/{name}"),
                deleted,
                file_identity: if deleted {
                    None
                } else {
                    file_identity(Path::new(&backing))
                },
                fsid,
                udev_uuid: udev_uuid(number),
                backing,
            },
        );
    }
    found
}

fn udev_uuid(device: u64) -> Option<String> {
    let major = device_major(device);
    let minor = device_minor(device);
    let text = std::fs::read_to_string(format!("/run/udev/data/b{major}:{minor}")).ok()?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("E:ID_FS_UUID=") {
            let cleaned: String = value
                .trim()
                .to_lowercase()
                .chars()
                .filter(|ch| *ch != '-')
                .collect();
            if cleaned.len() != 32 || !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return None;
            }
            let mut bytes = [0_u8; 16];
            for index in 0..16 {
                bytes[index] = u8::from_str_radix(&cleaned[index * 2..index * 2 + 2], 16).ok()?;
            }
            return Some(uuid_text(&bytes));
        }
    }
    None
}

fn carries(
    evidence: &LoopEvidence,
    identity: Option<(u64, u64)>,
    uuid: Option<&[u8; 16]>,
) -> Option<bool> {
    if let (Some(fsid), Some(uuid)) = (evidence.fsid, uuid) {
        return Some(fsid == uuid_fsid(uuid));
    }
    if evidence.deleted {
        return Some(false);
    }
    if identity.is_some() && evidence.file_identity == identity {
        return Some(true);
    }
    if let (Some(udev), Some(uuid)) = (&evidence.udev_uuid, uuid) {
        return Some(*udev == uuid_text(uuid));
    }
    None
}

fn still_attached_labels(image_file: &Path) -> Vec<String> {
    let Ok(entries) = mount_entries() else {
        return Vec::new();
    };
    let identity = file_identity(image_file);
    let uuid = image_uuid(image_file);
    loop_evidence(&entries)
        .into_iter()
        .filter(|(_number, evidence)| carries(evidence, identity, uuid.as_ref()) == Some(true))
        .map(|(number, evidence)| {
            format!(
                "{} ({}:{})",
                evidence.name,
                device_major(number),
                device_minor(number)
            )
        })
        .collect()
}

/// What is mounted at `mount_point`, judged against `image_file`:
/// (state, detail) with state absent|kernel|fuse|other|unverifiable,
/// mirroring `slotimage.mount_state`.
fn mount_state(image_file: &Path, mount_point: &Path) -> (String, String) {
    let entries = match mount_entries() {
        Ok(entries) => entries,
        Err(message) => return ("unverifiable".to_string(), message),
    };
    let target = normalize_absolute(mount_point);
    // The oracle's `_top_entry` takes the last matching entry: with
    // stacked mounts, the topmost mount decides.
    let top = entries
        .iter()
        .rev()
        .find(|entry| Path::new(&entry.mount_point) == target);
    let Some(top) = top else {
        return ("absent".to_string(), "nothing is mounted there".to_string());
    };
    let what = format!(
        "{} {} ({}, root {})",
        top.fstype, top.source, top.device, top.root
    );
    if top.fstype.starts_with("fuse") {
        return if file_identity(Path::new(&top.source)) == file_identity(image_file)
            && file_identity(image_file).is_some()
        {
            ("fuse".to_string(), what)
        } else {
            ("other".to_string(), what)
        };
    }
    let number = parse_device(&top.device);
    if top.fstype != "ext4" || number.map(|value| device_major(value) != 7).unwrap_or(true) {
        return ("other".to_string(), what);
    }
    let number = number.expect("loop device checked above");
    if top.root != "/" {
        return (
            "other".to_string(),
            format!("{what}: a bind of part of a file system, not an image's own mount"),
        );
    }
    let uuid = image_uuid(image_file);
    if let Some(uuid) = &uuid {
        if let Some(fsid) = mounted_fsid(mount_point, number) {
            return if fsid == uuid_fsid(uuid) {
                ("kernel".to_string(), what)
            } else {
                ("other".to_string(), what)
            };
        }
    }
    let evidence = loop_evidence(&entries);
    let Some(item) = evidence.get(&number) else {
        return (
            "unverifiable".to_string(),
            format!("{what}: the loop device is not bound"),
        );
    };
    match carries(item, file_identity(image_file), uuid.as_ref()) {
        Some(true) => ("kernel".to_string(), what),
        Some(false) => ("other".to_string(), what),
        None => (
            "unverifiable".to_string(),
            format!(
                "{what}: cannot tell which image {} holds (its backing file renders as {:?}, which does not resolve here, and udev has no file system UUID for it)",
                item.name, item.backing
            ),
        ),
    }
}

pub(crate) fn storage_sort_key(value: &Value) -> (String, String, String, String, String) {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (
        text("machine"),
        text("slot_type"),
        text("slot"),
        text("checkout"),
        text("kind"),
    )
}

/// Diagnose filesystem drift between the active rows and the managed
/// slot directories, printing the sorted findings.
pub(crate) fn parse_storage(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
    let mut findings: Vec<Value> = Vec::new();
    let mut expected: Vec<(String, BTreeSet<String>)> = vec![
        ("agent".to_string(), BTreeSet::new()),
        ("validate".to_string(), BTreeSet::new()),
    ];
    let mut records: Vec<(String, String, String, Value)> = Vec::new();
    for machine in &machines {
        let (active, order) = parse_state_ordered(root, Some(machine))?;
        let slots = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let by_slot: std::collections::HashMap<String, Value> = slots
            .into_iter()
            .filter_map(|slot| {
                let name = slot.get("slot")?.as_str()?.to_string();
                Some((name, slot))
            })
            .collect();
        for item in &order {
            let stored = by_slot.get(&item.slot).cloned().unwrap_or(Value::Null);
            records.push((
                machine.clone(),
                item.slot.clone(),
                item.slot_type.clone(),
                stored,
            ));
        }
    }
    records.sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    for (machine, slot, slot_type, stored) in &records {
        for (known_type, slots) in expected.iter_mut() {
            if known_type == slot_type {
                slots.insert(slot.clone());
            }
        }
        let slot_path = slot_root(&cfg, slot_type).join(slot);
        let present = slot_path.symlink_metadata().is_ok();
        let safe = slot_path.is_dir()
            && !slot_path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if !safe {
            let (kind, detail) = if present {
                (
                    "unsafe-slot-directory",
                    format!(
                        "active row points at an unsafe slot path {}",
                        slot_path.display()
                    ),
                )
            } else {
                (
                    "row-without-directory",
                    format!(
                        "active row but no slot directory at {}",
                        slot_path.display()
                    ),
                )
            };
            findings.push(note(
                kind,
                "row",
                Some(slot),
                Some(slot_type),
                Some(machine),
                None,
                detail,
                "preserve the active row; restore its exact slot directory or use the supported recovery command after proving the storage is intentionally absent",
            ));
            continue;
        }
        if let Some(detail) = unmounted_image_detail(&cfg, slot, slot_type, &slot_path) {
            findings.push(note(
                "image-not-mounted",
                "row",
                Some(slot),
                Some(slot_type),
                Some(machine),
                None,
                detail,
                "run 'wrkslots image mount' and resolve what it reports; do not repair Git registrations while the image is not mounted",
            ));
            continue;
        }
        let layout = stored
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or(&cfg.layout);
        if layout == "nested" {
            let wanted: BTreeSet<String> = stored
                .get("checkouts")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            match std::fs::read_dir(&slot_path) {
                Err(error) => findings.push(note(
                    "slot-unreadable",
                    "row",
                    Some(slot),
                    Some(slot_type),
                    Some(machine),
                    None,
                    format!("cannot inspect {}: {error}", slot_path.display()),
                    "make the recorded slot directory readable, then rerun 'wrkslots status'",
                )),
                Ok(entries) => {
                    let mut actual: BTreeSet<String> = BTreeSet::new();
                    let mut entry_error: Option<std::io::Error> = None;
                    for entry in entries {
                        match entry {
                            Ok(entry) => {
                                actual.insert(entry.file_name().to_string_lossy().into_owned());
                            }
                            Err(error) => {
                                entry_error = Some(error);
                                break;
                            }
                        }
                    }
                    if let Some(error) = entry_error {
                        findings.push(note(
                            "slot-unreadable",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            None,
                            format!("cannot inspect {}: {error}", slot_path.display()),
                            "make the recorded slot directory readable, then rerun 'wrkslots status'",
                        ));
                        continue;
                    }
                    let mut allowed = wanted.clone();
                    if slot_type == "agent" {
                        allowed.insert("HANDOFF.md".to_string());
                    }
                    for name in wanted.difference(&actual) {
                        findings.push(note(
                            "missing-checkout",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            Some(name),
                            format!(
                                "record names checkout {} but it is not in {}",
                                py_repr(name),
                                slot_path.display()
                            ),
                            "preserve the row and repair or recover the named checkout before mutation",
                        ));
                    }
                    for name in actual.difference(&allowed) {
                        findings.push(note(
                            "unexpected-entry",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            Some(name),
                            format!(
                                "slot holds {}, which its record does not name",
                                py_repr(name)
                            ),
                            "inspect and register or relocate the unexpected entry; do not delete it to make the registry look consistent",
                        ));
                    }
                }
            }
        }
    }
    for (slot_type, wanted) in &expected {
        let root_path = slot_root(&cfg, slot_type);
        if root_path.symlink_metadata().is_err() {
            continue;
        }
        let is_symlink = root_path
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !root_path.is_dir() {
            findings.push(note(
                "unsafe-slots-directory",
                "directory",
                None,
                Some(slot_type),
                None,
                None,
                format!(
                    "managed {slot_type} slots directory is unsafe: {}",
                    root_path.display()
                ),
                "restore the configured managed directory as a real directory before mutation",
            ));
            continue;
        }
        let entries = match std::fs::read_dir(&root_path) {
            Ok(entries) => entries,
            Err(error) => {
                findings.push(note(
                    "slots-directory-unreadable",
                    "directory",
                    None,
                    Some(slot_type),
                    None,
                    None,
                    format!("cannot enumerate {}: {error}", root_path.display()),
                    "make the configured managed directory readable, then rerun 'wrkslots status'",
                ));
                continue;
            }
        };
        let mut actual: BTreeSet<String> = BTreeSet::new();
        let mut collected: Vec<std::fs::DirEntry> = Vec::new();
        let mut entry_error: Option<std::io::Error> = None;
        for entry in entries {
            match entry {
                Ok(entry) => collected.push(entry),
                Err(error) => {
                    entry_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = entry_error {
            findings.push(note(
                "slots-directory-unreadable",
                "directory",
                None,
                Some(slot_type),
                None,
                None,
                format!("cannot enumerate {}: {error}", root_path.display()),
                "make the configured managed directory readable, then rerun 'wrkslots status'",
            ));
            continue;
        }
        for entry in collected {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_entry = path.is_dir()
                || path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
            if !is_entry || name.starts_with("EVENTS.") {
                continue;
            }
            if path == cfg.control.join("wrkslots") {
                continue;
            }
            if root_path == cfg.control
                && ["slot-images", "slot-state", "box-state", "wrkslots-salvage"]
                    .contains(&name.as_str())
            {
                continue;
            }
            if slot_type == "agent" && path == slot_root(&cfg, "validate") {
                continue;
            }
            actual.insert(name);
        }
        for slot in actual.difference(wanted) {
            let remedy = if slot_type == "agent" {
                format!(
                    "inspect the directory, then run 'wrkslots register {slot} --help' to record a live agent slot"
                )
            } else {
                "inspect the retained validation directory, then run 'wrkslots import-existing --help'".to_string()
            };
            findings.push(note(
                "directory-without-row",
                "directory",
                Some(slot),
                Some(slot_type),
                None,
                None,
                format!(
                    "{} exists but no active {slot_type} row claims it",
                    root_path.join(slot).display()
                ),
                &remedy,
            ));
        }
    }
    findings.sort_by_key(storage_sort_key);
    Ok(json!({
        "machines": machines,
        "findings": findings,
    }))
}
