"""The browser and benchmark targets refuse to start without their installed dependency.

`npx playwright test` in a directory with no `node_modules/` does not fail: it asks the npm
registry for a `playwright` to download, and from a host that cannot reach the registry that held
a validation run for 481 seconds before it failed anyway.  `py/tests/js/browser/check-deps.cjs`
now refuses first, and the targets run the installed `playwright`, never `npx`.

CI installs the packages before it validates, so nothing there would notice the guard being
removed or `npx` coming back.  These tests build their own copies of the directory instead, with
and without the packages, so they do not depend on what the host has installed.  The targets run
for real under a stand-in `dagrun` that records every box and runs only the dependency check,
with stand-in `npx` and `npm` first on PATH that record any call; so no test here can reach the
network either.

Like the other Node suites, these skip on a host without Node, and CI is where they bind.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


_REPO = Path(__file__).resolve().parents[2]
_BROWSER = _REPO / "py" / "tests" / "js" / "browser"
_WRKVIZ = _REPO / "py" / "wrkviz"
_PACKAGE = "@playwright/test"
_PINNED: str = json.loads((_BROWSER / "package.json").read_text())["devDependencies"][_PACKAGE]
_OFFLINE_INSTALL = "npm install --offline --no-audit --no-fund --no-package-lock"
# The pinned version with its last component one higher: differs only where a loose check would not.
_NEAR_MISS = ".".join([*_PINNED.split(".")[:-1], str(int(_PINNED.split(".")[-1]) + 1)])

# Records its argv as one JSON line, then runs the command after `--` only for the dependency
# check's box.  Every other box is only recorded, and succeeds, so a target that skips the check
# goes on to "run" its suite and exits 0, which the refusal tests reject.
_FAKE_DAGRUN = """\
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_DAGRUN_LOG"], "a") as log:
    log.write(json.dumps({"cwd": os.getcwd(), "args": args}) + "\\n")
label = args[args.index("--label") + 1]
if label == "timeline-browser-deps":
    command = args[args.index("--") + 1 :]
    os.execvp(command[0], command)
"""

# Stand-ins for the commands that could reach the registry: each call is recorded and fails.
_FAKE_FETCHER = """\
import os, sys
with open(os.environ["FAKE_FETCH_LOG"], "a") as log:
    log.write(os.path.basename(sys.argv[0]) + " " + " ".join(sys.argv[1:]) + "\\n")
