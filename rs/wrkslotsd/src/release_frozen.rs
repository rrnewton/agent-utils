//! Command: release one incomplete retained frozen validation
//! checkout on an operator's word (`release-frozen-incomplete`,
//! #1089). Step 4a ported the read-only half (reason and
//! coordinator gates, retention classification, dry-run plan);
//! step 4b-ii wires `--yes`: salvage every unheld HEAD into a
//! verified local bundle, write the release note, and remove
//! the checkout through the journaled ownerless validation
//! removal. Fixture registries only until the edition cutover.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::load_cfg;
use crate::config_validate::validate_name;
use crate::ensure_states::require_fixture;

type Result<T> = std::result::Result<T, String>;

/// Python `_frozen_release_reason`.
fn release_reason(raw: &str) -> Result<String> {
    let reason = raw.trim();
    if reason.is_empty() {
        return Err(
            "release-frozen-incomplete requires --reason saying why this checkout holds nothing anyone needs; nothing was read or changed"
                .to_string(),
        );
    }
    if reason.chars().count() > 2000 || reason.contains('\0') {
        return Err("--reason must be at most 2000 characters without NUL bytes".to_string());
    }
    Ok(reason.to_string())
}

fn random_hex(bytes: usize) -> String {
    use std::io::Read as _;

    let mut raw = vec![0u8; bytes];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut raw);
    }
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn git(path: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(path).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            path.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Python `_ensure_local_salvage_bundle` + receipt, in the
/// self-contained (non-thin) form: bundle one commit from a
/// checkout, verify it in a fresh bare clone, and record the
/// receipt beside it. Returns (bundle, bundle sha256, bytes,
/// receipt).
fn salvage_bundle(
    cfg: &crate::active_state::Cfg,
    checkout: &Path,
    relative: &str,
    head: &str,
    slot: &str,
) -> Result<(PathBuf, String, u64, PathBuf)> {
    let path = if relative == "." {
        checkout.to_path_buf()
    } else {
        checkout.join(relative)
    };
    let name = if relative == "." {
        slot.to_string()
    } else {
        format!("{slot}/{relative}")
    };
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_string();
    let safe = if safe.is_empty() {
        "checkout".to_string()
    } else {
        safe
    };
    let name_digest = &format!("{:x}", Sha256::digest(name.as_bytes()))[..12];
    let directory = cfg
        .control
        .join("wrkslots-salvage")
        .join(&cfg.machine)
        .join(slot)
        .join("ownerless")
        .join(format!(
            "{safe}-{name_digest}-{}",
            &head[..12.min(head.len())]
        ));
    let bundle = directory.join("repository.bundle");
    let receipt = directory.join("receipt.json");
    let archive_ref = format!(
        "refs/heads/wrkslots-salvage/{}/{}/ownerless/{safe}-{name_digest}",
        cfg.machine, slot
    );
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create local salvage archive directory {}: {error}",
            directory.display()
        )
    })?;
    if !bundle.exists() {
        let stage_root = directory.join(format!(".stage-{}", random_hex(8)));
        std::fs::create_dir_all(&stage_root).map_err(|error| {
            format!(
                "cannot create salvage stage {}: {error}",
                stage_root.display()
            )
        })?;
        let result = (|| -> Result<()> {
            let stage = stage_root.join("stage.git");
            git(
                &stage_root,
                &["init", "--bare", "--", &stage.to_string_lossy()],
                &[],
            )?;
            let git_dir = git(&path, &["rev-parse", "--absolute-git-dir"], &[])?
                .trim()
                .to_string();
            let objects = format!("{git_dir}/objects");
            let envs = [
                ("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects.as_str()),
                ("GIT_NO_LAZY_FETCH", "1"),
            ];
            git(&stage, &["update-ref", &archive_ref, head], &envs)?;
            let temp_bundle = directory.join(format!(".repository.bundle.tmp.{}", random_hex(8)));
            git(
                &stage,
                &[
                    "bundle",
                    "create",
                    &temp_bundle.to_string_lossy(),
                    &archive_ref,
                ],
                &envs,
            )?;
            // Verify in a fresh bare clone before publishing.
            let verify = stage_root.join("restored.git");
            git(
                &stage_root,
                &[
                    "clone",
                    "--bare",
                    "--",
                    &temp_bundle.to_string_lossy(),
                    &verify.to_string_lossy(),
                ],
                &[],
            )?;
            let restored = git(&verify, &["rev-parse", "--verify", &archive_ref], &[])?
                .trim()
                .to_string();
            if restored != head {
                return Err(format!(
                    "local salvage bundle ref {archive_ref} names {restored}, not {head}"
                ));
            }
            git(
                &verify,
                &["cat-file", "-e", &format!("{head}^{{tree}}")],
                &[],
            )?;
            git(
                &verify,
                &["fsck", "--full", "--strict", "--no-dangling"],
                &[],
            )?;
            std::fs::rename(&temp_bundle, &bundle).map_err(|error| {
                format!(
                    "cannot publish local salvage bundle {}: {error}",
                    bundle.display()
                )
            })?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&stage_root);
        result?;
    }
    let contents = std::fs::read(&bundle).map_err(|error| {
        format!(
            "cannot read local salvage bundle {}: {error}",
            bundle.display()
        )
    })?;
    let digest = format!("{:x}", Sha256::digest(&contents));
    let (remote_digest, _url) = crate::finish_write::remote_authority(&path, &cfg.default_remote)?;
    let payload = json!({
        "schema": 1,
        "kind": "wrkslots-local-salvage-archive",
        "machine": cfg.machine,
        "slot": slot,
        "generation": Value::Null,
        "checkout": name,
        "repository": relative,
        "remote": cfg.default_remote,
        "remote_url_sha256": remote_digest,
        "source_head": head,
        "salvage_commit": head,
        "status_sha256": format!("{:x}", Sha256::digest(b"")),
        "archive_root": cfg.control.to_string_lossy(),
        "bundle": bundle
            .strip_prefix(&cfg.control)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| bundle.to_string_lossy().into_owned()),
        "bundle_sha256": digest,
        "bundle_bytes": contents.len(),
        "archive_ref": archive_ref,
        "verification": {
            "fresh_bare_clone": true,
            "exact_ref": true,
            "full_fsck": true,
            "complete_history": true,
        },
        "archive_reason": "frozen-incomplete-operator-release",
    });
    if !receipt.exists() {
        std::fs::write(
            &receipt,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
        )
        .map_err(|error| {
            format!(
                "cannot write local salvage receipt {}: {error}",
                receipt.display()
            )
        })?;
    }
    Ok((bundle, digest, contents.len() as u64, receipt))
}

