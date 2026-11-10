"""The ``wrkslots image`` and ``wrkslots run`` commands.

Kept apart from ``cli.py`` so the representation and sandbox surface stays
readable; handlers import ``cli`` lazily to reuse its locks, state readers,
refusals, and process-use checks.
"""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import json
import os
import secrets
import shutil
from pathlib import Path
from typing import TYPE_CHECKING, TypeVar

from wrkslots import sandbox, slotimage, worktreesimage

if TYPE_CHECKING:
    from wrkslots import cli

GIB = 1024**3

_ParserT = TypeVar("_ParserT", bound=argparse.ArgumentParser)


def register(subparsers: argparse._SubParsersAction[_ParserT], formatter: type[argparse.HelpFormatter]) -> None:
    """Add the image, run, limits, and shell-command subcommands to the parser."""

    image = subparsers.add_parser(
        "image",
        help="inspect and manage disk-image slot storage",
        description=(
            "Manage the image representation. An image-backed slot's directory is the mount "
            "point of one sparse ext4 image under <control>/slot-images/<type>/<slot>/, with a "
            "second small image holding the sandbox's private HOME and /tmp. Representation is "
            "a storage choice only: ownership, heartbeats, salvage, and reclaim are unchanged. "
            "Every wrkslots command remounts image-backed slots that are not mounted (after a "
            "reboot, for example) before doing anything else."
        ),
        formatter_class=formatter,
    )
    actions = image.add_subparsers(dest="image_command", metavar="ACTION")

    status = actions.add_parser(
        "status",
        help="show each slot image, its mount, and how much host space it costs",
        description=(
            "List image-backed slots with apparent ceiling, host-allocated bytes, used bytes "
            "and inodes inside the image, and mount state. Also prints the representation "
            "new slots will get, and, when a worktrees_image section applies on this host, "
            "the project image's mount state and mount point."
        ),
        formatter_class=formatter,
    )
    status.add_argument(
        "--format",
        choices=("human", "json"),
        default="human",
        help="human prints one block per slot; json prints {new_slot_representation, images} (default: human)",
    )
    status.set_defaults(handler=_cmd_image_status)

    mount = actions.add_parser(
        "mount",
        help="mount every image-backed slot that is not mounted",
        description=(
            "Mount all image-backed slots at their recorded locations (idempotent). A slot "
            "that cannot be mounted does not stop the others: every slot that can be is "
            "mounted, then each failure is listed and the command exits 3. An image that a "
            "copy of an earlier mount in another mount namespace still holds is mounted "
            "through that same loop device, never a second one; an image already attached "
            "twice is refused. A mount point that holds exactly one file, the .git pointer "
            "Git writes for this slot's own worktree registration (a repair that ran while "
            "the image was unmounted leaves it), is cleared only with --coordinator-authorized, "
            "and the removal is recorded in the event log. An image that a stale-copy discard "
            "left unmounted (phase discarding-stale-copy) is never mounted automatically, "
            "because a mount would reuse a loop device that may hold the stale file system; "
            "--coordinator-authorized mounts it again from the image file, once no loop device "
            "holds it."
        ),
        epilog="examples:\n  wrkslots image mount\n  wrkslots image mount --coordinator-authorized",
        formatter_class=formatter,
    )
    mount.add_argument(
        "--coordinator-authorized",
        action="store_true",
        help=(
            "remove a stray .git file from an unmounted slot's mount point when it is exactly "
            "Git's own pointer to that slot's registered worktree (one regular file of at "
            "most 4096 bytes reading 'gitdir: <admin>' for this checkout's administrative "
            "directory, whose gitdir names this mount point), record that in the event log, "
            "then mount the slot. Anything else in a mount point is never removed. Also mount "
            "an image that a stale-copy discard left unmounted, from the image file only and "
            "only once no loop device holds it, returning it to phase ready; check its content "
            "first, since a stale copy released after the host's own mount may have written "
            "its old metadata into it (default: off, such slots are reported and left "
            "unmounted)"
        ),
    )
    mount.set_defaults(handler=_cmd_image_mount)

    unmount = actions.add_parser(
        "unmount",
        help="unmount one slot's images while nothing uses the slot",
        description=(
            "Unmount a slot's images after proving no process uses the slot. The next "
            "wrkslots command mounts them again; use this before copying an image file. "
            "Copies of the mount in other mount namespaces are released too; if one is still "
            "in use and keeps the image's loop device bound, the command says so and exits 3, "
            "because the image file is then still open. An image attached to two loop devices "
            "is refused."
        ),
        formatter_class=formatter,
    )
    _slot_arguments(unmount)
    unmount.set_defaults(handler=_cmd_image_unmount)

    trim = actions.add_parser(
        "trim",
        help="return blocks freed inside a slot image to the host now",
        description=(
            "Run fstrim on a slot's images. Kernel mounts discard freed blocks online; FUSE "
            "mounts return them only on trim or unmount."
        ),
        formatter_class=formatter,
    )
    _slot_arguments(trim)
    trim.set_defaults(handler=_cmd_image_trim)

    grow = actions.add_parser(
        "grow",
        help="raise a slot image's ceiling",
        description=(
            "Enlarge the sparse slot image and its file system. Kernel mounts grow online; "
            "FUSE mounts are briefly unmounted. Growing costs no host space until used."
        ),
        formatter_class=formatter,
    )
    _slot_arguments(grow)
    grow.add_argument("--ceiling-gib", type=int, required=True, metavar="GIB", help="new apparent size in GiB")
    grow.set_defaults(handler=_cmd_image_grow)

    set_default = actions.add_parser(
        "set-default",
        help="choose the representation for slots created from now on",
        description=(
            "Change configuration.slot_representation. Only slots created afterwards are "
            "affected; existing slots keep their representation (convert them explicitly). The "
            "configuration keeps its format (literate YAML is regenerated; JSON stays JSON) and "
            "the previous bytes are saved to .wrkslots.yml.bak."
        ),
        formatter_class=formatter,
    )
    set_default.add_argument("representation", choices=("worktree", "image"))
    set_default.set_defaults(handler=_cmd_image_set_default)

    convert = actions.add_parser(
        "convert",
        help="migrate one idle slot between plain worktrees and an image, in place",
        description=(
            "Copy a slot's content into a new image (or out of its image into plain "
            "directories), prove the copy matches file by file, swap it in at the same path, "
            "and re-verify every checkout's Git identity. Refuses while any process uses the "
            "slot. The slot path, branches, and registry record are unchanged."
        ),
        epilog=(
            "Converting to worktree discards the state image's private HOME directories "
            "(home_private). "
            "--keep-original leaves the pre-conversion copy under the slot's image directory "
            "(or beside it) for inspection instead of deleting it."
        ),
        formatter_class=formatter,
    )
    _slot_arguments(convert)
    convert.add_argument("--to", choices=("image", "worktree"), required=True, dest="target")
    convert.add_argument("--keep-original", action="store_true", help="keep the pre-conversion copy")
    convert.set_defaults(handler=_cmd_image_convert)

    image.set_defaults(handler=_cmd_image_help, image_parser=image)

    wt_image = subparsers.add_parser(
        "worktrees-image",
        help="manage the one btrfs image holding every slot (opt-in worktrees_image)",
        description=(
            "Manage the project worktrees image: one sparse btrfs image holding "
            "every slot, mounted beside the control files with a bind per slot "
            "(mount_at: binds, the default) or at the slots directory itself "
            "(mount_at: worktrees_dir). Configured by the worktrees_image "
            "section; absent means slots live on the host file system. The section's "
            "optional machines list scopes it to named hosts: elsewhere it behaves "
            "exactly as if absent, so it is safe to commit. Kernel mount "
            "only (sudo -n mount -o loop): btrfs has no unprivileged FUSE fallback. "
            "Slot-writing commands refuse unless the image is verifiably mounted, so "
            "slots are never written onto the host underneath the mount point."
        ),
        formatter_class=formatter,
    )
    wt_actions = wt_image.add_subparsers(dest="worktrees_image_command", metavar="ACTION")
    wt_create = wt_actions.add_parser(
        "create", help="create the sparse btrfs image file (does not mount it)"
    )
    wt_create.set_defaults(handler=_cmd_wt_image_create)
    wt_mount = wt_actions.add_parser("mount", help="mount the worktrees image at its configured mount point")
    wt_mount.set_defaults(handler=_cmd_wt_image_mount)
    wt_unmount = wt_actions.add_parser("unmount", help="unmount the worktrees image")
    wt_unmount.set_defaults(handler=_cmd_wt_image_unmount)
    wt_status = wt_actions.add_parser("status", help="show image file, mount state, and allocated bytes")
    wt_status.add_argument("--format", choices=("human", "json"), default="human")
    wt_status.set_defaults(handler=_cmd_wt_image_status)
    wt_grow = wt_actions.add_parser("grow", help="grow the image online (truncate, loop refresh, resize)")
    wt_grow.add_argument("--to-size-bytes", type=int, required=True, help="new apparent size in bytes")
    wt_grow.set_defaults(handler=_cmd_wt_image_grow)
    wt_trim = wt_actions.add_parser("trim", help="return freed guest blocks to the host (fstrim)")
    wt_trim.set_defaults(handler=_cmd_wt_image_trim)
    wt_migrate_layout = wt_actions.add_parser(
        "migrate-layout",
        help="switch the image between per-slot binds and mounting at the slots directory",
        description=(
            "Switch the worktrees image layout. --to direct unbinds every slot, "
            "reclaims any host original left beneath a bind (the verified image "
            "copy is kept), unmounts beside the control files and mounts the image "
            "at the slots directory itself, so slot subvolumes become its top-level "
            "entries. --to binds does the reverse. Refuses while any slot is in use "
            "(the removal census, a live or unverifiable owner, or a hold), while a "
            "validation slot exists (finish or remove it first), or while any entry "
            "in the slots directory is not a recorded slot. The worktrees_image "
            "mount_at key is rewritten only after the new mount verifiably holds "
            "every slot; on refusal the previous layout is re-established."
        ),
        epilog=(
            "examples:\n"
            "  wrkslots worktrees-image migrate-layout --to direct\n"
            "  wrkslots worktrees-image migrate-layout --to binds"
        ),
        formatter_class=formatter,
    )
    wt_migrate_layout.add_argument(
        "--to",
        choices=("direct", "binds"),
        required=True,
        dest="target",
        help="direct: mount at the slots directory, no binds; binds: mount beside "
        "the control files with one bind per slot",
    )
    wt_migrate_layout.set_defaults(handler=_cmd_wt_migrate_layout)
    wt_image.set_defaults(handler=_cmd_wt_image_help, wt_image_parser=wt_image)

    migrate = subparsers.add_parser(
        "migrate",
        help="move one slot into the project worktrees image (same path)",
        description=(
            "Copy one idle slot into a btrfs subvolume inside the mounted project "
            "worktrees image, verify the copy file by file (contents, modes, link "
            "targets, and each checkout's HEAD and status), swap it in at the "
            "same path, and reclaim the verified original. The registry record "
            "and slot path do not change. Refuses for an unregistered slot, a "
            "held slot, a slot whose owner is not proven dead, or while any "
            "process uses the slot."
        ),
        formatter_class=formatter,
    )
    _slot_arguments(migrate)
    migrate.set_defaults(handler=_cmd_migrate)

    run = subparsers.add_parser(
        "run",
        help="run a command boxed to one slot (limits + confined file-system view)",
        description=(
            "Run COMMAND boxed to one slot. The calling process joins a transient systemd user "
            "scope in the slot's own slice (limits shared by everything run against the slot) "
            "and then execs, so the PID never changes. With isolation userns or root, COMMAND "
            "also sees a confined file-system view (an accident boundary for cooperative agents, "
            "not containment of a hostile process; see the user guide's \"Limits of the box\"): "
            "a fresh /tmp per launch; $HOME as the slot's "
            "persistent private layer, in which every real top-level entry is bound read-only "
            "(nothing is copied), home_private_files are private copies seeded once, "
            "home_private directories are per-slot and writable, home_shared paths are the real "
            "ones and writable, and home_hidden credentials are masked; the slot, its Git "
            "directories, the project's outputs, and read_write paths writable; the wrkslots "
            "control directory (registry and other slots) always read-only, so registry commands "
            "such as heartbeat run outside the box; and, with protect_system, every other mount "
            "read-only. The box is the "
            "same for plain-worktree and image-backed slots. Defaults come from "
            "configuration.sandbox (see `wrkslots sandbox show-config`); list options ADD to "
            "the configured lists."
        ),
        epilog=(
            "Isolation: userns (default) builds the view in an unprivileged user namespace, where "
            "setuid programs gain nothing and host processes cannot be ptraced; root builds the "
            "identical view through a short-lived `sudo -n` launcher in a plain mount namespace "
            "and drops to your uid, gid, and groups, so setuid helpers (sudo, a harness launcher "
            "that enters a site sandbox) work inside the box, sudo can do whatever it allows, and "
            "your processes outside the box stay reachable through /proc; cgroup applies limits "
            "only. HOME modes: ro (default) keeps the real $HOME visible "
            "read-only; hidden covers it with an empty tmpfs and binds back only home_expose "
            "paths, read-only.\n\n"
            "Examples:\n"
            "  wrkslots run slot01 -- make test\n"
            "  wrkslots run slot01 --memory-max 32G --cpu-quota 800% -- claude\n"
            "  wrkslots run slot01 --isolation root -- claude\n"
            "  wrkslots run slot01 --home hidden --home-expose .gitconfig -- bash\n"
            "  wrkslots run slot01 --print -- bash\n"
            "  wrkslots run slot01 --place-only --format json\n"
            "  wrkslots run slot01 --in-scope run-1.scope --expect-slices "
            "'{\"enclosing\": null, \"all_slots\": \"wrkslots.slice\", \"slot\": "
            "\"wrkslots-slot01.slice\"}' --isolation root --closed-set --read-write "
            "/path/to/target -- make\n\n"
            "Placing your own scope: --place-only is for a caller that creates and owns its own "
            "transient scope instead of letting run create one. Run it BEFORE creating the scope, "
            "from the caller's own cgroup: it reads the caller's slice once, applies the slot "
            "limits to the all-slots and slot slices derived from that one reading, and prints "
            "the three names. Pass the slot slice to `systemd-run --scope --slice=...`. Then, from "
            "inside that scope, --in-scope UNIT --expect-slices JSON enters the slot's box in place: "
            "it checks that this process is in UNIT directly under those slices (it never reads the "
            "slice again), then runs the ordinary root-isolation steps and execs, creating no "
            "second scope and applying no limits. --closed-set makes the view's writable set "
            "exactly what the options name; with --print it prints that complete set as JSON, "
            "and --expect-writable JSON makes the real entry refuse unless its set is exactly that. "
            "--git-config-policy isolated masks the fixed default Git configuration locations "
            "($HOME/.gitconfig and $HOME/.config/git under every path to $HOME, and /etc/gitconfig) "
            "inside the view; --print reports the complete masks as JSON, and --expect-masks JSON "
            "makes the real entry refuse unless its masks are exactly that."
        ),
        formatter_class=formatter,
    )
    run.add_argument("slot", help="registered slot name")
    run.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")
    _view_arguments(run)
    run.add_argument("--cwd", metavar="PATH", help="working directory (default: the slot directory)")
    run.add_argument("--print", action="store_true", dest="print_only", help="print the scope, slice limits, environment additions, and helper command or view; run nothing")
    run.add_argument(
        "--place-only",
        action="store_true",
        help=(
            "apply the slot's slice limits and print the enclosing, all-slots and slot slice "
            "names, computed from ONE reading of this process's cgroup; join no scope and run "
            "nothing. Takes no COMMAND, --print or --cwd"
        ),
    )
    run.add_argument(
        "--in-scope",
        metavar="UNIT",
        default=None,
        help=(
            "enter the box from inside the caller's own scope UNIT (such as run-1.scope) instead "
            "of creating one: needs --expect-slices and --isolation root, joins no scope, applies "
            "no limits (run --place-only did that first), and refuses unless this process is in "
            "UNIT directly under the expected slot slice"
        ),
    )
    run.add_argument(
        "--expect-slices",
        metavar="JSON",
        default=None,
        help=(
            "with --in-scope: the object --place-only --format json printed, "
            '{"enclosing": NAME or null, "all_slots": NAME, "slot": NAME}; it must name this '
            "slot's slices"
        ),
    )
    run.add_argument(
        "--expect-writable",
        metavar="JSON",
        default=None,
        help=(
            "with --in-scope: the \"writable\" object an earlier --print of the same command "
            "reported; the entry compares its own complete writable set with it, before writing "
            "any private state and again from the view it is about to build, and refuses on any "
            "difference instead of entering the box"
        ),
    )
    run.add_argument(
        "--git-config-policy",
        choices=sandbox.GIT_CONFIG_POLICIES,
        default=None,
        help=(
            "with --in-scope: inherit (default) shows the host's Git configuration as it is; "
            "isolated masks $HOME/.gitconfig and $HOME/.config/git under $HOME and every alias of "
            "it, and /etc/gitconfig (a fixed list; no path input): an existing file reads as an "
            "empty read-only /dev/null and an existing directory as an empty read-only directory "
            "for COMMAND's whole lifetime; a missing location (no mount could keep a file created "
            "there later out), a symbolic link, or any failure refuses. Each outcome is checked "
            "inside the view before COMMAND starts. It also runs "
            "the slot's Git lookup before the view with /usr/bin/git and a fixed environment. It "
            "covers the default locations only: GIT_CONFIG_* or XDG_CONFIG_HOME set in COMMAND's "
            "own environment, and the slot's repositories' own configuration, are not changed"
        ),
    )
    run.add_argument(
        "--expect-masks",
        metavar="JSON",
        default=None,
        help=(
            "with --in-scope: the \"masks\" object an earlier --print of the same command reported "
            "(the complete ordered mask list, the Git configuration policy and its paths); the entry "
            "compares its own with it, before writing any private state and again from the view it "
            "is about to build, and refuses on any difference"
        ),
    )
    run.add_argument(
        "--closed-set",
        action="store_true",
        help=(
            "with --in-scope: --home-shared, --home-private, --output and --read-write REPLACE the "
            "configured lists instead of adding to them (absent means none), and the slot, its Git "
            "directories and other extra roots are read-only. Refuses --no-protect-system. With "
            "--print, the printed JSON lists the complete writable set"
        ),
    )
    run.add_argument(
        "--format",
        choices=("human", "json"),
        default=None,
        help=(
            "with --place-only: human prints one NAME: VALUE line per slice; json prints "
            "{enclosing, all_slots, slot}, where enclosing is null when the caller is in no "
            "slice of the user manager (default: human)"
        ),
    )
    run.add_argument(
        "--deadline-monotonic-ns",
        type=int,
        default=None,
        metavar="NS",
        help=(
            "with --place-only or --in-scope: an absolute CLOCK_MONOTONIC time in nanoseconds (the value "
            "time.monotonic_ns() would return then). Every systemd-escape, systemctl and sudo "
            "probe gets only the time left, is killed at the deadline, and the command is "
            "refused; one already past is refused before it starts (default: no deadline)"
        ),
    )
    run.add_argument("command", nargs="*", help="command to run, after a literal -- (default: $SHELL)")
    run.set_defaults(handler=_cmd_run)

    coordinator = subparsers.add_parser(
        "box",
        help="run a (sub)coordinator boxed to the project: all slots and the registry writable",
        description=(
            "Run COMMAND in a coordinator box: the same file-system view and limits as `run`, "
            "but with no slot. It is for an agent that creates slots and launches its own "
            "subagents into them. What it may write besides the usual home_shared, outputs, "
            "and read_write paths is chosen by --writable: worktrees (default) is the managed "
            "worktrees directory (every slot, the registry, slot images and state, validate "
            "slots) plus the Git directories the project's slots commit into; project is the "
            "whole project root. The agentctl registry (.agentctl in the working directory) "
            "is writable too, so the coordinator can launch agents. The registry being "
            "writable is the one deliberate exception to \"the registry is read-only inside a "
            "box\". $HOME is the box's own persistent private layer (<control>/box-state/NAME), "
            "/tmp is fresh per launch, and limits apply to the box's own slice."
        ),
        epilog=(
            "Image slots created from inside the box are mounted in the host's mount namespace "
            "through the user's systemd manager, so the host, workers launched into those slots, "
            "and the box itself all see them; mounts made on the host after the box started "
            "appear inside it under the worktrees directory. Subagents launched through a "
            "terminal multiplexer start outside this box (their pane shells belong to the "
            "multiplexer's server) and box themselves into their own slot.\n\n"
            "Examples:\n"
            "  wrkslots box -- claude\n"
            "  wrkslots box --isolation root --cwd worktrees -- claude\n"
            "  wrkslots box --writable project --name planner -- codex\n"
            "  wrkslots box --print -- bash"
        ),
        formatter_class=formatter,
    )
    coordinator.add_argument(
        "--name",
        default="coordinator",
        metavar="NAME",
        help="box name: its slice, scope, and persistent private $HOME layer (default: coordinator)",
    )
    coordinator.add_argument(
        "--writable",
        choices=sandbox.COORDINATOR_SCOPES,
        default=None,
        help=(
            "worktrees: the managed worktrees directory and the slots' Git directories; project: "
            "the whole project root (default: configuration.sandbox.coordinator_writable, else "
            "worktrees)"
        ),
    )
    _view_arguments(coordinator)
    coordinator.add_argument("--cwd", metavar="PATH", help="working directory (default: the project root)")
    coordinator.add_argument("--print", action="store_true", dest="print_only", help="print the scope, slice limits, environment additions, and helper command or view; run nothing")
    coordinator.add_argument("command", nargs="*", help="command to run, after a literal -- (default: $SHELL)")
    coordinator.set_defaults(handler=_cmd_box)

    box = subparsers.add_parser(
        "sandbox",
        help="show or complete the configuration.sandbox section",
        description=(
            "configuration.sandbox holds the defaults for `wrkslots run`, `wrkslots "
            "shell-command`, and agent launchers that box agents to slots. `init` writes the "
            "full section for a new project; unknown keys are refused."
        ),
        epilog=(
            "Keys: isolation (userns|cgroup|root), home (ro|hidden), home_shared, home_private, "
            "home_hidden, home_expose ($HOME-relative lists), home_private_files (top-level "
            "names), outputs (project-root-relative list), read_write (absolute or ~/ paths; "
            "$USER and $HOME expanded), env (NAME: value; leading ~ is $HOME), "
            "tmp_size, protect_system, coordinator_writable (worktrees|project: what `wrkslots box` "
            "may write), limits {memory_max, memory_high, cpu_quota, tasks_max, "
            "io_weight, all_slots_memory_max, all_slots_cpu_quota}.\n\n"
            "Examples:\n"
            "  wrkslots sandbox show-config\n"
            "  wrkslots sandbox write-defaults"
        ),
        formatter_class=formatter,
    )
    box_actions = box.add_subparsers(dest="sandbox_command", metavar="ACTION")
    show = box_actions.add_parser(
        "show-config",
        help="print the effective sandbox configuration as JSON",
        description=(
            "Print configuration.sandbox with every default filled in, exactly as `wrkslots "
            "run` will use it (before per-run options)."
        ),
        formatter_class=formatter,
    )
    show.set_defaults(handler=_cmd_sandbox_show)
    write_defaults = box_actions.add_parser(
        "write-defaults",
        help="add every missing sandbox default to the project configuration",
        description=(
            "Write each missing configuration.sandbox key (and missing limits key) with its "
            "default value. Present keys, and every other configuration key, are left as they "
            "are; the result is validated before it is written. A YAML file is rewritten in the "
            "canonical literate form (hand-written comments are regenerated) and a JSON file "
            "stays JSON; the previous bytes are saved to .wrkslots.yml.bak."
        ),
        formatter_class=formatter,
    )
    write_defaults.set_defaults(handler=_cmd_sandbox_write_defaults)
    box.set_defaults(handler=_cmd_sandbox_help, sandbox_parser=box)

    limits = subparsers.add_parser(
        "limits",
        help="show or apply the machine-wide guard on everything this user runs",
        description=(
            "Apply runtime (reboot-cleared) memory and CPU ceilings to this user's whole "
            "user-UID.slice with `sudo -n systemctl set-property --runtime`, so that every "
            "process the user runs, including agents that a harness moves into its own "
            "slice, shares one outer bound. Per-slot limits (wrkslots run) sit inside it. "
            "Nothing is written to host configuration."
        ),
        epilog="Example: wrkslots limits apply --memory-fraction 0.8 --cpu-fraction 0.9",
        formatter_class=formatter,
    )
    limits.add_argument("action", choices=("show", "apply", "clear"), help="show current values, apply new ones, or remove them")
    limits.add_argument("--memory-fraction", type=float, default=0.8, metavar="F", help="fraction of physical RAM for MemoryMax (default: 0.8)")
    limits.add_argument("--cpu-fraction", type=float, default=0.9, metavar="F", help="fraction of all CPUs for CPUQuota (default: 0.9)")
    limits.set_defaults(handler=_cmd_limits)

    shell_command = subparsers.add_parser(
        "shell-command",
        help="print an exec-only shell command line that enters a slot's box",
        description=(
            "Print one shell command line that replaces the current interactive shell with "
            "`wrkslots run SLOT -- SHELL -i`, using absolute interpreter paths and exec at "
            "every step so the process ID never changes. A terminal multiplexer that only "
            "starts agents in a pane whose own shell is at its prompt (Herdr) keeps working. "
            "An agent launcher runs it in a fresh pane before starting a harness. With a COMMAND "
            "after --, the line runs that program boxed instead of the shell (used for root "
            "isolation, where sudo stays in front of the program and the multiplexer cannot start "
            "an agent in the pane itself). The box is the same for plain-worktree and image-backed "
            "slots. With --box and no SLOT, the line enters a coordinator box (`wrkslots box`) "
            "instead, starting in --cwd."
        ),
        epilog=(
            "Examples:\n"
            "  wrkslots shell-command slot01 --format json\n"
            "  wrkslots shell-command slot01 --isolation root --format json -- /usr/local/bin/HARNESS ARGS\n"
            "  wrkslots shell-command --box --cwd worktrees --format json\n"
            "  wrkslots shell-command --box --isolation root --format json -- /usr/local/bin/HARNESS ARGS"
        ),
        formatter_class=formatter,
    )
    shell_command.add_argument("slot", nargs="?", help="registered slot name (omitted with --box)")
    shell_command.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")
    shell_command.add_argument(
        "--box",
        action="store_true",
        help="enter a coordinator box (`wrkslots box`) instead of a slot's box; no SLOT",
    )
    shell_command.add_argument(
        "--name",
        default="coordinator",
        metavar="NAME",
        help="with --box: the box name (default: coordinator)",
    )
    shell_command.add_argument(
        "--writable",
        choices=sandbox.COORDINATOR_SCOPES,
        default=None,
        help="with --box: worktrees or project (default: configuration.sandbox.coordinator_writable)",
    )
    shell_command.add_argument(
        "--cwd",
        metavar="PATH",
        help="with --box: the box's working directory (default: the project root)",
    )
    shell_command.add_argument(
        "--isolation",
        choices=sandbox.ISOLATIONS,
        default=None,
        help="passed to run (default: configuration.sandbox.isolation, else userns)",
    )
    shell_command.add_argument("--shell", default=None, metavar="PATH", help="interactive shell (default: $SHELL, else /bin/bash)")
    shell_command.add_argument(
        "--format",
        choices=("text", "json"),
        default="text",
        help="text prints the command line; json prints {command, slot_path, isolation}, plus box with --box, where slot_path is the box's working directory (default: text)",
    )
    shell_command.add_argument(
        "command",
        nargs="*",
        help="program to run boxed instead of the interactive shell, after a literal -- (default: SHELL -i)",
    )
    shell_command.set_defaults(handler=_cmd_shell_command)
    for name in ("image", "worktrees-image", "migrate", "run", "box", "sandbox", "limits", "shell-command"):
        _fill_help(subparsers.choices[name])


