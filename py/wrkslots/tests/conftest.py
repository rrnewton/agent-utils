"""Source-checkout test configuration for the wrkslots distribution."""

from __future__ import annotations

import sys
from collections.abc import Sequence
from pathlib import Path


PY_ROOT = Path(__file__).resolve().parents[2]
if str(PY_ROOT) not in sys.path:
    sys.path.insert(0, str(PY_ROOT))

# The suite predates disk-image slots and exercises the plain-worktree
# representation; image slots have their own tests (test_slot_images.py and
# e2e_images.sh). New projects created by these tests therefore default to
# worktrees. Subprocesses inherit this.
import os  # noqa: E402

os.environ.setdefault("WRKSLOTS_INIT_REPRESENTATION", "worktree")

import pytest  # noqa: E402

# After each test, unmount any slot image it left mounted in its tmp_path
# (#1064 test-temp-and-mount-leaks).
from wrkslots.tests.image_teardown import pytest_runtest_teardown  # noqa: E402,F401


@pytest.fixture(autouse=True)
def idle_validation_run_host(
    request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Give each in-process test a host on which no validation run exists.

    A validation row's liveness is answered from this host's live processes,
    user-systemd units and control groups, after proving that this process
    sees the whole host.  Lifecycle shards run in their own user and PID
    namespaces and on hosts with no user bus, where that evidence is
    unreadable or the proof fails, and on a busy host the real unit
    population changes under the enumeration.  Neither is what those tests
    examine.  So this replaces the authority's host evidence, the retained
    unit control-group read, and the host-view proof that recovery also
    makes.  Recovery still reads its own process table and units through
    the functions those tests stub.  Tests that examine the evidence itself
    carry the ``validation_run_evidence`` marker and supply it themselves.
    Child processes get the same host from ``idle_validation_host``.
    """

    if request.node.get_closest_marker("validation_run_evidence") is not None:
        return
    from wrkslots import cli
    from wrkslots.tests.idle_validation_host import (
        no_retained_cgroup_members,
        no_validation_runs,
        whole_host_process_view,
    )

    monkeypatch.setattr(cli, "_validation_run_host_evidence", no_validation_runs)
    monkeypatch.setattr(cli, "_retained_unit_cgroup_members", no_retained_cgroup_members)
    monkeypatch.setattr(cli, "_assert_host_process_view", whole_host_process_view)


def _pinned_census_users(paths: list[Path]) -> list[str]:
    """Processes whose cwd, root, executable or descriptors lie under ``paths``.

    One pass over /proc that skips entries vanishing mid-read, the way the Rust
    edition's audit snapshot reads it, so host churn cannot make it refuse.
    """

    targets = [str(path) for path in paths]
    users: list[str] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        links = [entry / "cwd", entry / "root", entry / "exe"]
        try:
            links.extend((entry / "fd").iterdir())
        except OSError:
            pass
        for link in links:
            try:
                target = os.readlink(link)
            except OSError:
                continue
            if any(target == item or target.startswith(item + "/") for item in targets):
                users.append(f"PID {entry.name} {link.name} -> {target}")
    return users


@pytest.fixture
def pinned_process_evidence(
    request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Pin the host-wide process/path census for tests of registry judgment (#1084, #1091).

    The real census proves that no process on the whole host uses a slot path:
    three attempts, each discarded when any process's /proc entries change, in
    one 60-second budget.  On a loaded host (gate load averages of 100-235) it
    refuses, fail-closed, and every row that consulted it becomes UNKNOWN, so
    tests about registry verdicts failed on whatever the host was doing.  Pinned,
    the census answers "no process uses these paths" after checking that this is
    true with one churn-tolerant pass, and fails the test loudly if it is not:
    a test whose fixture really runs something in a slot needs the real census.
    Tests of process or liveness evidence carry the ``real_process_evidence``
    marker and keep it.
    """

    if request.node.get_closest_marker("real_process_evidence") is not None:
        return
    from wrkslots import cli

    def census(slot_paths: Sequence[Path], **_options: object) -> cli._ProcessPathCensus:
        users = _pinned_census_users(list(slot_paths))
        if users:
            pytest.fail(
                "pinned process census: a process uses a slot path, so this test depends on "
                "real process evidence; mark it real_process_evidence: " + "; ".join(users),
                pytrace=False,
            )
        return cli._ProcessPathCensus((), (), owner_cgroup_complete=True)

    monkeypatch.setattr(cli, "_capture_process_path_census", census)
