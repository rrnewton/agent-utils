---
title: 'merge-queue-validator-admission-line: report donor admission only after the validator admission and pre-run checks pass'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-08T13:30:06.000000000+00:00
updated_at: 2026-11-08T13:30:06.000000000+00:00
---

# Description

Follow-up to #1059 merge-queue-donor-job-refusal, item (3) there. A donor reports a job admitted (`donor admit`) just before it starts the validator, so admission can be recorded minutes before the validator admission wrapper and pre-run checks inside the run have passed — or refused. In the 2026-11-08 incident behind #1059 the donor reported admitted at 12:50:05Z and the validator then refused; the early admission also protects a lease from the stale sweeps for a run that never really started.

# Design

Built like the tool-identity lines of #1045 merge-queue-donor-tool-identity and #1049 merge-queue-unattested-receipts:

- An optional validator output line (exact spelling settled in the change, in the style of `merge-queue-tool-identity:`) meaning "my own admission and pre-run checks passed; validation is about to execute". The validator prints it once, as early as that is true.
- The donor reports admitted only after seeing the line during the run (a watch on the run log, as the early-verdict watch already does in spirit), instead of before starting the validator.
- Without the line, today's behaviour is unchanged: admission is reported before the run. Old validators and old donors keep working in every combination.
- An optional strict config key (in the style of `validator.require_tool_identity_lines`, serialized only when on so existing configs and validator digests are unchanged until opted in) may require the line: with it on, a donor never reports admission before the line, and a run that ends without it was never admitted.

# Acceptance Criteria

- A test: with the admission line present, the job record shows admitted only after the validator prints it (a refusal printed instead leaves the job unadmitted).
- A test: without the line (an old validator), admission timing and stale protection are exactly today's.
- A test (if the strict key is built): with the strict key on, admission is withheld until the line; with it off or absent, the serialized validator config is unchanged.
- A clean whole-crate merge-queue run (rust.merge-queue.test within its 2400 CPU-s budget).

# Notes

Filed 2026-11-08 by the #1059 merge-queue-donor-job-refusal lane, which delivers items (1) and (2) of that issue and defers this item: it needs an in-run admission watch in the donor worker plus a validator config key, while the stale-base refusal fix is P1 for a waiting consumer. Related: #1045, #1049, #1059.
