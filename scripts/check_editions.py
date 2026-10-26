#!/usr/bin/env python3
"""Prove `validation/editions.json` still describes the tracked tools.

The editions file is the mirror-state model for the rust-primary campaign
(#539 rust-primary-mirror-state): per tool, the primary edition, the mirror's
state (parity, paused, archived, absent), and the parity check that keeps a
parity mirror honest. Two consumers read the model: the engine resolver at
dispatch time (through the generated `validation/editions.tsv`, one
tab-separated line per tool, so no JSON parsing happens in the dispatcher),
and this checker at validation time. This checker is the half that can fail
loudly in CI; it proves, offline:

* every command linked into `common/bin` has exactly one editions entry, and
  every tracked `rs/bin` or `py/bin` entrypoint is named by exactly the entry
  whose primary/mirror state it matches (a parity mirror names an existing
  entrypoint, an absent mirror names none, entrypoints sit on their own
  edition's side, and a tool with no entrypoints names a real Rust crate);
* a `parity` mirror names a parity check that exists as a job in
  `validation/cross.dag.yaml`, and a paused mirror names a commit-shaped
  `paused_at`;
* the generated `validation/editions.tsv` is byte-identical to what this
  file's tools generate (regenerate with `--write-tsv`);
* the agentctl python-only subcommands in the file are exactly the lists the
  Rust edition refuses by name (`PYTHON_ONLY_COMMANDS` and
  `PYTHON_ONLY_CHAT_COMMANDS` in `rs/agentctl/src/cli.rs`), each recorded as
  python-primary with an absent Rust mirror;
* every format inventory entry names its Rust and Python definitions (or
  says there are none), a duplicate verdict, and a collapse step; the paths
  those definitions cite exist; schema ids do not repeat; and the inventory
  still covers the campaign's named formats.

Usage:
    python3 scripts/check_editions.py
    python3 scripts/check_editions.py --self-test
    python3 scripts/check_editions.py --write-tsv
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from collections.abc import Mapping, Sequence, Set
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

EDITION_NAMES = ("rust", "python")
MIRROR_STATES = ("parity", "paused", "archived", "absent")
PATH_TOKEN = re.compile(r"(?:rs|py|vibe-talk|validation|cross|scripts)/[A-Za-z0-9_./-]+")
COMMIT_SHAPE = re.compile(r"^[0-9a-f]{7,40}$")
REQUIRED_FORMAT_TOPICS = (
    "queue",
    "registry",
    "coordinator designation",
    "landing queue",
    "launch spec",
    "chat bridge",
    "agent-teams",
    "wrkslots",
    "wrkviz",
    "validation components",
    "vibe-talk",
)


def _load(path: Path) -> dict[str, object]:
    value: object = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: top level is not a JSON object")
    return value


def _objects(document: Mapping[str, object], key: str) -> list[Mapping[str, object]]:
    items = document.get(key)
    if not isinstance(items, list) or not all(isinstance(item, dict) for item in items):
        raise ValueError(f"editions: {key!r} is not a list of objects")
    return items


def _mirror(tool: Mapping[str, object], where: str) -> Mapping[str, object]:
    mirror = tool.get("mirror")
    if not isinstance(mirror, dict):
        raise ValueError(f"{where}: 'mirror' is not an object")
    return mirror


def expected_tsv(document: Mapping[str, object]) -> str:
    """The resolver's view of the model: one tab-separated line per tool."""
    lines = []
    for tool in _objects(document, "tools"):
        mirror = _mirror(tool, str(tool.get("name")))
        paused_at = mirror.get("paused_at")
        lines.append(
            "\t".join(
                [
                    str(tool.get("name")),
                    str(tool.get("primary")),
                    str(mirror.get("state")),
                    str(paused_at) if isinstance(paused_at, str) else "",
                ]
            )
        )
    return "\n".join(lines) + "\n"