def _view_arguments(parser: argparse.ArgumentParser) -> None:
    """The view and limit options shared by `run` and `box` (and passed through by shell-command)."""

    parser.add_argument(
        "--isolation",
        choices=sandbox.ISOLATIONS,
        default=None,
        help=(
            "userns: limits plus the file-system view in an unprivileged user namespace; root: "
            "the same view built through sudo -n, then privileges dropped (setuid helpers, sudo "
            "included, work inside); cgroup: limits only, no file-system "
            "view (default: configuration.sandbox.isolation, else userns). Every mode is an "
            "accident boundary, not containment of a hostile process"
        ),
    )
    parser.add_argument(
        "--home",
        choices=sandbox.HOME_MODES,
        help="ro: every real $HOME entry visible read-only in the slot's layer; hidden: only home_expose paths (default: configuration.sandbox.home, else ro)",
    )
    parser.add_argument(
        "--home-shared",
        action="append",
        metavar="REL",
        help="$HOME-relative path bound read-write onto itself, shared with the host (repeatable; added to configuration.sandbox.home_shared)",
    )
    parser.add_argument(
        "--home-private",
        action="append",
        metavar="REL",
        help="$HOME-relative directory replaced by a persistent per-slot writable directory in the slot's layer (repeatable; added to configuration.sandbox.home_private)",
    )
    parser.add_argument(
        "--home-private-file",
        action="append",
        metavar="NAME",
        help="top-level $HOME file seeded once into the slot's layer as a private writable copy (repeatable; added to configuration.sandbox.home_private_files)",
    )
    parser.add_argument(
        "--home-hidden",
        action="append",
        metavar="REL",
        help="$HOME-relative path masked in the box: empty read-only directory or /dev/null (repeatable; added to configuration.sandbox.home_hidden)",
    )
    parser.add_argument(
        "--home-expose",
        action="append",
        metavar="REL",
        help="with --home hidden: $HOME-relative path bound back read-only (repeatable; added to configuration.sandbox.home_expose)",
    )
    parser.add_argument(
        "--output",
        action="append",
        metavar="REL",
        help="project-root-relative directory in the primary checkout left writable, skipped if missing (repeatable; added to configuration.sandbox.outputs)",
    )
    parser.add_argument(
        "--read-write",
        action="append",
        metavar="PATH",
        help="extra absolute or ~/ path left writable, skipped if missing; $USER and $HOME are expanded (repeatable; added to configuration.sandbox.read_write)",
    )
    parser.add_argument(
        "--env",
        action="append",
        metavar="NAME=VALUE",
        help="extra environment variable for COMMAND; a leading ~ in VALUE is your $HOME (repeatable; overrides configuration.sandbox.env)",
    )
    parser.add_argument("--tmp-size", metavar="SIZE", help="size limit of the per-launch /tmp tmpfs, such as 16G or 25%% (default: configuration.sandbox.tmp_size, else 16G)")
    parser.add_argument("--memory-max", metavar="SIZE", help="slot memory limit, such as 32G (swap is disabled when set)")
    parser.add_argument("--memory-high", metavar="SIZE", help="slot memory throttling threshold, such as 24G")
    parser.add_argument("--cpu-quota", metavar="PERCENT", help="slot CPU limit, such as 800%% for eight CPUs")
    parser.add_argument("--tasks-max", type=int, metavar="N", help=f"slot process and thread limit (default: configuration, else {sandbox.DEFAULT_TASKS_MAX})")
    parser.add_argument("--no-protect-system", action="store_true", help="leave mounts outside the slot writable ($HOME stays read-only; limits and masks still apply)")


