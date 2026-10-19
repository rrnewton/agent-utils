---
title: 'python-minimum-timeout: the Python 3.10 test job is cancelled at its time limit on every run'
status: closed
priority: 1
issue_type: bug
labels:
- validation
created_at: 2026-10-19T09:35:54.635835330+00:00
updated_at: 2026-10-19T10:39:39.471152307+00:00
closed_at: 2026-10-19T10:39:39.471152207+00:00
---

# Description

[opus 5.5] The repository-ci `python-minimum` job runs the whole `py` suite serially on Python 3.10, the declared minimum of every distribution, and has been cancelled at its 35-minute limit in every run that selected it since 2026-10-10 (latest 2026-10-18), at about 92%. Its pytest summary never prints, so nothing says which tests fail on 3.10; its package-contract step never runs; and every run that selects Python is red whatever the validation graph says. The workflow comment cites a tracker entry that does not exist and a count of 8 expected failures; about 70 failures had printed when it was last cancelled.

The `validate` job has the same exposure from the other side: a run of the whole graph that keeps going past a failure (see ci-failure-masking) takes as long as a green one, close to its 60-minute budget.

Fix:
- keep compileall, the dependency-free startup check and the wheel and sdist contract in a short `python-minimum` job;
- run the tests as six family-preserving shards (`scripts/run_pytest_shard.py`, the graph's own sharder) in a matrix with fail-fast off; each shard names a failing test the moment it fails (the component-test plugin), writes a JUnit report, and lists its failures in the job summary, and its test step has its own time limit below the job's, so a slow shard still reports;
- point the workflow comment at an issue that exists;
- raise the `validate` job's budget from 60 to 90 minutes, the nightly's.

Acceptance: every shard finishes inside its budget on a full run, and the run lists every 3.10 failure.
