"""Deterministic process-liveness checks for timeout teardown."""

from dagrun.teardown import _live_process_group_from_stat


def test_proc_stat_parser_excludes_zombies_from_the_term_grace() -> None:
    """An unreaped cooperative child must not be charged the full SIGTERM grace."""
    assert _live_process_group_from_stat("123 (worker ) with parens) Z 1 777 0 0") is None
    assert _live_process_group_from_stat("456 (worker ) with parens) S 1 888 0 0") == 888
    assert _live_process_group_from_stat("malformed") is None
    assert _live_process_group_from_stat("1 (x) S 0 nope") is None
