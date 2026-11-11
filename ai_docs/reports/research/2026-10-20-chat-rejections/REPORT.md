# chat rejection feedback — 2026-10-20

## Delivered behavior

The compiled bridge defaults `reject_reaction` to ❌ (Slack shortcode `x`). A
custom emoji selects another marker; `null`, `""`, and `false` disable feedback.
Existing ACK and delivery-receipt settings keep their meanings. Inbound-only
configurations with outbound transport disabled remain valid.

Eligible new human messages in configured channels get rejection feedback when
their sender is not permitted, including an expired or revoked temporary grant,
or when an ignored prefix excludes the message. The Slack provider accepts the
same new-message subtypes as request admission: no subtype, `thread_broadcast`,
`file_share`, and `me_message`. Other subtypes, including edits, deletes, bot
messages and channel notices, remain ignored without a reaction. Non-message
events, other channels, invalid identifiers and events predating the bridge's
lineage also receive none.

The provider sends only validated channel, message and sender identifiers plus a
rejection reason. Rejected text never becomes a request, prompt or reply route.
The host preserves the existing channel, sender-scope, replay and resource-bound
checks and rechecks current permission under the admission lock. A provider
denial from before a new grant cannot reject the newly permitted sender.
Inclusive boundary evidence must still match exactly; a provider that changes
the saved boundary after a policy change is refused. The bundled Slack provider
resumes strictly after its cursor.

Slack bot and app messages never receive rejection reactions. The dedicated
`ensure_rejection` helper operation verifies the supplied sender through
`slack.user describe --user=ID --columns=user_id,is_bot`. It requires one exact
matching user ID and a boolean `is_bot=false` before writing. Missing, unknown,
ambiguous or bot accounts produce no write. Event delivery `app_id` is not
treated as the author's account type. The authenticated provider supplies the
sender-to-message binding; the helper checks account type rather than fetching
the message again. Lookup and reaction share one timeout deadline.

## Durability and service behavior

The bounded rejection outbox uses the existing state lock and synced atomic
documents. Its operation ID is deterministically derived from provider, channel
and message, and survives replay, retry and restart. Completed, refused and
suppressed entries retain terminal replay evidence. The store holds at most 256
entries and 8 MiB; terminal entries can be evicted. Reconstructed feedback keeps
the same operation ID. The Slack helper's separate durable operation ledger
retains the first app, resource, sender and emoji binding, so an emoji change
after host eviction cannot add a second marker. Each helper record is bounded
at 4 KiB and looked up directly by UUID, with a separate operation lock; total
ledger disk usage grows with retained history, as does the existing reply
ledger. Keep that history with the bridge state. If every host entry is pending,
new feedback is dropped and logged without delaying the cursor.

Normal intake queues an independent bounded worker after cursor commit. Recovery
may retry independently saved feedback from an interrupted batch. A blocked,
failed or uncertain helper does not hold request admission or stop the service.
Uncertain attempts retry with the same saved ID. Nonretryable refusals are
terminal. Outbox errors are logged without converting feedback into an admission
failure. The helper runs outside the state lock. `tick` also performs bounded
recovery. Disabling feedback leaves saved work dormant; reenabling it retains
the original saved emoji.

Later successful admission suppresses pending feedback for that message after
the request and cursor are durable, even if feedback is disabled or the batch
has no new rejection candidates. Dispatch independently checks current requests
and retained retired identities. An interrupted admission before request
installation preserves the earlier pending refusal. A helper already in flight
can still complete a rejection that was valid when dispatched; earlier applied
reactions are not recalled. The userguide states this limit.

The wire capability is optional for the host: an older provider continues
serving requests and produces a visible missing-capability warning. Updated
feedback needs the bundled provider and guarded helper operation. An older host
refuses the new advertised event kind before Start. Unsupported helper
transports refuse feedback rather than falling back to an unguarded reaction.

The quickstart, userguide Reactions section, provider README and shared provider
protocol guides describe these contracts. The already delivered temporary-grant
and delivery-receipt work is recorded separately in the adjacent chat-grants
report; this change builds on its signed base `941d615dad4ced007766cfb3782a55c7f2713466`.

## Review corrections

