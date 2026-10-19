---
title: 'python-minimum-failures: tests that fail on Python 3.10, the declared minimum'
status: open
priority: 2
issue_type: bug
labels:
- validation
created_at: 2026-10-19T09:35:56.223051728+00:00
updated_at: 2026-10-19T09:35:56.223051728+00:00
---

# Description

[opus 5.5] Tests that fail on Python 3.10, the declared minimum of every distribution. The repository-ci job that runs them was cancelled at its time limit on every run since 2026-10-10 (see python-minimum-timeout), so no run has listed them; the last cancelled run had printed about 70 failures, and the workflow comment cited a tracker entry that does not exist.

A local run of the whole `py` tree on CPython 3.10.21 (a relocatable standalone build), six family-preserving shards, 2026-10-19, failed 69 tests. Every one of them passes on 3.12 run the same way. By cause:
- 7 in `wrkslots/tests/test_cache_plan_budget.py` (`test_expansion_yields_what_path_glob_yields` x5, `test_joint_expansion_lists_each_directory_once`, `test_unreadable_directory_contributes_nothing`): the expected value is `Path.glob`'s own answer, which on 3.10 differs for entries reached through symlinks and for non-directories (a broken link, a file named like the target). A test that takes a later release's `Path.glob` as the contract: make it version-neutral.
- 3 where a product CLI rejects a root `--` on 3.10: `wrkslots run -- <command>` answers "unrecognized arguments" (`test_slot_images.py` `test_wrkslots_run_boxes_a_plain_worktree_slot`, `test_coordinator_box_refuses_a_bare_slot_and_a_symlinked_agent_registry`), and so does agentctl in `test_agentctl_submission.py` `test_the_cli_passes_deliver_while_working_only_when_asked`. A product defect on 3.10 (older argparse reads `--` differently, see #236 root-terminator-parser).
- 5 parser-probe expectations (#236 root-terminator-parser) and 1 census descriptor snapshot (census-fd-snapshot): fixed with those issues.
- 52 in `wrkslots/tests/test_tool_authority.py` and `test_lifecycle.py` (the result-recorded frozen removal families, `test_recover_absent_agent_row_retains_what_a_writer_holding_a_submodule_directory_makes`): `fcntl.F_ADD_SEALS` is missing from that interpreter, which was built against old kernel headers. Probably environmental; confirm on the hosted runner's 3.10, which has the constant if its build headers define it.
- 1 `tests/test_cross_env_is_hermetic.py` `test_the_interpreter_copy_in_every_case_runs_the_program_its_arguments_name`: a copied relocatable interpreter cannot find its standard library. Environmental to that build.

Each failure the first sharded hosted run lists is either a product defect on 3.10 (fix it), a test that assumes 3.11+ behaviour (make it version-neutral), or one that fails the same way on 3.12 in the same runner (environmental: say why, or run it the way the graph does). Check each against the 3.12 lane before calling it a 3.10 defect.
