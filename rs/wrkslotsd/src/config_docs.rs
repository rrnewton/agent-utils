//! Configuration key documentation for `config convert`, generated
//! from the Python tables (CONFIG_KEY_DOCS, sandbox, slotimage,
//! worktreesimage) so the literate YAML emitter matches byte for byte.

pub(crate) const TOP_DOCS: &[(&str, &str)] = &[
    ("schema", "Configuration schema version (2). Written by init; do not edit."),
    ("min_wrkslots_version", "Oldest wrkslots version (MAJOR.MINOR.PATCH) that may read this file. Raise it only when a key this file relies on for correctness is newer than some reader; an older reader then refuses, naming the version. Absent means any version that knows every key."),
    ("ignorable_keys", "Optional keys a wrkslots that does not know them may ignore, with a warning, instead of refusing the file. List a new key here when it only adds behaviour; a key that is not listed (a typo, for example) still refuses."),
    ("machine", "Machine shard name: state files are ACTIVE.<machine>.json and EVENTS.<machine>. Init writes the short host name. Absent, or the literal auto, resolves to the short host name at load, so a tracked file can be shared by every host; WRKSLOTS_MACHINE or --machine still overrides any of these."),
    ("worktrees_dir", "Where slot directories live, relative to the project root. It says where live state is: never change it while slots exist."),
    ("layout", "nested (default when absent): <worktrees_dir>/<slot>/<repo>/. flat: each slot directory is its single checkout."),
    ("default_remote", "Remote whose landed ref decides whether published work is safe; `create --remote` overrides it per checkout."),
    ("default_landed_ref", "Remote-tracking ref that counts as landed; must be under refs/remotes/<default_remote>/."),
    ("heartbeat_ttl_seconds", "Seconds a slot owner's heartbeat stays valid. Removal waits until the owner is proven dead and this time-to-live has expired."),
    ("liveness_command", "Project-relative program run with the agent name. Exit 0: dead, 1: alive, 2: cannot tell."),
    ("liveness_batch_command", "Project-relative batch liveness program (one wrkslots-liveness-batch-request/v1 JSON request for many agents), used by audit on large registries."),
    ("max_active_slots", "Cap on active slots. Absent means uncapped."),
    ("eden_host_cap", "Host-wide Eden checkout cap, counted across every daemon config dir."),
    ("cache_globs", "Slot-relative globs of regenerable caches that `clean-caches` may delete."),
    ("repo_cache_globs", "Per-repository cache globs: repository name to a list of globs."),
    (
        "post_provision_hooks",
        "Shell commands run, in order, in every new checkout; an entry {command, slot_types} runs only in slots of those types.",
    ),
    ("identity", "Git author identity ({name, email}) that create sets privately in each new slot: `git config --worktree` in the slot's own checkout, plain `git config` in each populated submodule. Absent means create leaves the identity alone."),
    ("share_submodules", "Share submodule object stores across slots (#1206): when true, create makes each slot's submodules linked worktrees of the primary .git/modules repositories instead of full private clones, preparing those repositories (config only, logged). Absent or false means the historical private clone per slot. List the key in ignorable_keys so an older wrkslots ignores it instead of refusing."),
    ("retire_freeze", "When set, every retirement and removal on this host refuses with code retire-freeze and this reason. Managed by `wrkslots freeze`; absent means unfrozen."),
    ("proc_scan_bound_seconds", "CPU-time bound in seconds for one in-process /proc process-use census during removal. Absent means a host-scaled default (30 s plus 10 s per 1,000 live processes); the WRKSLOTS_PROC_SCAN_BOUND_SECONDS environment variable overrides both. An exceeded bound still refuses as indeterminate."),
    ("salvage_push_remotes", "Remote URLs that removal may push salvage commits to. Absent or empty allows none: uncommitted work on any other remote is kept in a verified local bundle instead."),
    ("reclaimable_ignored_paths", "Checkout-relative paths or globs of regenerable build output. A retained frozen validation checkout whose only residue is ignored regular files inside these subtrees still counts as pristine; any other ignored file, a symlink, or nested Git data keeps it preserved. Absent or empty: any ignored file keeps it preserved."),
    ("disk_advisory_bytes", "Free-space level (bytes) below which create warns. The three disk_* keys are set together and must satisfy emergency < provisioning floor < advisory."),
    ("disk_provisioning_floor_bytes", "Free-space level (bytes) below which create refuses unless --override-disk-floor is given."),
    ("disk_emergency_bytes", "Free-space level (bytes) below which create always refuses."),
    ("slot_representation", "How NEW slots are stored: image (one sparse ext4 disk image per slot, mounted at the slot directory) or worktree (plain directories; the meaning when absent). Existing slots keep their representation: change this default with `wrkslots image set-default`, and migrate one idle slot in place with `wrkslots image convert SLOT --to image|worktree`."),
    ("image", "Settings for new disk-image slots; absent keys take their defaults."),
    ("sandbox", "Defaults for the slot box (`wrkslots run`, `wrkslots shell-command`, and agent launchers). The box is the same for plain and image slots. `wrkslots sandbox show-config` prints the effective values; `wrkslots sandbox write-defaults` adds missing keys."),
    ("worktrees_image", "One sparse btrfs image holding every slot (see its mount_at key for where it mounts). Absent means slots live directly on the host file system (the default). Manage it with `wrkslots worktrees-image`; kernel mount only (sudo -n), because btrfs has no unprivileged FUSE fallback."),
];

