"""Tests for the ``cpuset-alloc`` CLI layer.

The stateful ledger itself is covered by ``test_reservation.py``. Here we cover the
allocator CLI:

  * ``_scope_argv`` builds the mutation-verified HARD-pin invocation
    (``systemd-run --user --scope -p AllowedCPUs=<set>``), NOT an escapable
    ``sched_setaffinity`` call.
  * ``run`` REFUSES (does not silently run un-pinned) when the hard mechanism is
    unavailable — a soft bound would silently contaminate a benchmark.
  * ``status`` / ``reclaim`` emit the qualified assignment as JSON.
  * A systemd-gated LIVE MUTATION test: request K cores, confirm a child CANNOT
    escape to a K+1th core (reading back what we wrote proves nothing).
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from collections.abc import Mapping
from contextlib import nullcontext
from pathlib import Path

import pytest

from dagrun import cpuset_allocator as ca
from dagrun import reservation


def _systemd() -> bool:
    return ca._systemd_run_available()


# --------------------------------------------------------------------------- #
# argv construction — the HARD mechanism, not sched_setaffinity                #
# --------------------------------------------------------------------------- #

def test_scope_argv_uses_allowedcpus_hard_pin() -> None:
    argv = ca._scope_argv([7, 3, 5], ["echo", "hi"])
    assert argv[0] == "systemd-run"
    assert "--user" in argv and "--scope" in argv and "--collect" in argv
    # Cores rendered sorted as a cpulist on AllowedCPUs — the cgroup cpuset path.
    assert "AllowedCPUs=3,5,7" in argv
    # The command follows a `--` separator, uninterpreted by systemd-run.
    dd = argv.index("--")
    assert argv[dd + 1 :] == ["echo", "hi"]
    # It must NOT reach for the escapable affinity mechanism.
    assert not any("setaffinity" in a or a == "taskset" for a in argv)


#: Arguments systemd-run would rewrite if it expanded the environment (#2316).
_EXPANSION_PAYLOAD = ["A:${PROBE}", "B:$$", "C:${x%% *}", "$PROBE", "D:$PROBE"]


def test_scope_argv_turns_expansion_off_before_the_separator() -> None:
    command = ["sh", "-c", "echo ${HOME} $$"]
    argv = ca._scope_argv([3, 1], command, tag="t", no_expand=["--expand-environment=no"])
    dd = argv.index("--")
    assert argv[dd - 1] == "--expand-environment=no"
    assert argv[dd + 1 :] == command
    # A systemd-run that refuses the option gets the old argv, unchanged.
    old = ca._scope_argv([3, 1], command, tag="t", no_expand=[])
    assert not any(a.startswith("--expand-environment") for a in old)
    assert len(old) + 1 == len(argv)


@pytest.mark.parametrize(("answer", "flag"), [(True, True), (False, False), (None, False)])
def test_scope_argv_default_asks_systemd_run_once(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    answer: bool | None,
    flag: bool,
) -> None:
    from dagrun import systemd_run

    asked: list[str] = []

    def probe(program: str) -> bool | None:
        asked.append(program)
        return answer

    monkeypatch.setattr(systemd_run, "probe_no_expand", probe)
    monkeypatch.setattr(systemd_run, "_SUPPORTED", None)
    for _ in range(2):
        argv = ca._scope_argv([0], ["true"])
        assert ("--expand-environment=no" in argv) is flag
    assert asked == ["systemd-run"]
    err = capsys.readouterr().err
    # Only a refusal is worth a note; a missing binary fails at launch and says so there.
    assert ("refuses --expand-environment=no" in err) is (answer is False)


def test_no_expand_probe_reports_support_refusal_and_absence(tmp_path: Path) -> None:
    from dagrun import systemd_run

    new = tmp_path / "new-systemd-run"
    new.write_text(
        '#!/bin/sh\n[ "$1" = --expand-environment=no ] && [ "$2" = --version ] && exit 0\nexit 9\n'
    )
    old = tmp_path / "old-systemd-run"
    old.write_text("#!/bin/sh\necho \"systemd-run: unrecognized option '$1'\" >&2\nexit 1\n")
    for path in (new, old):
        path.chmod(0o755)
    assert systemd_run.probe_no_expand(str(new)) is True
    assert systemd_run.probe_no_expand(str(old)) is False
    assert systemd_run.probe_no_expand(str(tmp_path / "missing")) is None
    note = systemd_run.unsupported_note("cpuset-alloc:")
    assert note.startswith("cpuset-alloc: note: ") and "${...}" in note and "#2316" in note


def test_scope_argv_passes_dollar_text_literally_live(
    inherited_runner_authority_env: Mapping[str, str],
) -> None:
    """Live: the exact argv ``cpuset-alloc run`` launches gets its arguments byte for byte, even
    with a caller variable of the same name set."""
    if inherited_runner_authority_env:
        pytest.skip(
            "live systemd scope would escape the parent-owned delegated validation cgroup "
            "(#2316 live probe)"
        )
    if not shutil.which("systemd-run") or not _systemd():
        pytest.skip("no usable systemd user scope here")
    core = min(os.sched_getaffinity(0))
    env = {k: v for k, v in os.environ.items() if k != "x"}
    env["PROBE"] = "expanded"
    proc = subprocess.run(
        ca._scope_argv([core], ["printf", "%s\\n", *_EXPANSION_PAYLOAD]),
        capture_output=True,
        text=True,
        env=env,
        stdin=subprocess.DEVNULL,
        timeout=60,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr
    assert proc.stdout.splitlines() == _EXPANSION_PAYLOAD, proc.stderr


def test_cpulist_is_sorted_comma_form() -> None:
    assert ca._cpulist([5, 1, 3]) == "1,3,5"
    assert ca._cpulist([2]) == "2"


# --------------------------------------------------------------------------- #
# run REFUSES rather than silently running un-pinned                          #
# --------------------------------------------------------------------------- #

def test_run_refuses_without_hard_mechanism(monkeypatch: pytest.MonkeyPatch) -> None:
    """No systemd user scope => no HARD pin. `run` must refuse (exit 3), never
    fall back to an escapable bound and silently contaminate the benchmark."""
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: False)

    # If the guard ever let execution through, this sentinel would fire.
    def _no_run(*a: object, **k: object) -> subprocess.CompletedProcess[bytes]:
        raise AssertionError("run must not launch a command when the pin is unavailable")

    monkeypatch.setattr(subprocess, "run", _no_run)
    rc = ca.main(["run", "--cores", "1", "--", "echo", "hi"])
    assert rc == 3


def test_run_rejects_empty_command(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    assert ca.main(["run", "--cores", "1"]) == 2


def test_run_requires_command_separator(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    assert ca.main(["run", "--cores", "1", "true"]) == 2


@pytest.mark.parametrize(
    "argv",
    [
        ["run", "--cores", "0", "--", "true"],
        ["run", "--cores", "1", "--sample-s", "-1", "--", "true"],
        ["run", "--cores", "1", "--sample-s", "nan", "--", "true"],
        ["run", "--cores", "1", "--sample-s", "0", "--max-irq-rate", "1", "--", "true"],
        ["selftest", "--sample-s", "0", "--max-irq-rate", "1"],
        ["selftest", "--tag", "not-supported"],
    ],
)
def test_invalid_options_are_clean_usage_errors(argv: list[str]) -> None:
    with pytest.raises(SystemExit) as raised:
        ca.main(argv)
    assert raised.value.code == 2


def test_run_refuses_when_allowed_cpus_is_soft_or_inert(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    monkeypatch.setattr(
        reservation,
        "reserve_cores",
        lambda *args, **kwargs: nullcontext([0]),
    )
    monkeypatch.setattr(
        ca,
        "_probe_hard_pin",
        lambda cores: {"verdict": "SOFT_OR_INERT", "cores": list(cores)},
    )

    def _no_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[bytes]:
        raise AssertionError("the benchmark command must not run after a soft pin probe")

    monkeypatch.setattr(subprocess, "run", _no_run)
    assert ca.main(["run", "--cores", "1", "--", "echo", "hi"]) == 3


def test_probe_requires_real_mutation_and_every_reserved_core() -> None:
    base: dict[str, object] = {
        "child_allowed_before": "2-3",
        "child_allowed_after": "2-3",
        "mutation_attempted": True,
        "mutation_blocked": True,
        "positive_cores_usable": [2, 3],
        "restore_exact": True,
    }
    assert ca._evaluate_probe([2, 3], 4, base)["verdict"] == "HARD"
    no_mutation = {**base, "mutation_attempted": False, "mutation_blocked": False}
    assert ca._evaluate_probe([2, 3], 4, no_mutation)["verdict"] == "SOFT_OR_INERT"
    partial = {**base, "positive_cores_usable": [2]}
    assert ca._evaluate_probe([2, 3], 4, partial)["verdict"] == "SOFT_OR_INERT"


def test_wrapped_signal_uses_shell_status() -> None:
    assert ca._wrapped_returncode(-15) == 143
    assert ca._wrapped_returncode(42) == 42


def test_selftest_forwards_the_stated_irq_budget(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: dict[str, object] = {}

    def reserve(*args: object, **kwargs: object) -> nullcontext[list[int]]:
        seen.update(kwargs)
        return nullcontext([0])

    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    monkeypatch.setattr(reservation, "reserve_cores", reserve)
    monkeypatch.setattr(ca, "_probe_hard_pin", lambda _cores: {"verdict": "HARD"})

    assert ca.main(["selftest", "--cores", "1", "--max-irq-rate", "7.5"]) == 0
    assert seen["max_irq_rate"] == 7.5


def test_missing_wrapped_executable_is_clean_operational_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    assert ca._run_reserved_hard([0], ["/definitely/missing/cpuset-command"]) == 3
    monkeypatch.setattr(ca, "_systemd_run_available", lambda: True)
    monkeypatch.setattr(ca, "_probe_hard_pin", lambda cores: {"verdict": "HARD"})
    monkeypatch.setattr(
        ca,
        "_scope_argv",
        lambda cores, cmd, tag="": ["/definitely/missing/cpuset-command"],
    )
    assert ca._run_reserved_hard([0], ["true"]) == 3


# --------------------------------------------------------------------------- #
# status / reclaim JSON over an isolated ledger                                #
# --------------------------------------------------------------------------- #

def test_status_and_reclaim_json(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    ledger = tmp_path / "ledger.json"
    rc = ca.main(["status", "--ledger", str(ledger)])
    assert rc == 0
    out = json.loads(capsys.readouterr().out)
    assert out == {"held_cores": [], "held_count": 0}

    rc = ca.main(["reclaim", "--ledger", str(ledger)])
    assert rc == 0
    out = json.loads(capsys.readouterr().out)
    assert out == {"reclaimed": [], "reclaimed_count": 0}


# --------------------------------------------------------------------------- #
# LIVE MUTATION — the owner's "confirm the tree cannot run on a K+1th core"    #
# --------------------------------------------------------------------------- #

@pytest.mark.skipif(not shutil.which("systemd-run"), reason="systemd-run not installed")
def test_selftest_verdict_is_hard_when_systemd_available(
    capsys: pytest.CaptureFixture[str],
    inherited_runner_authority_env: Mapping[str, str],
) -> None:
    """If a systemd user scope works here, the mutation self-test must conclude
    HARD: the child's escape to an excluded core is masked and the spinners stay
    inside the reserved set. If systemd is present but a user scope cannot be
    created (some CI images), the tool reports UNTESTABLE — accept that, don't
    assert a false HARD."""
    if inherited_runner_authority_env:
        pytest.skip(
            "live systemd scope would escape the parent-owned delegated validation cgroup; "
            "tracked by #28 delegated-cpuset"
        )
    k = 2 if len(os.sched_getaffinity(0)) >= 3 else 1
    rc = ca.main(["selftest", "--cores", str(k), "--sample-s", "0.05"])
    out = json.loads(capsys.readouterr().out)
    if out.get("verdict") != "HARD":
        assert rc != 0
        pytest.skip(f"host does not provide a HARD AllowedCPUs scope: {out}")
    assert out["verdict"] == "HARD", out
    assert rc == 0
    assert out["count"] == k
    assert len(out["cores"]) == k
    assert out["negative_escape_masked"] is True
    assert out["positive_stayed_in_set"] is True