sys.exit(99)
"""


def _node() -> str:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not installed; the dependency check cannot run here")
    return node


def _browser_copy(root: Path, installed: str | None, runner: bool) -> Path:
    """The check and its manifest, optionally with a fake install of `installed`."""

    browser = root / "browser"
    browser.mkdir()
    for name in ("check-deps.cjs", "package.json"):
        shutil.copy2(_BROWSER / name, browser / name)
    (browser / "benchmark-report.txt").write_text("")  # The benchmark target prints it.
    if installed is not None:
        package = browser / "node_modules" / "@playwright" / "test"
        package.mkdir(parents=True)
        (package / "package.json").write_text(json.dumps({"name": _PACKAGE, "version": installed}))
    if runner:
        bin_directory = browser / "node_modules" / ".bin"
        bin_directory.mkdir(parents=True, exist_ok=True)
        (bin_directory / "playwright").write_text("#!/bin/sh\nexit 0\n")
        (bin_directory / "playwright").chmod(0o755)
    return browser


def _check(browser: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_node(), "check-deps.cjs"],
        cwd=browser,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )


def test_the_check_refuses_when_nothing_is_installed(tmp_path: Path) -> None:
    result = _check(_browser_copy(tmp_path, installed=None, runner=False))

    assert result.returncode == 2, result.stdout + result.stderr
    assert result.stdout == ""
    assert f"{_PACKAGE} is not installed" in result.stderr
    assert "node_modules/.bin/playwright does not exist" in result.stderr
    assert _OFFLINE_INSTALL in result.stderr
    assert "does not name the version" not in result.stderr  # Nothing was read to name one.


@pytest.mark.parametrize(
    "version", ("0.0.1", _NEAR_MISS, f"{_PINNED}-beta"), ids=("other", "near-miss", "prerelease")
)
def test_the_check_refuses_a_version_other_than_the_pinned_one(
    tmp_path: Path, version: str
) -> None:
    assert version != _PINNED
    result = _check(_browser_copy(tmp_path, installed=version, runner=True))

    assert result.returncode == 2, result.stdout + result.stderr
    assert f"{_PACKAGE} {version} is installed; package.json pins {_PINNED}" in result.stderr
    assert "does not exist" not in result.stderr


@pytest.mark.parametrize("version", (None, "absent", 1), ids=("null", "absent", "number"))
def test_the_check_refuses_an_installed_manifest_that_names_no_version(
    tmp_path: Path, version: str | int | None
) -> None:
    browser = _browser_copy(tmp_path, installed=_PINNED, runner=True)
    manifest: dict[str, object] = {"name": _PACKAGE}
    if version != "absent":
        manifest["version"] = version
    (browser / "node_modules" / "@playwright" / "test" / "package.json").write_text(
        json.dumps(manifest)
    )

    result = _check(browser)

    assert result.returncode == 2, result.stdout + result.stderr
    assert result.stdout == ""
    assert "node_modules/@playwright/test/package.json does not name the version" in result.stderr
    assert "is not installed" not in result.stderr


def test_the_check_refuses_an_install_without_its_runner(tmp_path: Path) -> None:
    result = _check(_browser_copy(tmp_path, installed=_PINNED, runner=False))

    assert result.returncode == 2, result.stdout + result.stderr
    assert "node_modules/.bin/playwright does not exist" in result.stderr
    assert "is installed; package.json pins" not in result.stderr


@pytest.mark.parametrize("section", (None, "dependencies"), ids=("no-section", "wrong-section"))
def test_the_check_refuses_a_manifest_that_does_not_pin_the_package(
    tmp_path: Path, section: str | None
) -> None:
    browser = _browser_copy(tmp_path, installed=_PINNED, runner=True)
    manifest = json.loads((browser / "package.json").read_text())
    pins = manifest.pop("devDependencies")
    if section is not None:
        manifest[section] = pins
    (browser / "package.json").write_text(json.dumps(manifest))

    result = _check(browser)

    assert result.returncode == 2, result.stdout + result.stderr
    assert f"package.json does not pin {_PACKAGE} in devDependencies" in result.stderr
    assert "is installed; package.json pins" not in result.stderr


def test_the_check_refuses_an_installed_manifest_it_cannot_read(tmp_path: Path) -> None:
    browser = _browser_copy(tmp_path, installed=_PINNED, runner=True)
    (browser / "node_modules" / "@playwright" / "test" / "package.json").write_text("{")

    result = _check(browser)

    assert result.returncode == 2, result.stdout + result.stderr
    assert "node_modules/@playwright/test/package.json cannot be read" in result.stderr
    assert "is not installed" not in result.stderr
    assert "does not name the version" not in result.stderr


def test_the_check_passes_the_pinned_install(tmp_path: Path) -> None:
    result = _check(_browser_copy(tmp_path, installed=_PINNED, runner=True))

    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout == f"wrkviz browser dependencies: {_PACKAGE} {_PINNED}\n"


def _make(
    tmp_path: Path, target: str, browser: Path
) -> tuple[subprocess.CompletedProcess[str], list[list[str]], list[str]]:
    """Run a wrkviz target on `browser`; return the result, the boxes and any fetcher calls."""

    node = _node()
    tools = tmp_path / "tools"
    tools.mkdir()
    for name, source in (("dagrun", _FAKE_DAGRUN), ("npx", _FAKE_FETCHER), ("npm", _FAKE_FETCHER)):
        (tools / name).write_text(f"#!{sys.executable}\n{source}")
        (tools / name).chmod(0o755)
    boxes = tmp_path / "boxes.jsonl"
    fetches = tmp_path / "fetches.txt"
    environment = {
        **os.environ,
        "PATH": f"{tools}{os.pathsep}{Path(node).parent}{os.pathsep}{os.environ.get('PATH', '')}",
        "FAKE_DAGRUN_LOG": str(boxes),
        "FAKE_FETCH_LOG": str(fetches),
    }
    environment.pop("MAKEFLAGS", None)
    result = subprocess.run(
        # BOX_FLAGS is pinned empty because CI sets it, and every box's argv is compared exactly.
        [
            "make", "-s", "-C", str(_WRKVIZ), target,
            f"BROWSER={browser}", f"DAGRUN={tools / 'dagrun'}", "BOX_FLAGS=",
        ],  # fmt: skip
        env=environment,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    recorded = [
        json.loads(line)["args"] for line in boxes.read_text().splitlines()
    ] if boxes.exists() else []
    fetched = fetches.read_text().splitlines() if fetches.exists() else []
    return result, recorded, fetched


_DEPS_BOX = [
    "box", "--label", "timeline-browser-deps", "--mem", "256M", "--timeout", "60", "--cores", "1",
    "--", "node", "check-deps.cjs",
]  # fmt: skip


@pytest.mark.parametrize("target", ("browser", "benchmark"))
def test_a_target_stops_at_the_check_without_fetching_anything(tmp_path: Path, target: str) -> None:
    browser = _browser_copy(tmp_path, installed=None, runner=False)

    result, boxes, fetched = _make(tmp_path, target, browser)

    assert result.returncode == 2, result.stdout + result.stderr
    assert boxes == [_DEPS_BOX]  # The check ran first, and nothing ran after it.
    assert fetched == []
    assert f"{_PACKAGE} is not installed" in result.stderr
    assert _OFFLINE_INSTALL in result.stderr


def test_the_browser_target_runs_the_installed_playwright_after_the_check(tmp_path: Path) -> None:
    browser = _browser_copy(tmp_path, installed=_PINNED, runner=True)

    result, boxes, fetched = _make(tmp_path, "browser", browser)

    assert result.returncode == 0, result.stdout + result.stderr
    assert boxes == [
        _DEPS_BOX,
        [
            "box", "--label", "timeline-browser", "--mem", "6G", "--timeout", "900", "--cores", "4",
            "--", "./node_modules/.bin/playwright", "test", "--reporter=line",
        ],
    ]  # fmt: skip
    assert fetched == []
    assert not any("npx" in word for box in boxes for word in box)


def test_the_benchmark_target_runs_its_suite_after_the_check(tmp_path: Path) -> None:
    browser = _browser_copy(tmp_path, installed=_PINNED, runner=True)

    result, boxes, fetched = _make(tmp_path, "benchmark", browser)

    assert result.returncode == 0, result.stdout + result.stderr
    assert boxes == [
        _DEPS_BOX,
        [
            "box", "--label", "timeline-benchmark", "--mem", "6G", "--timeout", "900", "--cores", "4",
            "--", "node", "--test", "benchmark-budget.test.cjs",
        ],
    ]  # fmt: skip
    assert fetched == []
