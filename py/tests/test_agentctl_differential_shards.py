"""The agentctl differential shards partition its groups exactly once."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "cross"))

from agentctl_differential import shard_groups  # noqa: E402


def test_three_shards_cover_every_group_exactly_once() -> None:
    full = [name for name, _ in shard_groups(0, 1)]
    sharded = [name for shard in range(3) for name, _ in shard_groups(shard, 3)]
    assert sorted(sharded) == sorted(full)
    assert len(sharded) == len(full) == 27


def test_an_out_of_range_shard_is_refused() -> None:
    try:
        shard_groups(3, 3)
    except ValueError:
        return
    raise AssertionError("shard 3/3 was accepted")