def _fill(text: str | None) -> str | None:
    """Wrap prose paragraphs for the raw-description help formatter; keep indented lines."""

    import textwrap

    if not text:
        return text
    blocks = []
    for block in text.split("\n\n"):
        lines = block.split("\n")
        prose = " ".join(line for line in lines if not line.startswith("  "))
        indented = [line for line in lines if line.startswith("  ")]
        parts = [textwrap.fill(prose, width=96)] if prose else []
        blocks.append("\n".join([*parts, *indented]))
    return "\n\n".join(blocks)


def _fill_help(parser: argparse.ArgumentParser) -> None:
    parser.description = _fill(parser.description)
    parser.epilog = _fill(parser.epilog)
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            for child in action.choices.values():
                _fill_help(child)


def _slot_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("slot", help="registered slot name")
    parser.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")


def _cmd_wt_image_help(args: argparse.Namespace) -> int:
    parser = args.wt_image_parser
    assert isinstance(parser, argparse.ArgumentParser)
    parser.print_help()
    return 0


def _wt_settings(
    args: argparse.Namespace,
) -> "tuple[cli.Config, worktreesimage.WorktreesImageSettings]":
    config = _config(args)
    if config.worktrees_image is None:
        if config.worktrees_image_skipped is not None:
            raise _refuse(
                f"worktrees_image does not apply on this host: "
                f"{config.worktrees_image_skipped}"
            )
        raise _refuse(
            "no worktrees_image configured; add a worktrees_image section "
            "(path, size_bytes, fs: btrfs, mount: kernel) to enable it"
        )
    return config, config.worktrees_image


def _cmd_wt_image_create(args: argparse.Namespace) -> int:
    config, settings = _wt_settings(args)
    try:
        worktreesimage.create_image(settings.image_file(config.root), settings.size_bytes)
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    print(f"created {settings.image_file(config.root)} ({settings.size_bytes} bytes, sparse btrfs)")
    return 0


def _cmd_wt_image_mount(args: argparse.Namespace) -> int:
    config, settings = _wt_settings(args)
    mount_point = settings.mount_point(config.control, config.worktrees)
    try:
        worktreesimage.ensure_image_mounted(
            config.root, config.control, settings, config.worktrees
        )
        worktreesimage.ensure_binds(
            config.control,
            worktreesimage.image_mount_dir(config.control),
            config.worktrees,
            settings,
        )
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    suffix = "" if settings.direct else " (binds re-established)"
    print(f"mounted {settings.image_file(config.root)} at {mount_point}{suffix}")
    return 0


def _cmd_wt_image_unmount(args: argparse.Namespace) -> int:
    config, settings = _wt_settings(args)
    mount_point = settings.mount_point(config.control, config.worktrees)
    if settings.direct:
        # Every wrkslots command remounts on config load, and this one is
        # no exception: the direct slots directory cannot be left
        # unmounted here (and must never be unmounted from inside a
        # slot). Say so instead of silently remounting on the next load.
        raise _refuse(
            "in mount_at: worktrees_dir layout the image mounts at the slots "
            "directory and every wrkslots command remounts it; unmount it "
            "outside wrkslots only when no slot is in use, or switch layout "
            "with 'wrkslots worktrees-image migrate-layout --to binds' first"
        )
    try:
        worktreesimage.unmount_image(mount_point)
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    print(f"unmounted {mount_point}")
    return 0