pub(crate) const SANDBOX_DOCS: &[(&str, &str)] = &[
    ("isolation", "How `wrkslots run`, `shell-command`, and agent launchers box a command. userns (default): limits plus the file-system view below, built in an unprivileged user namespace. root: the same view built by a short-lived `sudo -n` launcher, then privileges dropped to your uid, gid, and groups; use it for harness launchers that perform a setuid step, which a user namespace refuses. Setuid programs (sudo included) work inside, and your processes outside the box stay reachable through /proc. Needs passwordless sudo. cgroup: per-slot limits only, no file-system view. The box is an accident boundary for cooperative agents, not containment of a hostile process: the user's systemd bus stays reachable, so `systemd-run --user` without --scope starts work outside it."),
    ("home", "ro (default): $HOME inside the box is this slot's persistent private layer, in which every real top-level entry is bound read-only (nothing is copied). New top-level files land in the layer; the real $HOME is never written. hidden: only the home_expose paths are bound into the layer."),
    ("home_shared", "$HOME-relative paths bound read-write from the real $HOME: agent credentials, settings, and transcripts that must stay shared with the host, and package caches (.cargo/registry and .cargo/git, which cargo writes when it fetches a crate it has not cached; ~/.cargo/bin and ~/.cargo/config.toml stay read-only on purpose). A project that pins its toolchain in rust-toolchain.toml may also need .rustup here so rustup can install it. Missing paths are skipped. Only these paths are writable; the rest of ~/.config and ~/.local stays read-only. Note: these directories hold settings and hook files that the harness later runs OUTSIDE any box, so a boxed agent that edits them affects unboxed runs."),
    ("home_private", "$HOME-relative directories that are private to the slot, persistent, and writable (kept in the slot's state: the state image, or <control>/slot-state/)."),
    ("home_private_files", "Top-level $HOME files copied into the slot's layer once and then private and writable, for tools that rewrite a state file with a temporary file and a rename."),
    ("home_hidden", "$HOME-relative credentials masked inside the box: a directory appears empty, a file reads as empty."),
    ("home_expose", "With home: hidden, the $HOME-relative paths bound in, read-only."),
    ("outputs", "Directories, relative to the project root (the primary checkout holding this file), that a boxed command may write. Missing ones are skipped."),
    ("read_write", "Extra absolute paths a boxed command may write. A leading ~ and $USER or $HOME are expanded, so a per-user path can be written as /var/.../$USER/... Missing paths are skipped."),
    ("env", "Extra environment variables for the boxed command, as NAME: value. A leading ~ in a value is your $HOME."),
    ("tmp_size", "Size of the fresh /tmp tmpfs each launch gets (discarded at exit), such as 16G or 25%."),
    ("protect_system", "true (default): every mount outside the writable paths above is read-only. false leaves the rest of the file system writable; $HOME stays read-only either way."),
    ("coordinator_writable", "What a coordinator box (`wrkslots box`, `agentctl start --project-box`) may write besides the paths above. worktrees (default): the managed worktrees directory (every slot, the registry, slot images and state, validate slots) and the Git directories the project's slots commit into. project: the whole project root. A coordinator box can write the registry and every slot by design. A project that keeps validation logs or other coordinator output outside the worktrees directory chooses project, or lists those paths in read_write."),
    ("limits", "systemd limits for the slot's slice, shared by everything run against the slot. null means no limit."),
];