def check_document(
    document: Mapping[str, object],
    *,
    resolver_tools: Sequence[str],
    entrypoints: Set[str],
    rust_crates: Set[str],
    rust_python_only: Sequence[str],
    rust_python_only_chat: Sequence[str],
    cross_jobs: Set[str],
    known_paths: Set[str],
    tsv_text: str | None,
) -> list[str]:
    """Return every way `document` disagrees with the tree it describes."""
    failures: list[str] = []
    if document.get("schema_version") != 1:
        failures.append("editions: schema_version is not 1")
    tools = _objects(document, "tools")
    by_name: dict[str, Mapping[str, object]] = {}
    for tool in tools:
        name = tool.get("name")
        if not isinstance(name, str) or not name:
            failures.append(f"editions: tool without a name: {tool!r}")
            continue
        where = f"editions tool {name}"
        if name in by_name:
            failures.append(f"{where}: duplicate tool entry")
        by_name[name] = tool
        primary = tool.get("primary")
        if primary not in EDITION_NAMES:
            failures.append(f"{where}: primary {primary!r} is not one of {EDITION_NAMES}")
        mirror = _mirror(tool, where)
        mirror_edition = mirror.get("edition")
        if mirror_edition not in EDITION_NAMES or mirror_edition == primary:
            failures.append(f"{where}: mirror edition {mirror_edition!r} must be the other edition")
        state = mirror.get("state")
        if state not in MIRROR_STATES:
            failures.append(f"{where}: mirror state {state!r} is not one of {MIRROR_STATES}")
        if state == "parity":
            parity_check = tool.get("parity_check")
            token = parity_check.split()[0] if isinstance(parity_check, str) and parity_check else ""
            if not token:
                failures.append(f"{where}: a parity mirror names no parity check")
            elif token not in cross_jobs:
                failures.append(f"{where}: parity check {token!r} is not a job in validation/cross.dag.yaml")
        if state == "paused":
            paused_at = mirror.get("paused_at")
            if not isinstance(paused_at, str) or not COMMIT_SHAPE.match(paused_at):
                failures.append(f"{where}: a paused mirror names no commit-shaped paused_at")
        entry_for = {
            "rust": tool.get("rust_entrypoint"),
            "python": tool.get("python_entrypoint"),
        }
        for edition, entry in entry_for.items():
            if entry is not None:
                if not isinstance(entry, str) or not entry.startswith(f"{'rs' if edition == 'rust' else 'py'}/bin/"):
                    failures.append(f"{where}: {edition} entrypoint {entry!r} is not on the {edition} side")
                elif entry not in entrypoints:
                    failures.append(f"{where}: {edition} entrypoint {entry!r} does not exist")
        if isinstance(primary, str) and primary in entry_for and name in resolver_tools:
            if entry_for[primary] is None:
                failures.append(f"{where}: primary {primary} edition has no entrypoint")
        if state == "parity" and isinstance(mirror_edition, str) and entry_for.get(mirror_edition) is None:
            failures.append(f"{where}: a parity mirror has no entrypoint")
        if state == "absent" and isinstance(mirror_edition, str) and entry_for.get(mirror_edition) is not None:
            failures.append(f"{where}: an absent mirror names an entrypoint; the state is not absent")
        if tool.get("rust_entrypoint") is None and tool.get("python_entrypoint") is None and name not in rust_crates:
            failures.append(f"{where}: no entrypoints and no rs/{name} crate; the entry names nothing tracked")
    for tool_name in resolver_tools:
        if tool_name not in by_name:
            failures.append(f"editions: common/bin/{tool_name} has no editions entry")
    named: set[str] = {
        entry
        for tool in tools
        for key in ("rust_entrypoint", "python_entrypoint")
        if isinstance((entry := tool.get(key)), str)
    }
    for entry in sorted(set(entrypoints) - named):
        failures.append(f"editions: tracked entrypoint {entry} is named by no tool entry")
    if tsv_text is not None and tsv_text != expected_tsv(document):
        failures.append("editions: validation/editions.tsv is stale; regenerate with scripts/check_editions.py --write-tsv")
    failures.extend(_check_subcommands(by_name, rust_python_only, rust_python_only_chat))
    failures.extend(_check_formats(_objects(document, "formats"), known_paths))
    return failures


def _check_subcommands(
    by_name: Mapping[str, Mapping[str, object]],
    rust_python_only: Sequence[str],
    rust_python_only_chat: Sequence[str],
) -> list[str]:
    failures: list[str] = []
    agentctl = by_name.get("agentctl")
    if agentctl is None:
        return ["editions: no agentctl tool entry"]
    listed: list[str] = []
    subcommands = agentctl.get("subcommands")
    if isinstance(subcommands, list):
        for item in subcommands:
            if isinstance(item, dict) and isinstance(item.get("name"), str):
                listed.append(item["name"])
                if item.get("primary") != "python":
                    failures.append(f"editions agentctl subcommand {item['name']}: primary is not python")
                sub_mirror = item.get("mirror")
                if not isinstance(sub_mirror, dict) or sub_mirror.get("state") != "absent":
                    failures.append(f"editions agentctl subcommand {item['name']}: mirror state is not absent (the Rust edition refuses it by name)")
    expected = list(rust_python_only) + [f"chat {name}" for name in rust_python_only_chat]
    if sorted(listed) != sorted(expected):
        failures.append(
            "editions: agentctl python-only subcommands "
            f"{sorted(listed)} do not match the Rust refusal lists {sorted(expected)}"
        )
    return failures