Independent review found that pending feedback could outlive a later successful
admission. Cancellation now follows durable request installation, with a second
dispatch guard for both current and retired requests. Five new regressions cover
the distinct ordering, disabled-feedback, restored-pending and completed-history
cases. Suppressed feedback is a separate terminal outcome; it does not invent a
helper attempt or successful receipt.

Final review also found that stable UUIDs alone did not cover an emoji change
after host tombstone eviction: Slack deduplicates by message and emoji, and the
runtime fake had instead deduplicated by UUID. The helper now persists each
rejection operation's first binding in its existing configured ledger before a
possible side effect, and retains it after the host outbox evicts a completed
row. A repeated operation returns its original receipt rather than adding a new
emoji. A full host-admission and real-helper regression covers this sequence
using the Slack fake's actual reaction identity. The runtime fake now models
the message-and-emoji identity as well. New tests and mutations cover durable
binding, uncertain outcomes and identity conflicts.

The first attempt to reproduce the eviction edge used the Unicode emoji 🚫,
which the Slack helper does not map. It compiled but correctly refused
`reaction_unsupported`, so that failure was not counted as evidence for the
duplicate-marker bug. The fixture now uses the already supported shortcode
`no_entry_sign`, without changing production mapping or weakening an assertion.
Against unchanged old production it compiled and failed the intended exact
receipt equality: Slack had added `no_entry_sign` after the original `x`.

The journal's first compile attempt hit the crate's `deny(unsafe_code)` policy.
The filesystem syscalls are now isolated in small functions with explicit
safety comments and the repository's existing per-function exception pattern;
the crate-wide denial remains. The private journal entry point also has a local
`clippy::too_many_arguments` allowance for its validated operation fields; no
test or behavioral check is exempted. The first executable journal run also found that
syncing every preexisting path ancestor reached a filesystem that does not
support directory sync. The helper now pins that ancestry without changing it,
requires the configured ledger's parent to exist, and creates and syncs only
the managed ledger and rejection-directory entries. Existing managed entries
are also synced to close a concurrent-creation race. No sync error is ignored.

Review also found a timeout fixture below the helper's existing supported
minimum. The fixture now uses 1000 ms and a 1.2-second lookup delay. The production
1000–29000 ms range and exact timeout/zero-write assertions are unchanged. The
separate shared-budget test still uses two 650 ms operations under a 1000 ms
deadline. No test assertions, tolerances, comparators or validation gates were
weakened, and no cases were skipped or relabelled.

## Validation commands and results

Commands ran from the repository root. Normal release checks use the unmodified
release profile. Build outputs remain in the checkout; logs and temporary
mutation backups use scratch storage.

The focused commands in this section first validated the candidate before the
last helper-ledger correction. The final-source mutation runs and repository
gate are recorded separately below.

```sh
cargo fmt --all --manifest-path rs/Cargo.toml
cargo fmt --all --manifest-path rs/Cargo.toml -- --check
```

Formatting passed before the mutation runs. The explicit check passed again
after all mutations were restored, and the final repository gate also passed
its workspace format check.

```sh
RUST_TEST_THREADS=1 CARGO_BUILD_JOBS=4 cargo test --offline --release \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p agentctl --lib rejection -- --nocapture
```

Passed: 21 runtime and service tests, none failed or ignored. These include real
provider-worker dispatch, causal blocked-helper continuation, permanent-failure
continuation, durable restart recovery, stable replay identity, settings and
strict wire binding.

```sh
RUST_TEST_THREADS=1 CARGO_BUILD_JOBS=4 cargo test --offline --release --no-fail-fast \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p chat-subscription-slack
```

Passed: all 83 Slack tests, with none failed or ignored, including bot/app
refusal, exact account verification, real helper roundtrips and shared deadlines.

```sh
CARGO_BUILD_JOBS=4 cargo clippy --offline --release --workspace --all-targets \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu -- -D warnings
python3 scripts/check_client_names.py
python3 scripts/check_documented_defaults.py
python3 scripts/embed_userguides.py --check
git diff --check
git diff --cached --check
```

All passed. Clippy denies warnings across every workspace target.

### Earlier unsuccessful checks