/// The release note text, Python `_frozen_release_note_text`.
fn note_text(release: &Value, plan: &Value, outcome: &str, recorded: &str) -> String {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let operator = release.get("operator").cloned().unwrap_or(Value::Null);
    let mut lines = vec![
        format!(
            "# Frozen validation checkout release {}",
            text(release, "release_id")
        ),
        String::new(),
        format!("- Recorded: {recorded}"),
        format!(
            "- Operator: {} (uid {}), coordinator PID {}",
            text(&operator, "user"),
            operator
                .get("uid")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            operator
                .get("coordinator_pid")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        ),
        format!("- Reason: {}", text(release, "reason")),
        format!("- Checkout: {}", text(plan, "checkout")),
        format!(
            "- Run record: {} (sha256 {})",
            text(plan, "record"),
            text(plan, "record_sha256")
        ),
        format!("- Target: {}", text(plan, "target")),
        format!(
            "- Size: {} bytes in {} files",
            plan.get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            plan.get("files")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        ),
        "- Salvage:".to_string(),
    ];
    let salvage = release
        .get("salvage")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if salvage.is_empty() {
        lines.push("  - none needed: every HEAD is already on a remote".to_string());
    }
    for item in &salvage {
        lines.push(format!(
            "  - {} {}: bundle {} (sha256 {}, {} bytes), receipt {}",
            text(item, "repository"),
            text(item, "commit"),
            text(item, "bundle"),
            text(item, "bundle_sha256"),
            item.get("bundle_bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            text(item, "receipt"),
        ));
    }
    lines.push(format!("- Outcome: {outcome}"));
    lines.push(String::new());
    lines.push(
        "The ownerless-validate-path-removed event in the operation log carries the same release id, reason and operator."
            .to_string(),
    );
    lines.push(String::new());
    lines.join("\n")
}

fn write_note(
    cfg: &crate::active_state::Cfg,
    release: &Value,
    plan: &Value,
    outcome: &str,
) -> Result<PathBuf> {
    let relative = release
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let note = cfg.control.join(relative);
    if let Some(parent) = note.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create frozen release note directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let recorded = crate::ensure_states::utc_now();
    std::fs::write(&note, note_text(release, plan, outcome, &recorded)).map_err(|error| {
        format!(
            "cannot write frozen release note {}: {error}",
            note.display()
        )
    })?;
    Ok(note)
}

fn username() -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return user;
        }
    }
    let uid = rustix::process::getuid().as_raw();
    if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
        for line in passwd.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.get(2).and_then(|value| value.parse::<u32>().ok()) == Some(uid) {
                return fields[0].to_string();
            }
        }
    }
    uid.to_string()
}