pub(crate) const LIMIT_DOCS: &[(&str, &str)] = &[
    ("memory_max", "Slot memory ceiling, such as 32G; swap is disabled when set."),
    ("memory_high", "Slot memory throttling threshold, such as 24G."),
    ("cpu_quota", "Slot CPU ceiling, such as 800% for eight CPUs."),
    ("tasks_max", "Slot process and thread limit (default 8192)."),
    ("io_weight", "Slot IO weight, 1 to 10000."),
    ("all_slots_memory_max", "Memory ceiling shared by all slots together."),
    ("all_slots_cpu_quota", "CPU ceiling shared by all slots together."),
    ("slot_cpu_quotas", "Ordered per-slot CPU ceilings: a list of {slot_prefix, cpu_quota}; the first prefix matching the slot name wins over cpu_quota."),
];

pub(crate) const IDENTITY_DOCS: &[(&str, &str)] = &[
    ("name", "user.name for commits made in new slots."),
    ("email", "user.email for commits made in new slots."),
];

pub(crate) const IMAGE_DOCS: &[(&str, &str)] = &[
    ("ceiling_bytes", "Apparent size of each new slot image in bytes (default 512 GiB). Images are sparse: an upper bound, not a reservation. Raise one slot later with `wrkslots image grow`."),
    ("state_ceiling_bytes", "Apparent size of each slot's state image (its private $HOME layer) in bytes (default 128 GiB)."),
    ("backend", "How slot images are mounted: kernel (sudo -n mount -o loop), fuse (fuse2fs, no privilege, slower for metadata-heavy work), or auto (default: kernel when sudo -n works, else fuse)."),
];

pub(crate) const WORKTREES_IMAGE_DOCS: &[(&str, &str)] = &[
    ("path", "Project-relative backing file for the one sparse btrfs image holding every slot. It lives outside the mount, on the host file system."),
    ("size_bytes", "Apparent size of the worktrees image in bytes. The file is sparse: an upper bound, not a reservation. Grow it later with `wrkslots worktrees-image grow`."),
    ("fs", "File system inside the worktrees image. Only btrfs is supported."),
    ("mount", "How the worktrees image is mounted: kernel (sudo -n mount -o loop) or auto (the same; btrfs has no unprivileged FUSE fallback). fuse is refused."),
    ("slot_quota_bytes", "Optional per-slot qgroup limit in bytes (referenced, post-compression). Each new slot becomes a btrfs subvolume inside the image."),
    ("machines", "Optional list of machine names (the configuration's machine identity) the worktrees image applies on. A host not listed behaves exactly as if the section were absent, so the section is safe to commit: the image exists on the named hosts only. Absent means every host."),
    ("mount_at", "Where the image mounts: binds (default; mounted beside the control files, one bind mount per slot) or worktrees_dir (mounted at the slots directory, slot subvolumes as its top-level entries, no binds). Change it only with `wrkslots worktrees-image migrate-layout`."),
    ("quota_mode", "Quota accounting for a new image: simple (default; btrfs simple quotas where the kernel and btrfs-progs support them, else full qgroups) or full. Mounting never changes an existing image's mode; status reports the active one."),
];

pub(crate) const OPTIONAL_KEYS: &[&str] = &[
    "layout",
    "liveness_batch_command",
    "max_active_slots",
    "eden_host_cap",
    "cache_globs",
    "repo_cache_globs",
    "post_provision_hooks",
    "identity",
    "share_submodules",
    "proc_scan_bound_seconds",
    "retire_freeze",
    "salvage_push_remotes",
    "reclaimable_ignored_paths",
    "disk_advisory_bytes",
    "disk_provisioning_floor_bytes",
    "disk_emergency_bytes",
    "slot_representation",
    "image",
    "sandbox",
    "worktrees_image",
    "min_wrkslots_version",
    "ignorable_keys",
];

pub(crate) const HEADER_TEMPLATE: &str = "wrkslots project configuration ({name}): where this project's agent and validation slots live and how they are stored and boxed. The format is a strict YAML subset (`wrkslots --userguide`, section \"Configuration file\"); a file whose first character is {{ is read as JSON.\nwrkslots rewrites this file with these comments regenerated (init --repair, sandbox write-defaults, image set-default); hand-written comments are not kept, and the previous bytes are saved to {name}.bak. Regenerate it with `wrkslots config convert --to yaml`.";