The first broad Rust run omitted the repository scheduler's `RUST_TEST_THREADS=1`
setting:

```sh
CARGO_BUILD_JOBS=4 cargo test --offline --release \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p agentctl -p chat-subscription \
  -p chat-subscription-plugin -p chat-subscription-slack
```

It exited 101: 1297 agentctl library cases passed and eight failed before the
remaining binaries ran. One exact logging expectation needed the new optional
capability warning and was updated to require the complete two-line output.
Seven existing service timing tests also failed in parallel. Their assertions
and deadlines were not changed; the final repository gate runs them with its
configured serialized test setting.

The first clippy invocation of the command above exited 101 on
`cloned_ref_to_slice_refs` in a new test. An equivalent exact slice equality
removed the unnecessary clone; no exemption was used for that finding. The
repeated workspace command passed. The later host-eviction regression hit the
same lint and received the same exact slice comparison before scoped clippy
passed.

```sh
CARGO_NET_OFFLINE=true make validate
```

The first attempt exited 2: 24 steps passed, one Slack step failed and three
other steps were aborted on eager exit. The unsupported 100 ms timeout fixture
described above failed; 82 of 83 Slack tests passed. The core's 32 tests, plugin's 62 tests,
workspace fmt/clippy and other completed steps passed. This attempt preceded
the pending-feedback review correction and does not establish the final gate.

### Mutation checks

Each mutation runs one exact regression before changing production code, removes
only the named behavior, runs the same regression, restores the exact source
bytes and file mode, and runs the regression again. Assertions and test sources
remain unchanged. A compiler failure, timeout, zero-test run or unrelated failure
does not qualify as a killed mutation. The temporary package optimization setting
below shortens this edit loop; the repository gate uses the normal release
profile.

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=1 RUST_TEST_THREADS=1 RUST_BACKTRACE=0 \
  cargo --config profile.release.package.agentctl.opt-level=0 \
  --config profile.release.incremental=true test --manifest-path rs/Cargo.toml \
  --offline --release --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/grant-mutation-tests -p agentctl --lib "$REJECTION_TEST" \
  -- --exact --nocapture
```

`REJECTION_TEST` selects the fully qualified test in each row:

| Removed behavior | Exact test |
| --- | --- |
| Unallowed sender candidate capture | `chat_runtime::rejections::tests::rejected_human_identity_saves_feedback_without_creating_a_request` |
| Newly permitted sender exclusion | `chat_runtime::rejections::tests::a_new_grant_or_permanent_owner_does_not_get_a_stale_sender_rejection` |
| Disabled transport guard | `chat_runtime::rejections::tests::disabling_rejections_leaves_saved_work_dormant_and_keeps_its_original_emoji` |
| Completed replay tombstone | `chat_runtime::rejections::tests::a_failed_rejection_reuses_its_operation_after_restart_and_replay` |
| Expired/revoked stale-subscription feedback | `chat_runtime::rejections::tests::expiry_and_revocation_of_a_live_provider_sender_save_rejection_feedback` |
| Stable retry operation ID | `chat_runtime::rejections::tests::a_failed_rejection_reuses_its_operation_after_restart_and_replay` |
| Best-effort feedback persistence | `chat_runtime::rejections::tests::rejection_outbox_failure_does_not_prevent_cursor_commit_or_other_requests` |
| Running service queue dispatch | `chat_service::rejection_tests::a_blocked_or_failing_rejection_helper_does_not_hold_cursor_or_later_requests` |
| Service continuation after helper failure | `chat_service::rejection_tests::a_blocked_or_failing_rejection_helper_does_not_hold_cursor_or_later_requests` |
| Post-admission pending cancellation | `chat_runtime::rejections::tests::later_admission_suppresses_pending_rejection_without_new_candidates_even_when_disabled` |
| Dispatch guard for current requests | `chat_runtime::rejections::tests::the_helper_snapshot_suppresses_restored_pending_feedback_for_a_current_request` |
| Dispatch guard for retired requests | `chat_runtime::rejections::tests::the_helper_snapshot_suppresses_restored_pending_feedback_for_a_retired_request` |
| Cancellation ordering after admission succeeds | `chat_runtime::rejections::tests::an_interrupted_admission_keeps_prior_feedback_pending_before_request_installation` |

All 13 production mutations compiled and exited 101 with exactly the intended
test and assertion failing. All 13 qualifying and 13 restored exact baselines
passed. There were no invalid attempts, retries or command timeouts. Final clean
`--lib rejection -- --nocapture` passed all 21 tests, none failed or ignored.
Independent byte and mode comparisons confirmed restoration of every mutated
source; the guarded test files were unchanged.

The Slack checks use the same environment and profile with the following crate
and exact integration-test selection:

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=1 RUST_TEST_THREADS=1 RUST_BACKTRACE=0 \
  cargo --config profile.release.package.agentctl.opt-level=0 \
  --config profile.release.incremental=true test --manifest-path rs/Cargo.toml \
  --offline --release --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/grant-mutation-tests -p chat-subscription-slack \
  --test "$SLACK_SUITE" "$SLACK_TEST" -- --exact --nocapture
```