/// Run the release classification and plan; with `--yes`,
/// salvage, note, and remove through the journaled removal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn release_frozen_incomplete(
    root: Option<&Path>,
    machine_override: Option<&str>,
    frozen_checkout: &str,
    completed_record: &str,
    repository: &str,
    reason: &str,
    yes: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    let reason = release_reason(reason)?;
    if !coordinator_authorized {
        return Err(
            "frozen validation release is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                .to_string(),
        );
    }
    require_fixture()?;
    let pid = coordinator_pid.ok_or("frozen validation release requires --coordinator-pid")?;
    let coordinator = crate::status_state::read_process_identity(pid)?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let config = crate::config_validate::load_config(root, machine_override)?;
    crate::ensure_states::refuse_partial_state_allow_seals(&cfg.control)?;
    if !crate::classify_journals::outstanding_journal_paths(&cfg)?.is_empty() {
        return Err(
            "an interrupted mutation is already recorded; recover it before releasing a frozen validation checkout"
                .to_string(),
        );
    }
    let plan = crate::validation_recovery::frozen_release_dry_run(
        &cfg,
        &config,
        frozen_checkout,
        completed_record,
        repository,
        &reason,
    )?;
    if !yes {
        return Ok(plan);
    }
    // Salvage every HEAD no remote holds, then note, then remove.
    let checkout = PathBuf::from(
        plan.get("checkout")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let slot = checkout
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut salvage: Vec<Value> = Vec::new();
    if let Some(repositories) = plan.get("repositories").and_then(Value::as_array) {
        for row in repositories {
            if row.get("on_remote").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let relative = row
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let head = row.get("head").and_then(Value::as_str).unwrap_or_default();
            let (bundle, digest, bytes, receipt) =
                salvage_bundle(&cfg, &checkout, relative, head, &slot)?;
            salvage.push(json!({
                "repository": relative,
                "commit": head,
                "bundle": bundle.to_string_lossy(),
                "bundle_sha256": digest,
                "bundle_bytes": bytes,
                "receipt": receipt.to_string_lossy(),
            }));
        }
    }
    let release_id = random_hex(16);
    let release = json!({
        "release_id": release_id,
        "reason": reason,
        "operator": {
            "user": username(),
            "uid": rustix::process::getuid().as_raw(),
            "coordinator_pid": pid,
        },
        "note": format!("wrkslots-salvage/frozen-releases/{release_id}.md"),
        "salvage": salvage,
    });
    crate::validation_recovery::frozen_operator_release_from_obj(&release)?;
    write_note(
        &cfg,
        &release,
        &plan,
        "authorized; removal is journaled under this release id",
    )?;
    let actor = crate::status_state::identity_to_value(&coordinator);
    let journal_value = crate::validation_recovery::ownerless_validation_journal_with_release(
        &cfg,
        &config,
        "frozen-checkout",
        frozen_checkout,
        Some(completed_record),
        Some(repository),
        None,
        Some(&release),
        &actor,
    )?;
    let mut journal_map = journal_value
        .as_object()
        .cloned()
        .ok_or_else(|| "ownerless validation journal must be an object".to_string())?;
    crate::remove_write::write_journal(&cfg, &selected, &journal_value)?;
    let roots = crate::validation_exclusion::exclusion_roots();
    if let Err(error) = crate::validation_exclusion::execute_ownerless_validation(
        &cfg,
        &config,
        &mut journal_map,
        &actor,
        &roots,
    ) {
        let _ = write_note(&cfg, &release, &plan, &format!("refused: {error}"));
        return Err(error);
    }
    let recorded = crate::ensure_states::utc_now();
    write_note(&cfg, &release, &plan, &format!("removed at {recorded}"))?;
    let mut payload = plan;
    payload["action"] = json!("released");
    payload["release_id"] = json!(release_id);
    payload["note"] = release["note"].clone();
    payload["salvage"] = release["salvage"].clone();
    Ok(payload)
}

/// The Python human rendering of one release payload.
pub(crate) fn release_text(payload: &Value) -> String {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str| payload.get(key).and_then(Value::as_u64).unwrap_or_default();
    if text("action") == "dry-run" {
        let mut out = format!(
            "DRY RUN: would release {} ({} bytes, {} files)\n  record={} target={}\n",
            text("checkout"),
            number("bytes"),
            number("files"),
            text("record"),
            text("target"),
        );
        if let Some(repositories) = payload.get("repositories").and_then(Value::as_array) {
            for row in repositories {
                let held = row.get("on_remote").and_then(Value::as_bool) == Some(true);
                out.push_str(&format!(
                    "  {} {}: {}\n",
                    row.get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    row.get("head").and_then(Value::as_str).unwrap_or_default(),
                    if held { "on remote" } else { "would bundle" },
                ));
            }
        }
        out.push_str("nothing changed; rerun with --yes to salvage and remove\n");
        out
    } else {
        let mut out = format!(
            "released {} ({} bytes); note {}\n",
            text("checkout"),
            number("bytes"),
            text("note")
        );
        if let Some(salvage) = payload.get("salvage").and_then(Value::as_array) {
            for item in salvage {
                out.push_str(&format!(
                    "  salvaged {} {} -> {}\n",
                    item.get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    item.get("commit")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    item.get("bundle")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ));
            }
        }
        out
    }
}
