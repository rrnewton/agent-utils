"""The ``wrkslots image`` and ``wrkslots run`` commands.

Kept apart from ``cli.py`` so the representation and sandbox surface stays
readable; handlers import ``cli`` lazily to reuse its locks, state readers,
refusals, and process-use checks.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
from pathlib import Path
from typing import TYPE_CHECKING

from wrkslots import sandbox, slotimage

if TYPE_CHECKING:
    from wrkslots import cli

GIB = 1024**3


def register(subparsers: argparse._SubParsersAction[argparse.ArgumentParser], formatter: type[argparse.HelpFormatter]) -> None:
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
            "new slots will get."
        ),
        formatter_class=formatter,
    )
    status.add_argument("--format", choices=("human", "json"), default="human")
    status.set_defaults(handler=_cmd_image_status)

    mount = actions.add_parser(
        "mount",
        help="mount every image-backed slot that is not mounted",
        description="Mount all image-backed slots at their recorded locations (idempotent).",
        formatter_class=formatter,
    )
    mount.set_defaults(handler=_cmd_image_mount)

    unmount = actions.add_parser(
        "unmount",
        help="unmount one slot's images while nothing uses the slot",
        description=(
            "Unmount a slot's images after proving no process uses the slot. The next "
            "wrkslots command mounts them again; use this before copying an image file."
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
            "affected; existing slots keep their representation (convert them explicitly)."
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
            "Converting to worktree discards the sandbox's private HOME and /tmp state image. "
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

    run = subparsers.add_parser(
        "run",
        help="run a command boxed to one slot (limits + private file-system view)",
        description=(
            "Run COMMAND as a transient systemd user service in the slot's own slice. The "
            "slot, its private state, the Git directories its checkouts commit into, and the "
            "wrkslots control directory are writable; with protect_system (the default) every "
            "other path is read-only. /tmp and the home_writable directories are private to "
            "the slot. Works for image-backed and plain slots; no root is needed."
        ),
        epilog=(
            "HOME modes: ro exposes all of $HOME read-only; select exposes only --home-path "
            "entries; none exposes nothing. Limits apply to wrkslots-<slot>.slice and are shared "
            "by every command run against the slot. Defaults come from configuration.sandbox. "
            "Example: wrkslots run slot01 --memory-max 32G --cpu-quota 800% -- claude"
        ),
        formatter_class=formatter,
    )
    run.add_argument("slot", help="registered slot name")
    run.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")
    run.add_argument("--home", choices=sandbox.HOME_MODES, help="HOME exposure (default: configuration, else ro)")
    run.add_argument("--home-path", action="append", metavar="REL", help="$HOME-relative path exposed read-only in select mode (repeatable)")
    run.add_argument(
        "--home-writable",
        action="append",
        metavar="REL",
        help=f"$HOME-relative directory made private and writable (repeatable; replaces the default {', '.join(sandbox.DEFAULT_HOME_WRITABLE)})",
    )
    run.add_argument(
        "--home-shared",
        action="append",
        metavar="REL",
        help=f"$HOME-relative path kept shared with the host and writable (repeatable; replaces the default {', '.join(sandbox.DEFAULT_HOME_SHARED)})",
    )
    run.add_argument("--read-write", action="append", metavar="PATH", help="extra absolute path left writable (repeatable)")
    run.add_argument("--memory-max", metavar="SIZE", help="slot memory limit, such as 32G (swap is disabled when set)")
    run.add_argument("--memory-high", metavar="SIZE", help="slot memory throttling threshold, such as 24G")
    run.add_argument("--cpu-quota", metavar="PERCENT", help="slot CPU limit, such as 800%% for eight CPUs")
    run.add_argument("--tasks-max", type=int, metavar="N", help=f"slot process and thread limit (default: {sandbox.DEFAULT_TASKS_MAX})")
    run.add_argument("--no-protect-system", action="store_true", help="leave paths outside the slot writable (limits and HOME policy still apply)")
    run.add_argument("--cwd", metavar="PATH", help="working directory (default: the slot directory)")
    run.add_argument(
        "--isolation",
        choices=sandbox.ISOLATIONS,
        default="namespace",
        help=(
            "namespace: limits plus the private file-system view (default); cgroup: limits "
            "only, for programs that need setuid helpers such as sudo or a harness launcher "
            "that enters its own site sandbox"
        ),
    )
    run.add_argument("--print", action="store_true", dest="print_only", help="print the scope, slice limits, and helper command; run nothing")
    run.add_argument("command", nargs="*", help="command to run, after a literal -- (default: $SHELL)")
    run.set_defaults(handler=_cmd_run)

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
            "An agent launcher runs it in a fresh pane before starting a harness."
        ),
        formatter_class=formatter,
    )
    shell_command.add_argument("slot", help="registered slot name")
    shell_command.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")
    shell_command.add_argument("--isolation", choices=sandbox.ISOLATIONS, default="namespace", help="passed to run (default: namespace)")
    shell_command.add_argument("--shell", default=None, metavar="PATH", help="interactive shell (default: $SHELL, else /bin/bash)")
    shell_command.add_argument(
        "--format",
        choices=("text", "json"),
        default="text",
        help="text prints the command line; json prints {command, slot_path} (default: text)",
    )
    shell_command.set_defaults(handler=_cmd_shell_command)


def _slot_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("slot", help="registered slot name")
    parser.add_argument("--slot-type", choices=("agent", "validate"), default="agent", help="slot type (default: agent)")


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
    if args.format == "json":
        print(json.dumps({"new_slot_representation": config.slot_representation, "images": reports}, indent=2))
        return 0
    print(f"new slots: {config.slot_representation}")
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


def _cmd_image_mount(args: argparse.Namespace) -> int:
    config = _config(args)
    try:
        actions = slotimage.ensure_all_mounted(config.control)
    except slotimage.ImageError as exc:
        raise _refuse(str(exc)) from exc
    print("\n".join(actions) if actions else "all image-backed slots are mounted")
    return 0


def _cmd_image_unmount(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    image = _load_image(config, args.slot_type, args.slot)
    with cli._mutation_locks(config, args.wait_lock):
        cli._assert_slot_unused(image.location)
        try:
            slotimage.unmount(image.state_image, image.state_mount)
            slotimage.unmount(image.slot_image, image.location)
        except slotimage.ImageError as exc:
            raise _refuse(str(exc)) from exc
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
        raw = cli._as_mapping(cli._read_json(config.config_path, "configuration"), "configuration")
        updated = dict(raw)
        if args.representation == "worktree":
            updated.pop("slot_representation", None)
        else:
            updated["slot_representation"] = args.representation
        cli._atomic_write_json(config.config_path, updated)
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
    slot_path.rmdir()
    os.rename(staging, slot_path)


def _cmd_run(args: argparse.Namespace) -> int:
    from wrkslots import cli

    config = _config(args)
    state = cli._load_active(config)
    record = cli._find_record(state, args.slot)
    if record.slot_type != args.slot_type:
        raise _refuse(f"slot {args.slot!r} is a {record.slot_type} slot, not {args.slot_type}")
    slot_path = cli._slot_directory(config, args.slot, args.slot_type)
    image = slotimage.load(config.control, args.slot_type, args.slot)
    if image is not None:
        state_directory = image.state_mount
    else:
        state_directory = config.control / "slot-state" / args.slot_type / args.slot
    vcs = cli._GitVcs()
    git_directories: list[Path] = []
    for repository, path, _head in _checkout_paths(config, record):
        common = vcs.common_directory(path if path.exists() else repository)
        if common not in git_directories:
            git_directories.append(common)
    view = sandbox.SlotView(
        slot=args.slot,
        slot_type=args.slot_type,
        slot_path=slot_path,
        state_directory=state_directory,
        git_directories=tuple(git_directories),
        control_directory=config.control,
        representation="image" if image is not None else "worktree",
    )
    settings = config.sandbox_settings
    try:
        settings = _override_settings(settings, args)
        command = list(args.command)
        if not command:
            command = [os.environ.get("SHELL", "/bin/sh")]
        return sandbox.run(
            view,
            settings,
            command,
            cwd=Path(args.cwd).absolute() if args.cwd else None,
            print_only=args.print_only,
            isolation=args.isolation,
        )
    except sandbox.SandboxError as exc:
        raise _refuse(str(exc)) from exc


def _cmd_shell_command(args: argparse.Namespace) -> int:
    import shlex
    import sys

    from wrkslots import cli

    config = _config(args)
    state = cli._load_active(config)
    cli._find_record(state, args.slot)
    shell = args.shell or os.environ.get("SHELL") or "/bin/bash"
    package_parent = str(Path(__file__).resolve().parent.parent)
    argv = [
        "exec",
        "env",
        f"PYTHONPATH={package_parent}",
        os.path.realpath(sys.executable),
        "-m",
        "wrkslots",
        "--project-root",
        str(config.root),
        "run",
        args.slot,
        "--slot-type",
        args.slot_type,
        "--isolation",
        args.isolation,
        "--",
        os.path.realpath(shutil.which(shell) or shell),
        "-i",
    ]
    line = " ".join(shlex.quote(part) if index else part for index, part in enumerate(argv))
    if args.format == "json":
        slot_path = cli._slot_directory(config, args.slot, args.slot_type)
        print(json.dumps({"command": line, "slot_path": str(slot_path)}))
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
    import dataclasses

    changes: dict[str, object] = {}
    if args.home is not None:
        changes["home"] = args.home
    if args.home_path:
        changes["home_paths"] = tuple(args.home_path)
        if args.home is None:
            changes["home"] = "select"
    if args.home_writable:
        changes["home_writable"] = tuple(args.home_writable)
    if args.home_shared:
        changes["home_shared"] = tuple(args.home_shared)
    if args.read_write:
        changes["read_write"] = settings.read_write + tuple(args.read_write)
    if args.memory_max is not None:
        changes["memory_max"] = sandbox.validate_limit(args.memory_max, "--memory-max")
    if args.memory_high is not None:
        changes["memory_high"] = sandbox.validate_limit(args.memory_high, "--memory-high")
    if args.cpu_quota is not None:
        changes["cpu_quota"] = sandbox.validate_limit(args.cpu_quota, "--cpu-quota")
    if args.tasks_max is not None:
        changes["tasks_max"] = args.tasks_max
    if args.no_protect_system:
        changes["protect_system"] = False
    merged = sandbox.settings_from_obj(
        {
            **{
                field.name: (list(value) if isinstance(value, tuple) else value)
                for field in dataclasses.fields(sandbox.SandboxSettings)
                for value in [getattr(settings, field.name)]
            },
            **{key: (list(value) if isinstance(value, tuple) else value) for key, value in changes.items()},
        }
    )
    return merged

