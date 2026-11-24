"""Gated TLC check of the wrkslots TLA+ specs (#2090).

Runs a small, fast config set with the checksum-pinned jar
when one is provisioned (TLA2TOOLS_JAR, else the per-host
default ~/.local/share/tla2tools/1.7.4/tla2tools.jar). When
no jar is present the whole module skips by name: the gate
then has NO model-check coverage of the wrkslots registry
(the situation on a host where the jar isn't provisioned).
Every wait is a subprocess
completion condition with a generous anti-hang ceiling;
verdicts are read from TLC's own output, never from timing.

Expected verdicts are the cross-checks themselves: the
pre-#2088 lock shape MUST starve, the fork and sidecar
configs MUST find their violations, and the fixed shapes
MUST pass.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
from pathlib import Path

import pytest

SPEC_DIR = Path(__file__).resolve().parents[1] / "spec"
PINNED_SHA256 = "936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
DEFAULT_JAR = Path.home() / ".local" / "share" / "tla2tools" / "1.7.4" / "tla2tools.jar"


def _jar() -> Path:
    jar = Path(os.environ.get("TLA2TOOLS_JAR") or DEFAULT_JAR)
    if not jar.is_file():
        pytest.skip(
            f"no TLC jar at {jar}: wrkslots model-check coverage "
            "absent on this host until the pinned jar is provisioned"
        )
    digest = hashlib.sha256(jar.read_bytes()).hexdigest()
    if digest != PINNED_SHA256:
        pytest.skip(
            f"TLC jar at {jar} has sha256 {digest}, not the pinned "
            f"{PINNED_SHA256}: refusing an unverified model checker"
        )
    return jar


def _java() -> str:
    found = shutil.which("java")
    if found:
        return found
    jre21 = Path("/usr/lib/jvm/jre-21/bin/java")
    if jre21.exists():
        return str(jre21)
    pytest.skip("no java on PATH or at /usr/lib/jvm/jre-21/bin/java for TLC")


def _run_tlc(tmp_path: Path, spec: str, config: str) -> str:
    jar = _jar()
    work = tmp_path / config
    work.mkdir()
    shutil.copy(SPEC_DIR / f"{spec}.tla", work / f"{spec}.tla")
    shutil.copy(SPEC_DIR / f"{config}.cfg", work / f"{config}.cfg")
    done = subprocess.run(
        [_java(), "-XX:+UseParallelGC", "-jar", str(jar),
         "-config", f"{config}.cfg", f"{spec}.tla"],
        cwd=work, text=True, capture_output=True,
        timeout=900, check=False,
    )
    return f"{done.stdout}\n{done.stderr}\nrc={done.returncode}"


def test_lock_pre_2088_starves(tmp_path: Path) -> None:
    out = _run_tlc(tmp_path, "WrkslotsLock", "WrkslotsLock-pre")
    assert "Temporal properties were violated" in out, out[-2000:]
    assert "Invariant Safety is violated" not in out, out[-2000:]


def test_lock_post_2088_passes(tmp_path: Path) -> None:
    out = _run_tlc(tmp_path, "WrkslotsLock", "WrkslotsLock-post")
    assert out.rstrip().endswith("rc=0"), out[-2000:]
    assert "violated" not in out, out[-2000:]


def test_transition_allfix_passes(tmp_path: Path) -> None:
    out = _run_tlc(tmp_path, "WrkslotsTransition", "WrkslotsTransition-allfix")
    assert out.rstrip().endswith("rc=0"), out[-2000:]


def test_transition_fork_found(tmp_path: Path) -> None:
    out = _run_tlc(tmp_path, "WrkslotsTransition", "WrkslotsTransition-fork")
    assert "Invariant NoViol is violated" in out, out[-2000:]


def test_transition_sidecar_found(tmp_path: Path) -> None:
    out = _run_tlc(tmp_path, "WrkslotsTransition", "WrkslotsTransition-sidecar")
    assert "Invariant ReloadEqualsMemory is violated" in out, out[-2000:]


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