def _check_formats(formats: Sequence[Mapping[str, object]], known_paths: Set[str]) -> list[str]:
    failures: list[str] = []
    seen: set[str] = set()
    schema_ids: dict[str, str] = {}
    topics = ""
    for item in formats:
        name = item.get("name")
        if not isinstance(name, str) or not name:
            failures.append(f"editions: format without a name: {item!r}")
            continue
        topics += f" {name.lower()}"
        if name in seen:
            failures.append(f"editions format {name}: duplicate format entry")
        seen.add(name)
        for key in ("schema_id", "rust_definition", "python_definition", "generated_schema", "duplicate", "collapse"):
            if key not in item:
                failures.append(f"editions format {name}: missing {key}")
        schema_id = item.get("schema_id")
        if isinstance(schema_id, str):
            if schema_id in schema_ids:
                failures.append(f"editions format {name}: schema_id {schema_id!r} also names {schema_ids[schema_id]}")
            schema_ids[schema_id] = name
        for key in ("rust_definition", "python_definition", "generated_schema"):
            text = item.get(key)
            if not isinstance(text, str):
                continue
            for token in PATH_TOKEN.findall(text):
                cleaned = token.rstrip(".,)")
                if cleaned not in known_paths:
                    failures.append(f"editions format {name}: {key} cites unknown path {cleaned}")
    for topic in REQUIRED_FORMAT_TOPICS:
        if topic not in topics:
            failures.append(f"editions: format inventory no longer covers {topic!r}")
    return failures


def _rust_refusal_lists(cli_source: str) -> tuple[list[str], list[str]]:
    def names(constant: str) -> list[str]:
        match = re.search(rf"const {constant}: \[&str; \d+\] = \[([^\]]*)\];", cli_source)
        if match is None:
            raise ValueError(f"cannot find {constant} in rs/agentctl/src/cli.rs")
        return re.findall(r'"([^"]+)"', match.group(1))

    return names("PYTHON_ONLY_COMMANDS"), names("PYTHON_ONLY_CHAT_COMMANDS")


def _cross_jobs(dag_text: str) -> set[str]:
    jobs: set[str] = set()
    group = ""
    for line in dag_text.splitlines():
        group_match = re.match(r"  - group: (\S+)", line)
        job_match = re.match(r"    job: (\S+)", line)
        if group_match:
            group = group_match.group(1)
        elif job_match and group:
            jobs.add(f"cross.{group}.{job_match.group(1)}")
    return jobs


def run_check(root: Path = REPO_ROOT) -> list[str]:
    editions_path = root / "validation" / "editions.json"
    document = _load(editions_path)
    tsv_path = root / "validation" / "editions.tsv"
    resolver_tools = sorted(
        path.name
        for path in (root / "common" / "bin").iterdir()
        if path.name != "engine-resolver" and path.is_symlink()
    )
    entrypoints = {
        f"{side}/bin/{path.name}"
        for side in ("rs", "py")
        for path in (root / side / "bin").iterdir()
        if path.name != "cargo-runner"
    }
    rust_crates = {path.name for path in (root / "rs").iterdir() if path.is_dir()}
    python_only, python_only_chat = _rust_refusal_lists(
        (root / "rs" / "agentctl" / "src" / "cli.rs").read_text(encoding="utf-8")
    )
    known_paths = {
        str(path.relative_to(root))
        for path in root.rglob("*")
        if ".git" not in path.parts and "target" not in path.parts
    }
    return check_document(
        document,
        resolver_tools=resolver_tools,
        entrypoints=entrypoints,
        rust_crates=rust_crates,
        rust_python_only=python_only,
        rust_python_only_chat=python_only_chat,
        cross_jobs=_cross_jobs((root / "validation" / "cross.dag.yaml").read_text(encoding="utf-8")),
        known_paths=known_paths,
        tsv_text=tsv_path.read_text(encoding="utf-8") if tsv_path.exists() else None,
    )