| Removed behavior | `SLACK_SUITE` | `SLACK_TEST` |
| --- | --- | --- |
| Exact ID and explicit human account check | `directory` | `rejection_never_writes_for_bot_unknown_ambiguous_or_mismatched_accounts` |
| Default transport human guard | `outbound` | `bot_and_unknown_accounts_never_receive_rejection_reactions` |
| Provider unallowed-sender metadata | `plugin` | `eligible_rejections_preserve_exact_metadata_and_advance_the_cursor` |
| Unsupported-subtype exclusion | `plugin` | `edits_deletes_bots_notices_and_invalid_metadata_have_no_rejection_feedback` |
| Shared lookup/write deadline | `directory` | `rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper` |

All five Slack mutations compiled and exited 101 at the intended assertion.
All five qualifying and five restored exact baselines passed. There were no
invalid attempts, retries or command timeouts. The final full Slack package
passed all 83 tests, none failed, ignored or filtered. Every mutated source's
bytes and mode were restored, and all other Slack Rust sources, integration
tests and shared reaction definitions matched their guarded snapshots.

### Repository validation before the last review correction

```sh
CARGO_NET_OFFLINE=true make validate
```

Passed (exit 0): all 28 selected DAG steps passed in 584.8 seconds, with no
failed, aborted, intentionally skipped, dependency-skipped or unlaunched steps.
The normalized inventory recorded 4,086 primary cases: all 1,521 Rust cases and
all four package cases passed; Python recorded 2,557 passes and four preexisting
expected-failure cases represented as skips. Both differential checks passed:
760 agentctl paired checks and 101 herdr-agent paired checks. Workspace rustfmt,
workspace clippy with `-D warnings`, standalone package guides and all repository
hygiene checks passed.

The crate test steps used the following command form, with
`RUST_TEST_THREADS=1`, `CARGO_NET_OFFLINE=true` and the scheduler's bounded build
width. `CRATE` was each exact package in the table:

```sh
python3 scripts/case_timings.py run-cargo -- cargo test --release --no-fail-fast \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p "$CRATE"
```

| `CRATE` | Cases passed | Cases failed |
| --- | --- | --- |
| `agentctl` | 1,329 | 0 |
| `chat-subscription` | 32 | 0 |
| `chat-subscription-plugin` | 62 | 0 |
| `chat-subscription-fake` | 15 | 0 |
| `chat-subscription-slack` | 83 | 0 |

The previously failing service cases all passed with the repository's configured
serialized test setting. Final review then found the helper-ledger correction
described above. These results cover the candidate before that correction; the
updated source requires the final gate recorded below.

### Final-source mutation checks

After the journal correction, the runtime fake was changed to model Slack's
channel/message/emoji identity while retaining all stable-UUID assertions. All
13 runtime mutations were repeated against that source: every mutation compiled
and failed its intended assertion (exit 101), all 26 qualifying/restored exact
baselines passed, and the final clean rejection library run passed all 21 cases.
Every mutated source was restored byte for byte with its original mode, and all
guarded test files remained unchanged. There were no invalid attempts or
timeouts in this run.

The helper-ledger checks use the Slack exact-test command form above. Their
additional production mutations target these tests:

