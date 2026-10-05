"""Run the wrkslots command on a host where no validation run exists.

A validation row's liveness is answered from this host's live processes and
user-systemd units.  Lifecycle shards run in user namespaces and on hosts with
no user bus, where that evidence is unreadable, and on a busy host the real
unit population changes under the enumeration.  Neither is what the lifecycle
tests examine, so they run against a host with no validation run: in process
through the autouse fixture in ``conftest.py``, and in child processes through
this entry point (``python -m wrkslots.tests.idle_validation_host ARGS``).
Retained run handles are project files and are still read for real.
"""

from __future__ import annotations

import sys
from collections.abc import Mapping

from wrkslots import cli


def no_validation_runs() -> tuple[
    tuple[cli._AbsentProcessObservation, ...], tuple[Mapping[str, str], ...]
]:
    """Host evidence with no live process and no user-systemd unit."""

    return (), ()


def main() -> int:
    cli._validation_run_host_evidence = no_validation_runs
    return cli.main(sys.argv[1:])


if __name__ == "__main__":
    raise SystemExit(main())
