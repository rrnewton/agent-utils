"""Same-UID census under SHORT-LIVED unrelated churn (consumer red controls).

Vendored from the consumer's dh-ceywa3 diagnosis (2026-11-22): on a busy
host, same-UID processes start at 27-50/s and nearly all exit within
0.3 s, so a newborn is typically LISTED by one inventory and GONE before
its own scan. The existing churn test in test_census_late_fork.py lets
each newborn live about two listings, so it never exercises that case.

Two controls:
- Characterization: the same-UID census still refuses under
  listed-then-gone churn (convergence for validate remove comes from
  the lineage census in the #2081 stack, which replaces this census on
  that path; this census remains for the paths that still use it).
- Holder naming: a holder found in round 1 is named (its PID) in the
  refusal or matches, never reported only as "did not close". The
  distinct holder-found shape and exit status are a separate slice.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_census_late_fork import _Host, _census, _scripted, slot  # noqa: F401


def _listed_then_gone_churn(host: _Host, listings: int) -> None:
    """After listing k: the newborn that listing k showed exits before its
    scan, and the next newborn starts (unrelated: it holds nothing)."""

    newborns: list[tuple[int, int]] = []

    def step() -> None:
        if newborns:
            host.exit(newborns[-1])
        newborn = (5000 + len(newborns), 90 + len(newborns))
        newborns.append(newborn)
        host.spawn(newborn)

    # Listing 1 shows only the bystander; from listing 2 on, every listing
    # shows one newborn that is gone before its scan.
    host.after_inventory[1] = step
    for listing in range(2, listings + 1):
        host.after_inventory[listing] = step


def test_listed_then_gone_unrelated_churn_still_refuses(
    monkeypatch: pytest.MonkeyPatch, slot: Path,  # noqa: F811
) -> None:
    """The same-UID census refuses after 8 followed rounds under
    listed-then-gone churn, whatever its time bound (the RUN 57/58
    refusal shape). The wording no longer claims the 8 rounds were
    consecutive (#2082 R3)."""

    host = _Host(slot, (100, 7))
    _listed_then_gone_churn(host, 40)
    _scripted(monkeypatch, host)
    with pytest.raises(cli.Refusal) as caught:
        _census(host)
    text = str(caught.value)
    assert "same-UID process census did not close" in text
    assert "(not necessarily consecutive)" in text
    assert len(host.inventories) == 1 + cli._SAME_UID_CENSUS_INVENTORIES


def test_a_found_holder_is_never_reported_as_census_did_not_close(
    monkeypatch: pytest.MonkeyPatch, slot: Path,  # noqa: F811
) -> None:
    """A holder found in round 1, then short-lived unrelated churn: the
    census reports the holder (in its matches, or a refusal naming its
    PID), never a bare "did not close" with no PID. The distinct
    holder-found shape and exit status are a separate #2082 slice."""

    holder = (100, 7)
    host = _Host(slot, holder, holders=[holder])
    _listed_then_gone_churn(host, 40)
    _scripted(monkeypatch, host)
    try:
        census = _census(host)
    except cli.Refusal as exc:
        assert str(holder[0]) in str(exc), f"holder {holder[0]} not named: {exc}"
    else:
        assert {pid for pid, *_ in census.matches} == {holder[0]}