| Removed behavior | Suite | Exact test |
| --- | --- | --- |
| Original emoji after host eviction and configuration change | `bridge_wire` | `replay_after_host_eviction_and_emoji_change_keeps_one_slack_rejection` |
| Original emoji after an applied operation loses its response | `outbound` | `uncertain_rejection_replay_keeps_the_first_emoji_and_rechecks_the_author` |
| Prior ambiguity after a later author refusal | `outbound` | `a_refused_retry_preserves_the_uncertainty_of_an_earlier_rejection` |
| Immutable app binding | `outbound` | `reused_rejection_uuid_cannot_change_its_app_channel_message_or_sender` |
| Immutable channel binding | `outbound` | `reused_rejection_uuid_cannot_change_its_app_channel_message_or_sender` |
| Immutable message binding | `outbound` | `reused_rejection_uuid_cannot_change_its_app_channel_message_or_sender` |
| Immutable sender binding | `outbound` | `reused_rejection_uuid_cannot_change_its_app_channel_message_or_sender` |
| Durable intent before a possible write | `outbound` | `uncertain_rejection_replay_keeps_the_first_emoji_and_rechecks_the_author` |
| Durable completion before a success receipt | `outbound` | `rejection_persistence_failure_prevents_a_write_or_a_false_success_receipt` |
| Fail closed on an unreadable journal | `outbound` | `corrupt_oversized_or_unsafe_rejection_journals_contact_nothing` |
| Native per-operation lock | `directory` | `a_locked_rejection_operation_refuses_without_waiting_or_starting_the_cli` |
| Persistence charged to the shared deadline | `directory` | `rejection_persistence_time_is_part_of_the_same_cli_deadline` |
| 4 KiB journal record bound | `outbound` | `corrupt_oversized_or_unsafe_rejection_journals_contact_nothing` |

The oversized fixture remains valid JSON with whitespace padding to 4,097 bytes;
its failure therefore independently checks the byte bound. The actual host
eviction fixture uses public admission and the real helper/wire path; fixture
setup records actual completion receipts in the otherwise private host outbox
while no service is running. Its fake provider deduplicates physical reactions
by app, channel, message and emoji, independently of UUID. A native helper-process
test separately covers restart replay without another CLI call.

All five original Slack mutations and all 13 journal mutations passed their
qualification: every production mutation compiled and failed the intended
runtime assertion (exit 101), and all 36 qualifying/restored exact baselines
passed. Each runner's final clean Slack package passed all 94 cases with none
failed or ignored. There were no invalid attempts, retries or timeouts. An
independent comparison of all 28 guarded source/test/manifest files confirmed
their exact SHA-256 hashes and modes after restoration. Together with the
runtime checks, the final source has 31 compiled mutation kills and 62 passing
qualifying/restored exact baselines. No test was edited during a mutation run.

### Final repository validation

```sh
CARGO_NET_OFFLINE=true make validate
```

Passed (exit 0): all 28 selected steps passed in 580.5 seconds, with no failed,
aborted, intentionally skipped, dependency-skipped or unlaunched steps. This run
covered the final journal source and natural-identity runtime fake. The staged
tree before this result-only report update was
`077977a43274d17ce6f14af3cd411dbcbf1f3ba0`.

All 1,532 Rust cases passed under the normal release profile and configured
serialized test setting:

| Package | Passed | Failed |
| --- | --- | --- |
| `agentctl` | 1,329 | 0 |
| `chat-subscription` | 32 | 0 |
| `chat-subscription-plugin` | 62 | 0 |
| `chat-subscription-fake` | 15 | 0 |
| `chat-subscription-slack` | 94 | 0 |

The crate steps used the exact command form above through
`scripts/case_timings.py run-cargo`. Workspace rustfmt and clippy across all
targets with `-D warnings` passed. All four package cases passed. Python
recorded 2,557 passes and four preexisting expected failures represented as
skips; no new test was skipped. Both differential suites passed, with 760
agentctl pairs and 101 herdr-agent pairs. Embedded guides, documented defaults,
client-name checks and the remaining repository hygiene steps passed. The
normalized inventory contains 4,097 primary cases.
