#!/usr/bin/env python3
"""Meter the rust-primary campaign: what still needs a Python interpreter?

For every dispatched tool in `validation/editions.tsv`, this runs the tracked
dispatcher (`common/bin/<tool> --help`) with a PATH that contains every
ordinary system tool except any Python interpreter, and reports one line per
tool:

* ``ok``            -- a Rust-primary tool ran with no Python on PATH;
* ``needs-python``  -- a Python-primary tool could not run (expected today);
* ``FAIL``          -- a Rust-primary tool did not run, or a Python-primary
  tool *did* run (the PATH scrub leaked an interpreter).

Exit status is 1 on any FAIL and 0 otherwise, so the meter doubles as a
regression gate: the day a Rust tool secretly grows a Python dependency, this
goes red. The agentctl subcommands the editions model records as
python-primary are listed as ``needs-python`` without being executed (one of
them starts a server). This is the campaign's progress meter
(#542 rust-primary-rust-only-check); run it in a container from
`validation/rust-only/` for the hermetic version.

Usage:
    python3 scripts/check_rust_only.py
    python3 scripts/check_rust_only.py --self-test
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PROBE_ARGS = ("--help",)


def is_python_name(name: str) -> bool:
    """Interpreter names the scrub removes: python, python3, python3.12, ..."""
    if name == "python":
        return True
    return name.startswith("python") and (name == "python3" or name[6:].replace(".", "").isdigit() or name.startswith("python3."))


def scrubbed_path(source_path: str, shim_dir: Path) -> str:
    """A PATH of symlinks to everything on `source_path` except interpreters."""
    seen: set[str] = set()
    for directory in source_path.split(os.pathsep):
        if not directory:
            continue
        try:
            entries = sorted(Path(directory).iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.name in seen or is_python_name(entry.name):
                continue
            seen.add(entry.name)
            target = shim_dir / entry.name
            try:
                if entry.is_symlink() or entry.is_file():
                    target.symlink_to(entry.resolve() if entry.is_symlink() else entry)
            except OSError:
                continue
    return str(shim_dir)


def editions_rows(tsv_text: str) -> list[tuple[str, str, str]]:
    rows: list[tuple[str, str, str]] = []
    for line in tsv_text.splitlines():
        parts = line.split("\t")
        if len(parts) >= 3:
            rows.append((parts[0], parts[1], parts[2]))
    return rows


def probe_tools(root: Path, *, environ: Mapping[str, str]) -> tuple[list[str], list[str]]:
    """Run the meter against `root`; return (report lines, failures)."""
    rows = editions_rows((root / "validation" / "editions.tsv").read_text(encoding="utf-8"))
    dispatched = {
        path.name
        for path in (root / "common" / "bin").iterdir()
        if path.is_symlink() and path.name != "engine-resolver"
    }
    lines: list[str] = []
    failures: list[str] = []
    ok = 0
    for name, primary, _state in rows:
        if name not in dispatched:
            continue
        result = subprocess.run(
            [str(root / "common" / "bin" / name), *PROBE_ARGS],
            cwd=root,
            capture_output=True,
            text=True,
            env=dict(environ),
            check=False,
            timeout=600,
        )
        if primary == "rust":
            if result.returncode == 0:
                ok += 1
                lines.append(f"ok {name}")
            else:
                failures.append(f"FAIL {name}: Rust-primary tool exited {result.returncode} with no Python on PATH")
                lines.append(failures[-1])
        else:
            if result.returncode == 0:
                failures.append(f"FAIL {name}: Python-primary tool ran with no Python on PATH (interpreter leak in the scrub?)")
                lines.append(failures[-1])
            else:
                lines.append(f"needs-python {name}")
    document: object = json.loads((root / "validation" / "editions.json").read_text(encoding="utf-8"))
    if isinstance(document, dict):
        tools = document.get("tools")
        if isinstance(tools, list):
            for tool in tools:
                if not isinstance(tool, dict):
                    continue
                subcommands = tool.get("subcommands")
                if not isinstance(subcommands, list):
                    continue
                for item in subcommands:
                    if isinstance(item, dict) and isinstance(item.get("name"), str):
                        lines.append(f"needs-python {tool.get('name')} {item['name']} (subcommand)")
    total = sum(1 for name, _p, _s in rows if name in dispatched)
    rust_primary = sum(1 for name, primary, _s in rows if name in dispatched and primary == "rust")
    lines.append(
        f"rust-only meter: {ok}/{rust_primary} Rust-primary dispatched tools run with no Python on PATH; "
        f"{total - rust_primary} dispatched tools still need Python"
    )
    return lines, failures


def run_check(root: Path = REPO_ROOT) -> int:
    with tempfile.TemporaryDirectory(prefix="agent-utils-rust-only-") as directory:
        shim = Path(directory) / "bin"
        shim.mkdir()
        environ = {
            key: value
            for key, value in os.environ.items()
            if key not in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE", "CI_HUB_PYTHON")
        }
        environ["PATH"] = scrubbed_path(os.environ.get("PATH", ""), shim)
        lines, failures = probe_tools(root, environ=environ)
    print("\n".join(lines))
    if failures:
        print(f"check-rust-only: {len(failures)} failure(s)", file=sys.stderr)
        return 1
    print("check-rust-only: OK")
    return 0


def _self_test() -> int:
    failures: list[str] = []
    if not is_python_name("python3.12") or is_python_name("pythonic") or not is_python_name("python"):
        failures.append("is_python_name misclassifies interpreter names")
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory) / "au"
        (root / "common" / "bin").mkdir(parents=True)
        (root / "py" / "bin").mkdir(parents=True)
        (root / "rs" / "bin").mkdir(parents=True)
        (root / "validation").mkdir(parents=True)
        resolver = REPO_ROOT / "common" / "bin" / "engine-resolver"
        shutil.copy2(resolver, root / "common" / "bin" / "engine-resolver")
        (root / "common" / "bin" / "engine-resolver").chmod(0o755)
        for tool in ("tick-hub", "wrkslots"):
            (root / "common" / "bin" / tool).symlink_to("engine-resolver")
            (root / "py" / "bin" / tool).write_text("print('py')\n")
            (root / "py" / "bin" / tool).chmod(0o755)
        rust_stub = root / "rs" / "bin" / "tick-hub"
        rust_stub.write_text("#!/bin/sh\necho rs\n")
        rust_stub.chmod(0o755)
        (root / "validation" / "editions.tsv").write_text(
            "tick-hub\trust\tparity\t\nwrkslots\tpython\tabsent\t\n"
        )
        (root / "validation" / "editions.json").write_text(
            json.dumps({"tools": [{"name": "agentctl", "subcommands": [{"name": "mcp"}]}]})
        )
        with tempfile.TemporaryDirectory() as shim_dir:
            shim = Path(shim_dir)
            environ = dict(os.environ)
            environ["PATH"] = scrubbed_path(os.environ.get("PATH", ""), shim)
            if (shim / "python3").exists():
                failures.append("scrubbed PATH still contains python3")
            lines, found = probe_tools(root, environ=environ)
        if found:
            failures.append(f"sandbox meter: expected no failures, got {found}")
        if "ok tick-hub" not in lines or "needs-python wrkslots" not in lines:
            failures.append(f"sandbox meter: unexpected lines {lines}")
        if not any("agentctl mcp (subcommand)" in line for line in lines):
            failures.append(f"sandbox meter: subcommand line missing from {lines}")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        print(f"\n{len(failures)} self-test failure(s)", file=sys.stderr)
        return 1
    print("check-rust-only --self-test: PASSED (3 controls)")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="check the meter offline, then exit")
    args = parser.parse_args(argv)
    if args.self_test:
        return _self_test()
    return run_check()


if __name__ == "__main__":
    raise SystemExit(main())
