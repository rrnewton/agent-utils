"""Every vendored gh_resolve.py is an exact copy of the canonical resolver.

Each Python tool is published as an independent distribution that may not
import a sibling, so the tools that call GitHub each carry a copy of
common/gh_resolve/gh_resolve.py. This test is what keeps them one resolver.
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPOSITORY = Path(__file__).resolve().parents[2]
CANONICAL = REPOSITORY / "common" / "gh_resolve" / "gh_resolve.py"
COPIES = ("wrkviz", "dagrun", "pr_landing_planner")


@pytest.mark.parametrize("package", COPIES)
def test_the_vendored_copy_matches_the_canonical_resolver(package: str) -> None:
    copy = REPOSITORY / "py" / package / "gh_resolve.py"

    assert copy.read_bytes() == CANONICAL.read_bytes(), (
        f"{copy.relative_to(REPOSITORY)} differs from {CANONICAL.relative_to(REPOSITORY)}; "
        "edit the canonical file and copy it to every package"
    )


def test_no_other_package_carries_a_stale_copy() -> None:
    found = {path.parent.name for path in (REPOSITORY / "py").glob("*/gh_resolve.py")}

    assert found == set(COPIES)