def _cmd_wt_image_status(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config, settings = _wt_settings(args)
    report = worktreesimage.status_report(config.root, config.worktrees, settings)
    lock_missing: list[str] = []
    try:
        lock_missing = cli._image_slots_missing_lock(config, [cli._load_active(config)])
        report["lock_missing"] = lock_missing
    except (cli.Refusal, cli.StateError, OSError) as exc:
        report["lock_missing"] = lock_missing
        report["lock_check_error"] = str(exc)
    if args.format == "json":
        print(json.dumps(report, indent=2))
        return 0
    print(f"image: {report['image_file']} (exists={report['image_exists']})")
    print(
        f"mount point: {report['mount_point']} state={report['state']} "
        f"mount_at={report['mount_at']} quota={report['quota_mode']}"
    )
    if report["image_exists"]:
        print(f"apparent={report['apparent_bytes']} allocated={report['allocated_bytes']}")
    for entry in lock_missing:
        print(f"lock missing: {entry}")
    if "lock_check_error" in report:
        print(f"lock check unavailable: {report['lock_check_error']}")
    if report["state"] != worktreesimage.MOUNTED:
        print("remedy: wrkslots worktrees-image mount")
    return 0


def _cmd_wt_image_grow(args: argparse.Namespace) -> int:
    config, settings = _wt_settings(args)
    try:
        worktreesimage.grow_image(
            settings.image_file(config.root),
            settings.mount_point(config.control, config.worktrees),
            args.to_size_bytes,
        )
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    print(f"grew {settings.image_file(config.root)} to {args.to_size_bytes} bytes")
    return 0


def _cmd_wt_image_trim(args: argparse.Namespace) -> int:
    config, settings = _wt_settings(args)
    try:
        output = worktreesimage.trim_image(
            settings.mount_point(config.control, config.worktrees)
        )
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    print(output or "trimmed")
    return 0


def _lock_slot_checkouts(config: "cli.Config", slot: str, slot_type: str) -> None:
    """Take wrkslots' image Git lock on every checkout of one slot row.

    No row (an unregistered directory being migrated by path) means no
    registration exists to lock. An unreadable registry is left to the
    doctor lock-missing finding; a foreign lock refuses.
    """

    from wrkslots import cli

    try:
        state = cli._load_active(config)
    except (cli.Refusal, cli.StateError, OSError):
        return
    record = next(
        (item for item in state.slots if item.slot == slot and item.slot_type == slot_type),
        None,
    )
    if record is None:
        return
    vcs = cli._GitVcs()
    for checkout in record.checkouts:
        _relative, repository = cli._stored_repository_path(config, checkout.repository)
        vcs.lock_worktree(
            repository, cli._stored_path_reference(config, checkout.path, "checkout path")
        )


def _assert_migratable(
    config: "cli.Config", slot: str, slot_type: str, slot_path: Path
) -> None:
    """The migrate-layout census bar for one slot (#1150).

    The slot must be registered (no row means no hold or liveness
    proof exists), have no hold, have an owner the registered
    liveness authority proves dead, and have no process the
    process-use census finds in it. Callers hold the mutation locks,
    so the proofs cannot go stale mid-copy.
    """

    from wrkslots import cli

    try:
        state = cli._load_active(config)
    except (cli.Refusal, cli.StateError, OSError) as exc:
        raise _refuse(
            f"cannot read the slot registry to prove slot {slot} is safe to "
            f"migrate: {exc}"
        ) from exc
    record = next(
        (item for item in state.slots if item.slot == slot and item.slot_type == slot_type),
        None,
    )
    if record is None:
        raise _refuse(
            f"slot {slot} is not registered on machine {config.machine}; migrate "
            "needs its registry row to prove it is unheld and its owner is dead - "
            "create or register the slot first ('wrkslots status' lists the rows "
            "this host knows)"
        )
    hold = cli._load_record_holds(config, [state]).get((record.machine, record.slot))
    if hold is None or hold.error is not None or hold.value is not None:
        detail = hold.error if hold is not None and hold.error else "slot is held"
        raise _refuse(f"slot {slot}: {detail}; unhold it before migrate")
    owner_state, owner_detail = cli._owner_liveness_states(config, [record])[
        (record.machine, record.slot, record.generation)
    ]
    if owner_state != "dead":
        raise _refuse(
            f"slot {slot}: owner is not proven dead ({owner_state}: "
            f"{owner_detail}); stop its agent before migrate"
        )
    try:
        cli._assert_slot_unused(
            slot_path,
            cli._record_for_slot_use_check(
                record, validate_complete=False, allow_live_validate_owner=False
            ),
        )
    except cli.Refusal as exc:
        raise _refuse(f"slot {slot} is in use; migrate only a stopped slot: {exc}") from exc


def _cmd_migrate(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config, settings = _wt_settings(args)
    if config.worktrees_image_state != worktreesimage.MOUNTED:
        raise _refuse(
            f"worktrees image is {config.worktrees_image_state}; run "
            "'wrkslots worktrees-image mount' first"
        )
    # The census proofs and the copy run under the mutation locks
    # (#1150), so a hold placed or an owner revived mid-copy cannot
    # slip past them.
    with cli._mutation_locks(config, args.wait_lock):
        return _cmd_migrate_locked(config, settings, args)


def _cmd_migrate_locked(
    config: "cli.Config",
    settings: worktreesimage.WorktreesImageSettings,
    args: argparse.Namespace,
) -> int:
    from wrkslots import cli

    slot_path = cli._slot_directory(config, args.slot, args.slot_type)
    if not slot_path.is_dir():
        raise _refuse(f"slot directory is missing: {slot_path}")
    if settings.direct:
        return _cmd_migrate_direct(config, settings, args, slot_path)
    if args.slot in worktreesimage.load_binds(config.control):
        _lock_slot_checkouts(config, args.slot, args.slot_type)
        print(f"{args.slot} is already bound from the worktrees image")
        return 0
    _assert_migratable(config, args.slot, args.slot_type, slot_path)
    before = worktreesimage.listing_signature(slot_path)
    before_git = worktreesimage.git_state_signature(slot_path)
    mount_dir = worktreesimage.image_mount_dir(config.control)
    subvolume = mount_dir / args.slot
    if subvolume.exists():
        raise _refuse(f"image subvolume already exists: {subvolume}; inspect it first")
    displaced = slot_path.parent / f".{args.slot}.premigrate-{secrets.token_hex(8)}"
    try:
        worktreesimage.create_slot_subvolume(subvolume, settings.slot_quota_bytes)
        try:
            shutil.copytree(
                slot_path,
                subvolume,
                dirs_exist_ok=True,
                symlinks=True,
                ignore=worktreesimage._skip_special,
            )
        except BaseException:
            # A failed copy must not leave a partial subvolume that the
            # next migrate attempt would refuse on; the original is
            # untouched either way.
            worktreesimage.delete_slot_subvolume(subvolume)
            raise
        if (
            worktreesimage.listing_signature(subvolume) != before
            or worktreesimage.git_state_signature(subvolume) != before_git
        ):
            worktreesimage.delete_slot_subvolume(subvolume)
            raise _refuse("migrated copy does not match the slot file by file; original kept")
        # Reclaim (#1149): move the verified original aside, bind the
        # subvolume at the original real path (path, registry record
        # and agent cwd never change), re-prove the displaced tree
        # unused, then delete it — nothing stays beneath the bind.
        # A crash between the rename and the bind leaves the original
        # at the named sibling; rename it back and rerun migrate.
        os.rename(slot_path, displaced)
        try:
            worktreesimage.bind_slot(config.control, mount_dir, args.slot, slot_path)
        except BaseException:
            if slot_path.is_dir() and next(slot_path.iterdir(), None) is None:
                slot_path.rmdir()
            os.rename(displaced, slot_path)
            worktreesimage.delete_slot_subvolume(subvolume)
            raise
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    try:
        _assert_migratable(config, args.slot, args.slot_type, displaced)
    except Exception as exc:
        raise _refuse(
            f"migrated {args.slot} into the worktrees image, bound at {slot_path}, but "
            f"the verified original at {displaced} could not be proven unused for "
            f"reclaim: {exc}; delete it once inspected"
        ) from exc
    try:
        shutil.rmtree(displaced)
    except OSError as exc:
        raise _refuse(
            f"migrated {args.slot} into the worktrees image, bound at {slot_path}, but "
            f"the verified original remains at {displaced}: {exc}; delete it once inspected"
        ) from exc
    _lock_slot_checkouts(config, args.slot, args.slot_type)
    print(f"migrated {args.slot} into the worktrees image, bound at {slot_path} (original reclaimed)")
    return 0


def _cmd_migrate_direct(
    config: "cli.Config",
    settings: worktreesimage.WorktreesImageSettings,
    args: argparse.Namespace,
    slot_path: Path,
) -> int:
    """Direct-layout `migrate`: turn a plain in-image slot dir into a subvolume.

    The slot already lives inside the mounted image; a staging subvolume
    is filled, compared file by file, and swapped in by two renames, and
    the plain original is then deleted (nothing is kept beneath anything
    in this layout).
    """

    if worktreesimage.is_subvolume(slot_path):
        _lock_slot_checkouts(config, args.slot, args.slot_type)
        print(f"{args.slot} is already a subvolume in the worktrees image")
        return 0
    _assert_migratable(config, args.slot, args.slot_type, slot_path)
    before = worktreesimage.listing_signature(slot_path)
    before_git = worktreesimage.git_state_signature(slot_path)
    staging = slot_path.parent / f".{args.slot}.migrating-{secrets.token_hex(8)}"
    displaced = slot_path.parent / f".{args.slot}.premigrate-{secrets.token_hex(8)}"
    try:
        worktreesimage.create_slot_subvolume(staging, settings.slot_quota_bytes)
        try:
            shutil.copytree(
                slot_path,
                staging,
                dirs_exist_ok=True,
                symlinks=True,
                ignore=worktreesimage._skip_special,
            )
        except BaseException:
            worktreesimage.delete_slot_subvolume(staging)
            raise
        if (
            worktreesimage.listing_signature(staging) != before
            or worktreesimage.git_state_signature(staging) != before_git
        ):
            worktreesimage.delete_slot_subvolume(staging)
            raise _refuse("migrated copy does not match the slot file by file; original kept")
    except worktreesimage.WorktreesImageError as exc:
        raise _refuse(str(exc)) from exc
    os.rename(slot_path, displaced)
    try:
        os.rename(staging, slot_path)
    except OSError:
        os.rename(displaced, slot_path)
        worktreesimage.delete_slot_subvolume(staging)
        raise
    try:
        shutil.rmtree(displaced)
    except OSError as exc:
        raise _refuse(
            f"migrated {args.slot} into the worktrees image at {slot_path}, but the "
            f"verified plain original remains at {displaced}: {exc}; delete it once "
            "inspected"
        ) from exc
    _lock_slot_checkouts(config, args.slot, args.slot_type)
    print(f"migrated {args.slot} into the worktrees image at {slot_path} (original reclaimed)")
    return 0


def _layout_census(
    config: "cli.Config", state: "cli.ActiveState"
) -> "list[cli.ActiveRecord]":
    """Every agent slot, proven idle for a layout switch; refuses otherwise.

    The switch unmounts every slot's storage, so the bar is the removal
    fence's: the registered liveness authority says the owner is dead,
    no hold protects the slot, and the process-use census finds nobody
    in it. Validation slots refuse outright (they are ephemeral: finish
    or remove them first), as does a per-slot-image slot.
    """

    from wrkslots import cli

    records = list(state.slots)
    validate = [record for record in records if record.slot_type == "validate"]
    if validate:
        names = ", ".join(sorted(record.slot for record in validate))
        raise _refuse(
            f"validation slots exist ({names}); finish or remove them before "
            "switching the worktrees image layout"
        )
    holds = cli._load_record_holds(config, [state])
    liveness = cli._owner_liveness_states(config, records)
    for record in records:
        hold = holds.get((record.machine, record.slot))
        if hold is None or hold.error is not None or hold.value is not None:
            detail = hold.error if hold is not None and hold.error else "slot is held"
            raise _refuse(f"slot {record.slot}: {detail}; unhold it before migrate-layout")
        owner_state, owner_detail = liveness[
            (record.machine, record.slot, record.generation)
        ]
        if owner_state != "dead":
            raise _refuse(
                f"slot {record.slot}: owner is not proven dead ({owner_state}: "
                f"{owner_detail}); stop its agent before migrate-layout"
            )
        slot_path = cli._slot_directory(config, record.slot, record.slot_type)
        if cli._slot_image_at(config, slot_path) is not None:
            raise _refuse(
                f"slot {record.slot} uses a per-slot image; convert or remove it "
                "before switching the worktrees image layout"
            )
        cli._assert_slot_unused(
            slot_path,
            cli._record_for_slot_use_check(
                record, validate_complete=False, allow_live_validate_owner=False
            ),
        )
    return records


def _write_mount_at(config: "cli.Config", mount_at: str) -> None:
    """Rewrite only ``worktrees_image.mount_at``; caller holds mutation locks."""

    from wrkslots import cli

    raw = cli._read_config(config.config_path)
    section = dict(cli._as_mapping(raw.get("worktrees_image"), "configuration.worktrees_image"))
    section["mount_at"] = mount_at
    updated = dict(raw)
    updated["worktrees_image"] = section
    cli._write_config(config.config_path, updated)


def _layout_unknown_entries(
    root: Path, allowed: set[str], *, what: str
) -> None:
    if not root.is_dir():
        return
    unknown = sorted(
        entry.name for entry in root.iterdir() if entry.name not in allowed
    )
    if unknown:
        raise _refuse(
            f"{what} holds entries that are not recorded slots ({', '.join(unknown)}); "
            "inspect and move or register them before switching layout - the switch "
            "mounts over this directory and would hide them"
        )


def _cmd_wt_migrate_layout(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config, settings = _wt_settings(args)
    want_direct = args.target == "direct"
    if settings.direct == want_direct:
        print(f"worktrees image already uses mount_at: {settings.mount_at}")
        return 0
    if config.worktrees_image_state != worktreesimage.MOUNTED:
        raise _refuse(
            f"worktrees image is {config.worktrees_image_state}; run "
            "'wrkslots worktrees-image mount' and resolve what it reports first"
        )
    with cli._mutation_locks(config, args.wait_lock):
        cli._refuse_partial_state(config)
        state = cli._load_active(config)
        agents = _layout_census(config, state)
        if want_direct:
            _migrate_layout_to_direct(config, settings, agents)
        else:
            _migrate_layout_to_binds(config, settings, agents)
    fresh = cli._load_config(args.project_root, args.machine)
    fresh_settings = fresh.worktrees_image
    if (
        fresh_settings is None
        or fresh_settings.direct != want_direct
        or fresh.worktrees_image_state != worktreesimage.MOUNTED
    ):
        raise _refuse(
            "layout switched and the configuration rewritten, but reloading it "
            f"reports state={fresh.worktrees_image_state}; inspect "
            "'wrkslots worktrees-image status' before creating slots"
        )
    mode = worktreesimage.quota_mode_at(
        fresh_settings.mount_point(fresh.control, fresh.worktrees),
        fresh_settings.image_file(fresh.root),
    )
    print(
        f"worktrees image now mounts at "
        f"{fresh_settings.mount_point(fresh.control, fresh.worktrees)} "
        f"(mount_at: {fresh_settings.mount_at}, quota={mode}, {len(agents)} slots)"
    )
    return 0


def _migrate_layout_to_direct(
    config: "cli.Config",
    settings: worktreesimage.WorktreesImageSettings,
    agents: "list[cli.ActiveRecord]",
) -> None:
    from wrkslots import cli

    mount_dir = worktreesimage.image_mount_dir(config.control)
    records = worktreesimage.load_bind_records(config.control)
    for record in agents:
        bind = records.get(record.slot)
        if bind is None:
            raise _refuse(
                f"slot {record.slot} is not in the worktrees image (no bind "
                "record); run 'wrkslots migrate' for it, or remove it, first"
            )
        if record.slot == worktreesimage.VALIDATE_SUBDIR:
            raise _refuse(
                "agent slot 'validate' cannot switch to the direct layout: that "
                "top-level entry holds the validation slots there; remove or "
                "rename (recreate) the slot first"
            )
        if not (mount_dir / bind.subvolume).is_dir():
            raise _refuse(
                f"slot {record.slot}: image subvolume is missing: "
                f"{mount_dir / bind.subvolume}"
            )
    stray = sorted(set(records) - {record.slot for record in agents})
    if stray:
        raise _refuse(
            f"binds recorded for slots with no active row ({', '.join(stray)}); "
            "inspect .worktrees-image.json before switching layout"
        )
    _layout_unknown_entries(
        config.worktrees,
        {record.slot for record in agents},
        what=f"slots directory {config.worktrees}",
    )
    image_file = settings.image_file(config.root)
    direct_settings = dataclasses.replace(
        settings, mount_at=worktreesimage.MOUNT_AT_WORKTREES_DIR
    )
    try:
        for record in agents:
            worktreesimage.unbind_slot(
                config.control,
                record.slot,
                cli._slot_directory(config, record.slot, record.slot_type),
            )
        for record in agents:
            host_path = cli._slot_directory(config, record.slot, record.slot_type)
            if host_path.is_dir() and not host_path.is_symlink():
                # The empty mount point, plus the stale host original a
                # `migrate` kept beneath the bind (#1149): the subvolume
                # in the image is the slot; the original is reclaimed.
                shutil.rmtree(host_path)
        worktreesimage.unmount_image(mount_dir)
        worktreesimage.mount_image(image_file, config.worktrees, direct_settings)
        (config.worktrees / worktreesimage.VALIDATE_SUBDIR).mkdir(exist_ok=True)
        for record in agents:
            slot_path = cli._slot_directory(config, record.slot, record.slot_type)
            if not worktreesimage.is_subvolume(slot_path):
                raise worktreesimage.WorktreesImageError(
                    f"slot {record.slot} is not a subvolume at {slot_path} after "
                    "mounting the image at the slots directory"
                )
    except (worktreesimage.WorktreesImageError, OSError) as exc:
        try:
            with contextlib.suppress(Exception):
                worktreesimage.unmount_image(config.worktrees)
            worktreesimage.save_bind_records(config.control, records)
            worktreesimage.ensure_image_mounted(
                config.root, config.control, settings, config.worktrees
            )
            worktreesimage.ensure_binds(
                config.control, mount_dir, config.worktrees, settings
            )
        except Exception as rollback_exc:
            raise _refuse(
                f"migrate-layout failed ({exc}) and re-establishing the bind "
                f"layout also failed ({rollback_exc}); the image and its "
                "subvolumes are intact: inspect the mounts, then run "
                "'wrkslots worktrees-image mount'"
            ) from exc
        raise _refuse(
            f"migrate-layout failed ({exc}); the bind layout was "
            "re-established and the configuration is unchanged"
        ) from exc
    for record in agents:
        _lock_slot_checkouts(config, record.slot, record.slot_type)
    _write_mount_at(config, worktreesimage.MOUNT_AT_WORKTREES_DIR)


def _migrate_layout_to_binds(
    config: "cli.Config",
    settings: worktreesimage.WorktreesImageSettings,
    agents: "list[cli.ActiveRecord]",
) -> None:
    from wrkslots import cli

    for record in agents:
        slot_path = cli._slot_directory(config, record.slot, record.slot_type)
        if not worktreesimage.is_subvolume(slot_path):
            raise _refuse(
                f"slot {record.slot} at {slot_path} is a plain directory inside "
                "the image, not a subvolume; run 'wrkslots migrate' for it first"
            )
    _layout_unknown_entries(
        config.worktrees,
        {record.slot for record in agents} | {worktreesimage.VALIDATE_SUBDIR},
        what=f"slots directory {config.worktrees}",
    )
    validate_dir = config.worktrees / worktreesimage.VALIDATE_SUBDIR
    if validate_dir.is_dir() and next(validate_dir.iterdir(), None) is not None:
        raise _refuse(
            f"{validate_dir} is not empty; remove its validation slots before "
            "switching to the bind layout"
        )
    image_file = settings.image_file(config.root)
    binds_settings = dataclasses.replace(
        settings, mount_at=worktreesimage.MOUNT_AT_BINDS
    )
    mount_dir = worktreesimage.image_mount_dir(config.control)
    bound: list[str] = []
    try:
        worktreesimage.unmount_image(config.worktrees)
        worktreesimage.ensure_image_mounted(
            config.root, config.control, binds_settings, config.worktrees
        )
        for record in agents:
            worktreesimage.bind_slot(
                config.control,
                mount_dir,
                record.slot,
                cli._slot_directory(config, record.slot, record.slot_type),
            )
            bound.append(record.slot)
    except (worktreesimage.WorktreesImageError, OSError) as exc:
        try:
            for slot in bound:
                with contextlib.suppress(Exception):
                    worktreesimage.unbind_slot(
                        config.control, slot, config.worktrees / slot
                    )
            with contextlib.suppress(Exception):
                worktreesimage.unmount_image(mount_dir)
            worktreesimage.mount_image(image_file, config.worktrees, settings)
        except Exception as rollback_exc:
            raise _refuse(
                f"migrate-layout failed ({exc}) and re-establishing the direct "
                f"layout also failed ({rollback_exc}); the image and its "
                "subvolumes are intact: inspect the mounts, then run "
                "'wrkslots worktrees-image mount'"
            ) from exc
        raise _refuse(
            f"migrate-layout failed ({exc}); the direct layout was "
            "re-established and the configuration is unchanged"
        ) from exc
    for record in agents:
        _lock_slot_checkouts(config, record.slot, record.slot_type)
    _write_mount_at(config, worktreesimage.MOUNT_AT_BINDS)


def _cmd_image_help(args: argparse.Namespace) -> int:
    parser = args.image_parser
    assert isinstance(parser, argparse.ArgumentParser)
    parser.print_help()
    return 0


def _config(args: argparse.Namespace) -> "cli.Config":
    from wrkslots import cli

    return cli._load_config(args.project_root, args.machine)


def _refuse(message: str) -> "cli.Refusal":
    from wrkslots import cli

    return cli.Refusal(message)


def _load_image(config: "cli.Config", slot_type: str, slot: str) -> slotimage.SlotImage:
    try:
        image = slotimage.load(config.control, slot_type, slot)
    except slotimage.ImageError as exc:
        raise _refuse(str(exc)) from exc
    if image is None:
        raise _refuse(f"{slot_type} slot {slot!r} is not image-backed")
    return image


def _cmd_image_status(args: argparse.Namespace) -> int:
    config = _config(args)
    try:
        reports = [slotimage.usage(image) for image in slotimage.all_images(config.control)]
    except slotimage.ImageError as exc:
        raise _refuse(str(exc)) from exc
    wt_report = (
        worktreesimage.status_report(config.root, config.worktrees, config.worktrees_image)
        if config.worktrees_image is not None
        else None
    )
    if args.format == "json":
        payload: dict[str, object] = {
            "new_slot_representation": config.slot_representation,
            "images": reports,
        }
        if wt_report is not None:
            payload["worktrees_image"] = wt_report
        print(json.dumps(payload, indent=2))
        return 0
    print(f"new slots: {config.slot_representation}")
    if wt_report is not None:
        print(
            f"worktrees image: {wt_report['state']} at {wt_report['mount_point']} "
            f"(mount_at={wt_report['mount_at']})"
        )
    if not reports:
        print("no image-backed slots")
    for report in reports:
        print(f"{report['slot_type']}/{report['name']} at {report['location']} ({report['phase']})")
        for label in ("slot", "state"):
            entry = report.get(label)
            if not isinstance(entry, dict):
                continue
            print(
                f"  {label:5s} mounted={entry.get('mounted')} backend={entry.get('backend', '-')} "
                f"host={_gib(entry.get('host_allocated_bytes'))} used={_gib(entry.get('used_bytes'))} "
                f"ceiling={_gib(entry.get('ceiling_bytes'))} inodes={entry.get('used_inodes', '-')}"
            )
    return 0


def _gib(value: object) -> str:
    if not isinstance(value, int):
        return "-"
    return f"{value / GIB:.2f}G"


#: Git's pointer file is one short line; anything larger is not one.
_STRAY_POINTER_LIMIT = 4096


def stray_worktree_pointer(
    config: "cli.Config", record: "cli.ActiveRecord", image: slotimage.SlotImage
) -> tuple[Path, str] | None:
    """``(file, content)`` when an unmounted slot's mount point holds only Git's own pointer.

    ``git worktree repair``, run in the main worktree while a flat image slot
    was unmounted, writes ``.git`` into the empty mount point, which then
    refuses the mount. That file is recognised only when it is exactly Git's
    pointer to this slot's own registration: the mount point holds nothing
    else; ``.git`` is a regular file the user owns, at most 4096 bytes, reading
    ``gitdir: <admin>`` and a newline; ``<admin>`` is a linked-worktree
    directory of the checkout's recorded repository; and ``<admin>/gitdir``
    names exactly this mount point's ``.git``.
    """

    from wrkslots import cli

    location = image.location
    if slotimage.is_mounted(image.slot_image, location) or len(record.checkouts) != 1:
        return None
    try:
        entries = sorted(os.listdir(location))
    except OSError:
        return None
    if entries != [".git"]:
        return None
    pointer = location / ".git"
    try:
        info = os.lstat(pointer)
        if (
            not (info.st_mode & 0o170000 == 0o100000)
            or info.st_uid != os.getuid()
            or info.st_size > _STRAY_POINTER_LIMIT
        ):
            return None
        content = pointer.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return None
    if not content.startswith("gitdir: ") or not content.endswith("\n") or content.count("\n") != 1:
        return None
    admin = Path(content.removeprefix("gitdir: ").removesuffix("\n"))
    checkout = record.checkouts[0]
    try:
        if cli._stored_path(config, checkout.path, "checkout path") != location:
            return None
        _relative, repository = cli._stored_repository_path(config, checkout.repository)
        common = cli._GitVcs().common_directory(repository)
    except (cli.Refusal, cli.StateError):
        return None
    if not admin.is_absolute() or admin.parent != common / "worktrees" or not admin.is_dir():
        return None
    try:
        registered = (admin / "gitdir").read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return None
    if registered.rstrip("\n") not in (str(pointer), os.path.realpath(pointer)):
        return None
    return pointer, content


def _clear_stray_pointers(config: "cli.Config", authorized: bool) -> list[str]:
    """Remove exact stray Git pointers (when authorized) and describe each one found."""

    from wrkslots import cli

    notes: list[str] = []
    try:
        state = cli._load_active(config)
    except (cli.Refusal, cli.StateError):
        return notes
    for record in state.slots:
        try:
            image = slotimage.load(config.control, record.slot_type, record.slot)
        except slotimage.ImageError:
            continue
        if image is None or image.phase != "ready":
            continue
        found = stray_worktree_pointer(config, record, image)
        if found is None:
            continue
        pointer, content = found
        if not authorized:
            notes.append(
                f"{record.slot_type}/{record.slot}: its mount point holds only Git's own pointer "
                f"to its worktree registration ({pointer}: {content.strip()!r}); rerun "
                "'wrkslots image mount --coordinator-authorized' to remove it and mount the slot"
            )
            continue
        pointer.unlink()
        cli._fsync_directory(pointer.parent)
        state = cli._replace_record(state, record)
        cli._write_active_state(
            config,
            state,
            action="stray-git-pointer-removed",
            slot=record.slot,
            evidence={
                "generation": record.generation,
                "path": str(pointer),
                "content": content,
                "coordinator_authorized": True,
                "actor": cli._identity_to_obj(cli._read_process_identity(os.getpid())),
                "reason": (
                    "the image was not mounted and its mount point held only Git's own pointer "
                    "to this slot's worktree registration"
                ),
            },
            require_repository=False,
        )
        print(f"removed stray Git pointer {pointer} (recorded in the event log)")
    return notes


def _cmd_image_mount(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    with cli._mutation_locks(config, args.wait_lock):
        notes = _clear_stray_pointers(config, bool(getattr(args, "coordinator_authorized", False)))
        try:
            actions, failures = slotimage.ensure_all_mounted(
                config.control,
                remount_discarded=bool(getattr(args, "coordinator_authorized", False)),
            )
        except slotimage.ImageError as exc:
            raise _refuse(str(exc)) from exc
        cli._refresh_owned_mounts(config)
    print("\n".join(actions) if actions else "no image-backed slot needed mounting")
    if failures:
        raise _refuse(
            f"{len(failures)} image-backed slot(s) could not be mounted; every other slot was: "
            + "; ".join([*failures, *notes])
        )
    return 0


def _cmd_image_unmount(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    image = _load_image(config, args.slot_type, args.slot)
    with cli._mutation_locks(config, args.wait_lock):
        cli._assert_slot_unused(image.location)
        try:
            residual = [
                *slotimage.unmount(image.state_image, image.state_mount),
                *slotimage.unmount(image.slot_image, image.location),
            ]
        except slotimage.ImageError as exc:
            raise _refuse(str(exc)) from exc
        cli._refresh_owned_mounts(config)
    if residual:
        raise _refuse(
            f"unmounted {args.slot_type}/{args.slot} on this host, but {', '.join(residual)} "
            "still holds its image (a mount in another mount namespace that is still in use). "
            "Do not copy or change the image file until that is released; the next wrkslots "
            "command remounts the slot through the same device"
        )
    print(f"unmounted {args.slot_type}/{args.slot}; the next wrkslots command remounts it")
    return 0


def _cmd_image_trim(args: argparse.Namespace) -> int:
    config = _config(args)
    image = _load_image(config, args.slot_type, args.slot)
    for line in slotimage.trim(image):
        print(line)
    return 0


def _cmd_image_grow(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    image = _load_image(config, args.slot_type, args.slot)
    with cli._mutation_locks(config, args.wait_lock):
        try:
            grown = slotimage.grow(image, args.ceiling_gib * GIB)
        except slotimage.ImageError as exc:
            raise _refuse(str(exc)) from exc
    print(f"{args.slot_type}/{args.slot} ceiling is now {grown.ceiling_bytes // GIB} GiB")
    return 0


def _cmd_image_set_default(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    with cli._locked_config(config.config_path, args.wait_lock):
        raw = cli._read_config(config.config_path)
        updated = dict(raw)
        if args.representation == "worktree":
            updated.pop("slot_representation", None)
        else:
            updated["slot_representation"] = args.representation
        cli._write_config(config.config_path, updated)
    print(
        f"new slots will use the {args.representation} representation; "
        "existing slots are unchanged (see `wrkslots image convert`)"
    )
    return 0


def _checkout_paths(config: "cli.Config", record: "cli.ActiveRecord") -> list[tuple[Path, Path, str]]:
    from wrkslots import cli

    rows: list[tuple[Path, Path, str]] = []
    for checkout in record.checkouts:
        _relative, repository = cli._stored_repository_path(config, checkout.repository)
        path = cli._stored_path(config, checkout.path, "checkout path")
        rows.append((repository, path, checkout.head))
    return rows


def _verify_checkouts(config: "cli.Config", record: "cli.ActiveRecord") -> None:
    from wrkslots import cli

    vcs = cli._GitVcs()
    for repository, path, _head in _checkout_paths(config, record):
        vcs.verify_existing_worktree(repository, path)


def _cmd_image_convert(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    slot_path = cli._slot_directory(config, args.slot, args.slot_type)
    with cli._mutation_locks(config, args.wait_lock):
        cli._refuse_partial_state(config)
        state = cli._load_active(config)
        record = cli._find_record(state, args.slot)
        if record.slot_type != args.slot_type:
            raise _refuse(f"slot {args.slot!r} is a {record.slot_type} slot, not {args.slot_type}")
        current = cli._slot_representation_of(config, args.slot, args.slot_type)
        if current == args.target:
            print(f"{args.slot_type}/{args.slot} is already {args.target}-backed")
            return 0
        if not slot_path.is_dir() or slot_path.is_symlink():
            raise _refuse(f"slot directory is missing or unsafe: {slot_path}")
        # Path use, not owner liveness: an idle agent may stay registered and
        # alive; what must not happen is copying files out from under a process
        # that has them open or is working inside the slot.
        cli._assert_slot_unused(slot_path)
        _verify_checkouts(config, record)
        if args.target == "image":
            _convert_to_image(config, record, slot_path, keep_original=args.keep_original)
        else:
            _convert_to_worktree(config, record, slot_path, keep_original=args.keep_original)
        cli._refresh_owned_mounts(config)
        _verify_checkouts(config, record)
    print(f"converted {args.slot_type}/{args.slot} to {args.target} at {slot_path}")
    return 0


def _convert_to_image(
    config: "cli.Config", record: "cli.ActiveRecord", slot_path: Path, *, keep_original: bool
) -> None:
    from wrkslots import cli

    staging = slotimage.image_directory(config.control, record.slot_type, record.slot).with_name(
        f".{record.slot}.convert-{secrets.token_hex(4)}"
    )
    image = cli._provision_slot_image(config, record.slot, record.slot_type, staging)
    original = image.directory / "pre-image-original"
    try:
        slotimage.copy_tree(slot_path, image.location)
        cli._assert_slot_unused(slot_path)
    except (slotimage.ImageError, cli.Refusal) as exc:
        slotimage.destroy(image, allow_content=True)
        raise _refuse(f"conversion to image abandoned; the slot is unchanged: {exc}") from exc
    os.rename(slot_path, original)
    try:
        slotimage.relocate(image, slot_path)
    except slotimage.ImageError as exc:
        os.rename(original, slot_path)
        raise _refuse(f"conversion to image failed; the original was restored: {exc}") from exc
    if keep_original:
        print(f"kept the pre-conversion copy at {original}")
    else:
        shutil.rmtree(original)


def _convert_to_worktree(
    config: "cli.Config", record: "cli.ActiveRecord", slot_path: Path, *, keep_original: bool
) -> None:
    image = _load_image(config, record.slot_type, record.slot)
    staging = image.directory.with_name(f".{record.slot}.convert-{secrets.token_hex(4)}")
    staging.mkdir(mode=0o755)
    try:
        slotimage.copy_tree(slot_path, staging)
    except slotimage.ImageError as exc:
        shutil.rmtree(staging)
        raise _refuse(f"conversion to worktree abandoned; the slot is unchanged: {exc}") from exc
    if keep_original:
        kept = image.directory.with_name(f".{record.slot}.pre-worktree-image")
        slotimage.unmount(image.state_image, image.state_mount)
        slotimage.unmount(image.slot_image, image.location)
        os.rename(image.directory, kept)
        print(f"kept the pre-conversion image directory at {kept}")
    else:
        slotimage.discard_files(image)
    slotimage.remove_mount_point(slot_path, image.directory.parent)
    os.rename(staging, slot_path)


#: The Git executable and the complete environment of the isolated policy's
#: lookup before the view (:func:`_isolated_common_directory`): nothing comes
#: from PATH or the caller's environment, and no global or system configuration
#: is read.
ISOLATED_GIT_EXECUTABLE = "/usr/bin/git"
ISOLATED_GIT_ENVIRONMENT = {
    "PATH": "/usr/bin:/bin",
    "HOME": "/nonexistent",
    "LC_ALL": "C",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_SYSTEM": "/dev/null",
    "GIT_NO_REPLACE_OBJECTS": "1",
    "GIT_TERMINAL_PROMPT": "0",
    "GIT_OPTIONAL_LOCKS": "0",
}
ISOLATED_GIT_SECONDS = 30.0


def _isolated_common_directory(repository: Path) -> Path:
    """``git rev-parse --git-common-dir`` for the isolated Git configuration policy.

    Runs the fixed :data:`ISOLATED_GIT_EXECUTABLE` (a root-owned regular file no
    group or other user can write; a link is refused) with exactly
    :data:`ISOLATED_GIT_ENVIRONMENT`, bounded by :data:`ISOLATED_GIT_SECONDS`.
    The ordinary lookup keeps its own behaviour.
    """

    import stat
    import subprocess

    try:
        status = os.lstat(ISOLATED_GIT_EXECUTABLE)
    except OSError as exc:
        raise _refuse(f"git-config-policy isolated: cannot inspect {ISOLATED_GIT_EXECUTABLE}: {exc}") from exc
    if not stat.S_ISREG(status.st_mode) or status.st_uid != 0 or status.st_mode & 0o022:
        raise _refuse(
            f"git-config-policy isolated: {ISOLATED_GIT_EXECUTABLE} is not a root-owned regular file "
            "that only root can write; refusing"
        )
    command = [
        ISOLATED_GIT_EXECUTABLE, "--no-replace-objects", "-c", "core.useReplaceRefs=false",
        "-C", str(repository), "rev-parse", "--path-format=absolute", "--git-common-dir",
    ]
    try:
        completed = subprocess.run(
            command, env=dict(ISOLATED_GIT_ENVIRONMENT), text=True, capture_output=True,
            check=False, timeout=ISOLATED_GIT_SECONDS, stdin=subprocess.DEVNULL,
        )
    except subprocess.TimeoutExpired as exc:
        raise _refuse(f"git-config-policy isolated: the Git lookup in {repository} timed out") from exc
    except OSError as exc:
        raise _refuse(f"git-config-policy isolated: cannot execute Git: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise _refuse(f"Git refused in {repository}: git rev-parse --git-common-dir" + (f": {detail}" if detail else ""))
    lines = completed.stdout.splitlines()
    common = Path(lines[0]).absolute() if len(lines) == 1 else None
    if common is None or common.is_symlink() or not common.is_dir():
        raise _refuse(f"Git reported an unsafe common directory: {completed.stdout.strip()}")
    return common


def slot_view(
    config: "cli.Config", slot: str, slot_type: str, *, isolated_git: bool = False
) -> sandbox.SlotView:
    """Describe a registered slot for the sandbox, whatever its representation.

    ``isolated_git`` (the isolated Git configuration policy) finds the Git
    common directories with :func:`_isolated_common_directory` instead of the
    ordinary lookup.
    """

    from wrkslots import cli

    state = cli._load_active(config)
    record = cli._find_record(state, slot)
    if record.slot_type != slot_type:
        raise _refuse(f"slot {slot!r} is a {record.slot_type} slot, not {slot_type}")
    slot_path = cli._slot_directory(config, slot, slot_type)
    try:
        image = slotimage.load(config.control, slot_type, slot)
    except slotimage.ImageError as exc:
        raise _refuse(str(exc)) from exc
    # Representation decides only where the slot's private state lives.
    if image is not None:
        state_directory = image.state_mount
    else:
        state_directory = config.control / "slot-state" / slot_type / slot
    vcs = cli._GitVcs()
    common_directory = _isolated_common_directory if isolated_git else vcs.common_directory
    git_directories: list[Path] = []
    for repository, path, _head in _checkout_paths(config, record):
        common = common_directory(path if path.exists() else repository)
        if common not in git_directories:
            git_directories.append(common)
    return sandbox.SlotView(
        slot=slot,
        slot_type=slot_type,
        slot_path=slot_path,
        state_directory=state_directory,
        git_directories=tuple(git_directories),
        control_directory=config.control,
        representation="image" if image is not None else "worktree",
        project_root=config.root,
    )


def _project_git_directories(config: "cli.Config", state: "cli.ActiveState") -> list[Path]:
    """Git common directories a coordinator commits into or creates slots from.

    The project root's own repository (with its modules/ and worktrees/), every
    repository directly below the project root, and every repository an active
    slot's checkouts belong to.
    """

    from wrkslots import cli

    vcs = cli._GitVcs()
    found: list[Path] = []

    def add(repository: Path) -> None:
        try:
            common = vcs.common_directory(repository)
        except (cli.Refusal, cli.StateError, OSError):
            return
        if common not in found:
            found.append(common)

    managed = cli._managed_worktrees_root(config)
    if (config.root / ".git").exists():
        add(config.root)
    try:
        children = sorted(config.root.iterdir())
    except OSError:
        children = []
    for child in children:
        if child in (managed, config.control) or child.is_symlink() or not child.is_dir():
            continue
        if (child / ".git").exists():
            add(child)
    for record in state.slots:
        for repository, path, _head in _checkout_paths(config, record):
            add(path if path.exists() else repository)
    return found


def box_view(config: "cli.Config", name: str, scope: str) -> sandbox.SlotView:
    """Describe a coordinator box: no slot; the registry and slots (or the project) writable."""

    from wrkslots import cli

    cli._validate_name(name, "box name")
    if scope not in sandbox.COORDINATOR_SCOPES:
        raise _refuse(f"--writable must be one of {', '.join(sandbox.COORDINATOR_SCOPES)}")
    state = cli._load_active(config)
    managed = cli._managed_worktrees_root(config)
    roots = cli._slot_roots(config)
    if scope == "project":
        primary = config.root
        writable = [config.root]
    else:
        primary = managed
        writable = [managed, config.control, *roots.values()]
    for directory in _project_git_directories(config, state):
        writable.append(directory)
    # Image slots are mounted at the slot directories, their state images under
    # the control directory: those are the mounts the box must see arrive.
    propagate = [config.control, *roots.values()]
    # A root below another writable root is already writable: one bind each.
    unique = [
        path
        for index, path in enumerate(writable)
        if path not in writable[:index] and not any(other in path.parents for other in writable)
    ]
    return sandbox.SlotView(
        slot=name,
        slot_type="box",
        slot_path=primary,
        state_directory=config.control / "box-state" / name,
        git_directories=(),
        control_directory=config.control,
        representation=scope,
        project_root=config.root,
        coordinator=True,
        writable=tuple(path for path in unique if path != primary),
        propagate=tuple(dict.fromkeys(propagate)),
    )


def _agent_registry(view: sandbox.SlotView, cwd: Path, *, create: bool = True) -> sandbox.SlotView:
    """Make ``cwd/.agentctl`` (agentctl's default registry) writable when it lies in the project."""

    registry = cwd / ".agentctl"
    root = view.project_root
    if root is None or not (registry == root / ".agentctl" or root in registry.parents):
        return view
    covered = [view.slot_path, *view.writable]
    if any(registry == path or path in registry.parents for path in covered):
        return view
    if registry.is_symlink():
        raise _refuse(f"the agent registry {registry} is a symbolic link; remove it or use a real directory")
    if not registry.exists():
        if not create:
            return dataclasses.replace(view, writable=(*view.writable, registry))
        registry.mkdir(mode=0o700)
    if registry.is_symlink() or not registry.is_dir():
        raise _refuse(f"the agent registry {registry} is not a real directory")
    return dataclasses.replace(view, writable=(*view.writable, registry))


def _cmd_box(args: argparse.Namespace) -> int:
    config = _config(args)
    try:
        settings = _override_settings(config.sandbox_settings, args)
        view = box_view(config, args.name, settings.coordinator_writable)
        cwd = Path(args.cwd).absolute() if args.cwd else config.root
        if not cwd.is_dir():
            raise _refuse(f"--cwd {cwd} is not a directory")
        if settings.isolation != "cgroup":
            view = _agent_registry(view, cwd, create=not args.print_only)
        command = list(args.command)
        if not command:
            command = [os.environ.get("SHELL", "/bin/sh")]
        return sandbox.run(view, settings, command, cwd=cwd, print_only=args.print_only)
    except sandbox.SandboxError as exc:
        raise _refuse(str(exc)) from exc


def _cmd_run(args: argparse.Namespace) -> int:
    config = _config(args)
    git_config_policy: str | None = getattr(args, "git_config_policy", None)
    view = slot_view(config, args.slot, args.slot_type, isolated_git=git_config_policy == "isolated")
    deadline_ns: int | None = getattr(args, "deadline_monotonic_ns", None)
    output_format: str | None = getattr(args, "format", None)
    try:
        settings = _override_settings(config.sandbox_settings, args)
        in_scope: str | None = getattr(args, "in_scope", None)
        expect_slices: str | None = getattr(args, "expect_slices", None)
        expect_writable: str | None = getattr(args, "expect_writable", None)
        expect_masks: str | None = getattr(args, "expect_masks", None)
        closed_set = bool(getattr(args, "closed_set", False))
        if getattr(args, "place_only", False):
            for option, given in (
                ("--in-scope", in_scope is not None),
                ("--expect-slices", expect_slices is not None),
                ("--expect-writable", expect_writable is not None),
                ("--git-config-policy", git_config_policy is not None),
                ("--expect-masks", expect_masks is not None),
                ("--closed-set", closed_set),
            ):
                if given:
                    raise _refuse(f"--place-only runs nothing and takes no {option}")
            return _place_only(args, view, settings, deadline_ns, output_format or "human")
        if output_format is not None:
            raise _refuse("--format applies only with --place-only")
        if in_scope is not None or expect_slices is not None:
            return _in_scope(args, view, settings, in_scope, expect_slices, closed_set, deadline_ns)
        if expect_writable is not None:
            raise _refuse("--expect-writable applies only with --in-scope")
        for option, given in (
            ("--git-config-policy", git_config_policy is not None),
            ("--expect-masks", expect_masks is not None),
        ):
            if given:
                raise _refuse(f"{option} applies only with --in-scope")
        for option, given in (
            ("--deadline-monotonic-ns", deadline_ns is not None),
            ("--closed-set", closed_set),
        ):
            if given:
                raise _refuse(f"{option} applies only with --place-only or --in-scope")
        command = list(args.command)
        if not command:
            command = [os.environ.get("SHELL", "/bin/sh")]
        return sandbox.run(
            view,
            settings,
            command,
            cwd=Path(args.cwd).absolute() if args.cwd else None,
            print_only=args.print_only,
        )
    except sandbox.SandboxError as exc:
        raise _refuse(str(exc)) from exc


def _place_only(
    args: argparse.Namespace,
    view: sandbox.SlotView,
    settings: sandbox.SandboxSettings,
    deadline_ns: int | None,
    output_format: str,
) -> int:
    """``run --place-only``: apply the slot's slice limits and print the names; run nothing."""

    import sys

    for option, given in (
        ("COMMAND", bool(args.command)),
        ("--print", bool(args.print_only)),
        ("--cwd", args.cwd is not None),
    ):
        if given:
            raise _refuse(f"--place-only runs nothing and takes no {option}")
    placement, applied = sandbox.place_only(view, settings.limits, deadline_ns=deadline_ns)
    for line in applied:
        print(f"wrkslots run: limits {line}", file=sys.stderr)
    if output_format == "json":
        print(json.dumps(placement.to_obj(), sort_keys=True))
    else:
        print(f"enclosing: {placement.enclosing or '-'}")
        print(f"all_slots: {placement.all_slots}")
        print(f"slot: {placement.slot}")
    return 0


def _expected_slices(text: str) -> sandbox.SlicePlacement:
    try:
        loaded: object = json.loads(text)
    except ValueError as exc:
        raise _refuse(f"--expect-slices must be the JSON object --place-only printed: {exc}") from exc
    keys = {"enclosing", "all_slots", "slot"}
    if not isinstance(loaded, dict) or set(loaded) != keys:
        raise _refuse("--expect-slices must be an object with exactly enclosing, all_slots and slot")
    enclosing, all_slots, slot = loaded["enclosing"], loaded["all_slots"], loaded["slot"]
    if not (enclosing is None or (isinstance(enclosing, str) and enclosing)):
        raise _refuse("--expect-slices enclosing must be a slice name or null")
    if not (isinstance(all_slots, str) and all_slots and isinstance(slot, str) and slot):
        raise _refuse("--expect-slices all_slots and slot must be slice names")
    return sandbox.SlicePlacement(enclosing=enclosing, all_slots=all_slots, slot=slot)


def _expected_writable(text: str | None) -> dict[str, object] | None:
    if text is None:
        return None
    try:
        loaded: object = json.loads(text)
    except ValueError as exc:
        raise _refuse(f"--expect-writable must be the JSON object --print reported as writable: {exc}") from exc
    if not isinstance(loaded, dict):
        raise _refuse("--expect-writable must be the JSON object --print reported as writable")
    return {str(key): value for key, value in loaded.items()}


def _expected_masks(text: str | None) -> dict[str, object] | None:
    if text is None:
        return None
    try:
        loaded: object = json.loads(text)
    except ValueError as exc:
        raise _refuse(f"--expect-masks must be the JSON object --print reported as masks: {exc}") from exc
    if not isinstance(loaded, dict):
        raise _refuse("--expect-masks must be the JSON object --print reported as masks")
    return {str(key): value for key, value in loaded.items()}


def _in_scope(
    args: argparse.Namespace,
    view: sandbox.SlotView,
    settings: sandbox.SandboxSettings,
    unit: str | None,
    expect_slices: str | None,
    closed_set: bool,
    deadline_ns: int | None,
) -> int:
    """``run --in-scope``: enter the slot's box from inside the caller's own scope."""

    if unit is None or expect_slices is None:
        raise _refuse("--in-scope and --expect-slices are given together")
    if closed_set and args.no_protect_system:
        raise _refuse("--closed-set refuses --no-protect-system: every other mount must stay read-only")
    expected = _expected_slices(expect_slices)
    expected_writable = _expected_writable(getattr(args, "expect_writable", None))
    expected_masks = _expected_masks(getattr(args, "expect_masks", None))
    git_config_policy = getattr(args, "git_config_policy", None) or "inherit"
    command = list(args.command)
    cwd = Path(args.cwd).absolute() if args.cwd else None
    if args.print_only:
        plan = sandbox.in_scope_plan(
            view,
            settings,
            command,
            unit=unit,
            expected=expected,
            cwd=cwd,
            slot_writable=not closed_set,
            deadline_ns=deadline_ns,
            expected_writable=expected_writable,
            git_config_policy=git_config_policy,
            expected_masks=expected_masks,
        )
        print(json.dumps(plan, indent=2, sort_keys=True))
        return 0
    sandbox.enter_in_place(
        view,
        settings,
        command,
        unit=unit,
        expected=expected,
        cwd=cwd,
        slot_writable=not closed_set,
        deadline_ns=deadline_ns,
        expected_writable=expected_writable,
        git_config_policy=git_config_policy,
        expected_masks=expected_masks,
    )
    raise AssertionError("enter_in_place returns only by raising")


def _cmd_sandbox_help(args: argparse.Namespace) -> int:
    parser = args.sandbox_parser
    assert isinstance(parser, argparse.ArgumentParser)
    parser.print_help()
    return 0


def _cmd_sandbox_show(args: argparse.Namespace) -> int:
    config = _config(args)
    print(json.dumps(sandbox.settings_to_obj(config.sandbox_settings), indent=2))
    return 0


def _cmd_sandbox_write_defaults(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    with cli._locked_config(config.config_path, args.wait_lock):
        raw = cli._read_config(config.config_path)
        existing = raw.get("sandbox", {})
        if not isinstance(existing, dict):
            raise _refuse("configuration.sandbox must be an object")
        try:
            merged, added = sandbox.merge_defaults(existing)
        except sandbox.SandboxError as exc:
            raise _refuse(str(exc)) from exc
        if added:
            updated = dict(raw)
            updated["sandbox"] = merged
            cli._write_config(config.config_path, updated)
    if added:
        print(f"added to configuration.sandbox in {config.config_path}: {', '.join(added)}")
    else:
        print(f"configuration.sandbox in {config.config_path} already has every key")
    return 0


def _cmd_shell_command(args: argparse.Namespace) -> int:
    import shlex
    import sys

    from wrkslots import cli

    config = _config(args)
    command = list(args.command)
    if args.box:
        if args.slot is not None:
            # With --box there is no SLOT: argparse hands the first word after -- to it.
            # A word before -- is a mistaken SLOT, not a program to run boxed.
            raw = list(getattr(args, "raw_arguments", []) or [])
            after = raw[raw.index("--") + 1 :] if "--" in raw else []
            if not after or after[0] != args.slot:
                raise _refuse(
                    f"shell-command --box takes no SLOT ({args.slot!r}); put a program to run "
                    "boxed after a literal --"
                )
            command = [args.slot, *command]
        cli._validate_name(args.name, "box name")
    else:
        if args.slot is None:
            raise _refuse("shell-command needs a SLOT, or --box for a coordinator box")
        state = cli._load_active(config)
        cli._find_record(state, args.slot)
    if not sandbox.scope_in_place_available():
        # The line must keep the pane's process ID (a multiplexer checks it); the
        # fallback that `wrkslots run` uses without D-Bus cannot.
        raise _refuse(
            "shell-command needs the systemd user manager's D-Bus API (busctl --user) so the "
            "boxed shell keeps the pane's process ID; it is unavailable here"
        )
    shell = args.shell or os.environ.get("SHELL") or "/bin/bash"
    # The real interpreter (a wrapper script that forks would change the PID)
    # running the entry script in isolated mode: nothing is added to the
    # environment the boxed shell inherits (no PYTHONPATH).
    argv = [
        "exec",
        os.path.realpath(sys.executable),
        "-I",
        str(Path(__file__).resolve().with_name("__main__.py")),
        "--project-root",
        str(config.root),
    ]
    if args.box:
        cwd = Path(args.cwd).absolute() if args.cwd else config.root
        argv += ["box", "--name", args.name, "--cwd", str(cwd)]
        if args.writable is not None:
            argv += ["--writable", args.writable]
    else:
        argv += ["run", args.slot, "--slot-type", args.slot_type]
    if args.isolation is not None:
        argv += ["--isolation", args.isolation]
    if command:
        # A command given after -- replaces the interactive shell: the pane then
        # runs that program boxed, and closes when it exits.
        argv += ["--", *command]
    else:
        argv += ["--", os.path.realpath(shutil.which(shell) or shell), "-i"]
    line = " ".join(shlex.quote(part) if index else part for index, part in enumerate(argv))
    if args.format == "json":
        isolation = args.isolation or config.sandbox_settings.isolation
        if args.box:
            # slot_path is where the pane lands: the box's working directory.
            payload = {"command": line, "slot_path": str(cwd), "isolation": isolation, "box": args.name}
        else:
            slot_path = cli._slot_directory(config, args.slot, args.slot_type)
            payload = {"command": line, "slot_path": str(slot_path), "isolation": isolation}
        print(json.dumps(payload))
    else:
        print(line)
    return 0


def _cmd_limits(args: argparse.Namespace) -> int:
    import subprocess

    unit = f"user-{os.getuid()}.slice"
    keys = ("MemoryMax", "MemorySwapMax", "CPUQuotaPerSecUSec")
    if args.action in ("apply", "clear"):
        if args.action == "apply":
            for label, value in (("--memory-fraction", args.memory_fraction), ("--cpu-fraction", args.cpu_fraction)):
                if not 0 < value <= 1:
                    raise _refuse(f"{label} must be greater than 0 and at most 1")
            memory = int(os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE") * args.memory_fraction)
            cpu = max(1, int((os.cpu_count() or 1) * 100 * args.cpu_fraction))
            properties = [f"MemoryMax={memory}", "MemorySwapMax=0", f"CPUQuota={cpu}%"]
        else:
            properties = ["MemoryMax=infinity", "MemorySwapMax=infinity", "CPUQuota="]
        result = subprocess.run(
            ["sudo", "-n", "systemctl", "set-property", "--runtime", unit, *properties],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise _refuse(f"cannot set {unit} limits (needs sudo -n): {result.stderr.strip()}")
    shown = subprocess.run(
        ["systemctl", "show", unit, *(f"--property={key}" for key in keys)],
        capture_output=True,
        text=True,
        check=False,
    )
    print(f"{unit}:")
    for line in shown.stdout.splitlines():
        print(f"  {line}")
    return 0 if shown.returncode == 0 else 1


def _override_settings(settings: sandbox.SandboxSettings, args: argparse.Namespace) -> sandbox.SandboxSettings:
    """Apply per-run options on top of the configured settings, then re-validate."""

    merged = sandbox.settings_to_obj(settings)
    if getattr(args, "closed_set", False):
        # A closed set: the per-run options are the whole list, not additions.
        for key in ("home_shared", "home_private", "outputs", "read_write"):
            merged[key] = []
    if getattr(args, "writable", None) is not None:
        merged["coordinator_writable"] = args.writable
    if args.isolation is not None:
        merged["isolation"] = args.isolation
    if args.home is not None:
        merged["home"] = args.home
    for option, key in (
        ("home_shared", "home_shared"),
        ("home_private", "home_private"),
        ("home_private_file", "home_private_files"),
        ("home_hidden", "home_hidden"),
        ("home_expose", "home_expose"),
        ("output", "outputs"),
        ("read_write", "read_write"),
    ):
        extra = getattr(args, option)
        if extra:
            current = merged[key]
            assert isinstance(current, list)
            merged[key] = [*current, *extra]
    if args.env:
        env = merged["env"]
        assert isinstance(env, dict)
        for item in args.env:
            name, separator, value = item.partition("=")
            if not separator:
                raise sandbox.SandboxError(f"--env needs NAME=VALUE, not {item!r}")
            env[name] = value
    if args.tmp_size is not None:
        merged["tmp_size"] = args.tmp_size
    if args.no_protect_system:
        merged["protect_system"] = False
    limits = merged["limits"]
    assert isinstance(limits, dict)
    for option in ("memory_max", "memory_high", "cpu_quota"):
        value = getattr(args, option)
        if value is not None:
            limits[option] = sandbox.validate_limit(value, "--" + option.replace("_", "-"))
    if args.tasks_max is not None:
        limits["tasks_max"] = args.tasks_max
    return sandbox.settings_from_obj(merged)