def _self_test() -> int:
    failures: list[str] = []

    agentctl_tool: dict[str, object] = {
        "name": "agentctl",
        "primary": "rust",
        "mirror": {"edition": "python", "state": "parity"},
        "parity_check": "cross.agentctl.differential",
        "rust_entrypoint": "rs/bin/agentctl",
        "python_entrypoint": "py/bin/agentctl",
        "subcommands": [
            {"name": "mcp", "primary": "python", "mirror": {"edition": "rust", "state": "absent"}, "parity_check": None},
            {"name": "chat launch", "primary": "python", "mirror": {"edition": "rust", "state": "absent"}, "parity_check": None},
        ],
    }
    tick_hub: dict[str, object] = {
        "name": "tick-hub",
        "primary": "rust",
        "mirror": {"edition": "python", "state": "parity"},
        "parity_check": "cross.tick-hub.differential",
        "rust_entrypoint": "rs/bin/tick-hub",
        "python_entrypoint": "py/bin/tick-hub",
    }
    format_entry: dict[str, object] = {
        "name": "agent queue items",
        "schema_id": "test-schema/v1",
        "rust_definition": "rs/agent-queue/src/queue.rs",
        "python_definition": None,
        "generated_schema": None,
        "duplicate": "yes",
        "collapse": "one Rust type",
    }

    def doc(tool: Mapping[str, object]) -> dict[str, object]:
        return {"schema_version": 1, "tools": [tool, agentctl_tool], "formats": [format_entry]}

    def run(document: Mapping[str, object], tsv: str | None = None) -> list[str]:
        return check_document(
            document,
            resolver_tools=["tick-hub", "agentctl"],
            entrypoints={"rs/bin/tick-hub", "py/bin/tick-hub", "rs/bin/agentctl", "py/bin/agentctl"},
            rust_crates={"agent-teams"},
            rust_python_only=["mcp"],
            rust_python_only_chat=["launch"],
            cross_jobs={"cross.tick-hub.differential", "cross.agentctl.differential"},
            known_paths={"rs/agent-queue/src/queue.rs"},
            tsv_text=expected_tsv(document) if tsv is None else tsv,
        )

    def expect(label: str, document: Mapping[str, object], wanted: str | None, tsv: str | None = None) -> None:
        found = run(document, tsv)
        # The synthetic formats list cannot cover every required topic; ignore those here.
        found = [item for item in found if "no longer covers" not in item]
        if wanted is None and found:
            failures.append(f"{label}: expected no failures, got {found}")
        if wanted is not None and not any(wanted in item for item in found):
            failures.append(f"{label}: expected a failure mentioning {wanted!r}, got {found}")

    expect("parity mirror with a real check passes", doc(tick_hub), None)
    expect("parity mirror without a check fails", doc(dict(tick_hub, parity_check=None)), "names no parity check")
    expect("parity check that is not a cross job fails", doc(dict(tick_hub, parity_check="cross.nope.nope")), "not a job in validation/cross.dag.yaml")
    paused = dict(tick_hub, mirror={"edition": "python", "state": "paused"}, parity_check=None)
    expect("paused mirror without a commit fails", doc(paused), "paused_at")
    expect("paused mirror with a fake commit fails", doc(dict(paused, mirror={"edition": "python", "state": "paused", "paused_at": "not-a-commit"})), "paused_at")
    expect("missing entrypoint fails", doc(dict(tick_hub, rust_entrypoint="rs/bin/nope")), "does not exist")
    expect("swapped entrypoint fails", doc(dict(tick_hub, rust_entrypoint="py/bin/tick-hub")), "not on the rust side")
    expect("absent mirror naming an entrypoint fails", doc(dict(tick_hub, mirror={"edition": "python", "state": "absent"}, parity_check=None)), "state is not absent")
    expect("stale tsv fails", doc(tick_hub), "stale", tsv="tick-hub\tpython\tparity\t\n")

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "validation").mkdir()
        (root / "common" / "bin").mkdir(parents=True)
        (root / "rs" / "bin").mkdir(parents=True)
        (root / "py" / "bin").mkdir(parents=True)
        (root / "rs" / "agentctl" / "src").mkdir(parents=True)
        for tool in ("tick-hub", "agentctl"):
            (root / "common" / "bin" / tool).symlink_to("engine-resolver")
            (root / "rs" / "bin" / tool).touch()
            (root / "py" / "bin" / tool).touch()
        (root / "rs" / "agentctl" / "src" / "cli.rs").write_text(
            'const PYTHON_ONLY_COMMANDS: [&str; 1] = ["mcp"];\n'
            'const PYTHON_ONLY_CHAT_COMMANDS: [&str; 1] = ["launch"];\n',
            encoding="utf-8",
        )
        (root / "validation" / "cross.dag.yaml").write_text(
            "steps:\n  - group: tick-hub\n    job: differential\n  - group: agentctl\n    job: differential\n",
            encoding="utf-8",
        )
        document = doc(tick_hub)
        (root / "validation" / "editions.json").write_text(json.dumps(document), encoding="utf-8")
        (root / "validation" / "editions.tsv").write_text(expected_tsv(document), encoding="utf-8")
        found = [item for item in run_check(root) if "no longer covers" not in item and "cites unknown path" not in item]
        if found:
            failures.append(f"real-tree shape: expected no failures, got {found}")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("check-editions --self-test: PASSED (10 controls)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="check the guard offline, then exit")
    parser.add_argument("--write-tsv", action="store_true", help="regenerate validation/editions.tsv from editions.json, then exit")
    args = parser.parse_args(argv)
    if args.self_test:
        return _self_test()
    if args.write_tsv:
        document = _load(REPO_ROOT / "validation" / "editions.json")
        (REPO_ROOT / "validation" / "editions.tsv").write_text(expected_tsv(document), encoding="utf-8")
        print("check-editions: wrote validation/editions.tsv")
        return 0
    failures = run_check()
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("check-editions: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
