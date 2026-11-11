# chat-grants implementation and validation — 2026-10-20

## Delivered behavior

The compiled `agentctl chat` commands manage durable temporary permission:

```sh
agentctl chat grant --bridge-state DIR SENDER... --for 12h --note discussion
agentctl chat grant --bridge-state DIR --all-members --for 7d
agentctl chat grants --bridge-state DIR --json
agentctl chat revoke --bridge-state DIR SENDER...
agentctl chat grant-limit --bridge-state DIR --for 30d
```

Permanent owners remain in `config.allowed_senders`. Explicit grants record the
provider ID, creation and expiry timestamps, and optional note. The maximum
defaults to 30 days and can change on existing state. Grant, revoke and limit
writes use the existing state lock and atomic, synced documents. The running
provider refreshes its explicit sender list after an item or heartbeat. Admission
checks current grants under that lock, including a subscription-scope check
before committing an item received during a permission change. This preserves the
first new sender's message for replay. Expiry is logged once durably; earlier
admitted requests retain their admission evidence and can finish.

The Slack helper resolves names through `slack.user describe --user`, and drains
every page of `slack.conversation members`. Enumeration is complete or refused;
each current member gets a separate ID grant and later joiners receive none.
Readonly directory operations create no posting intent. A forced one-row page
from the installed client confirmed that `source_complete=true` can coexist with
`has_next_page=true`; the helper checks completeness on each page and follows
every token. Only response metadata was used as evidence.

## Delivery receipts

Verification reads other registered panes intentionally to detect misrouted
prompts. A transient read error in an unrelated peer previously ended verification
after one injection. Peer observations now retry within one existing readback
deadline. Persistent failures and fresh or ambiguous peer prompt evidence still
leave delivery unproven; this never injects the prompt again.

A fresh Claude queued-message placeholder inside the composer can now provide
the existing printed-evidence proof while a paste hint hides the working footer.
Preexisting hints remain insufficient, and a redraw that reveals the original
staged prompt returns to the existing dropped-key handling. Logs established the
peer read failure; the queue-display path was reproduced with terminal-frame
tests. No live prompt injection or bridge-state change was used to verify it.
Existing ambiguous deliveries still require operator settlement.

## Commands and results

All commands ran from the repository root. Build outputs stayed in the checkout;
diagnostic logs and mutation backups used scratch storage.

```sh
cargo fmt --all --manifest-path rs/Cargo.toml
cargo fmt --all --manifest-path rs/Cargo.toml -- --check
```

Both passed.

```sh
CARGO_BUILD_JOBS=4 cargo clippy --offline --release --workspace --all-targets \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu -- -D warnings
```

Passed, with warnings denied across all workspace targets.

```sh
RUST_TEST_THREADS=1 CARGO_BUILD_JOBS=4 cargo test --offline --release \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p agentctl -p chat-subscription-slack
```

The initial run passed 1,288 `agentctl` library tests and all its integration
tests, then exited 101 on the new native-helper roundtrip fixture. Cargo's binary
was hardlinked, which the production single-link helper check correctly refused.
The fixture now copies the native binary to private scratch storage with mode
`0500`; no production guard or assertion was changed.

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=1 RUST_TEST_THREADS=1 cargo test \
  --offline --release --config profile.release.package.agentctl.opt-level=0 \
  --config profile.release.incremental=true --manifest-path rs/Cargo.toml \
  --target x86_64-unknown-linux-gnu --target-dir rs/target/grant-mutation-tests \
  -p agentctl --lib grant
```

Passed all 19 grant library tests, including the added blocked-receive ordering.
The unoptimized package override shortens the temporary mutation edit loop;
ordinary release validation uses the normal profile.

```sh
python3 scripts/check_client_names.py
python3 scripts/check_documented_defaults.py
git diff --check
```

Passed.

### Mutation checks

Each row below ran its exact test with the production behavior present, removed
only the named production behavior, ran the same test again, restored the exact
source bytes and mode, then ran the same test once more. This reproduces each
command from the repository root; the runner supplied absolute manifest and
target-directory paths:

```sh
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=1 RUST_TEST_THREADS=1 RUST_BACKTRACE=0 \
  cargo --config profile.release.package.agentctl.opt-level=0 \
  --config profile.release.incremental=true test --manifest-path rs/Cargo.toml \
  --offline --release --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/grant-mutation-tests -p agentctl --lib "$GRANT_TEST" \
  -- --exact --nocapture
```

`GRANT_TEST` was each fully qualified test below. All eight production mutations
compiled and exited 101 with exactly the intended test failure and assertion.
All eight preceding and eight restored checks exited 0. No test assertions,
comparators, tolerances or gates changed. Both mutation groups finished with the
clean `--lib grant -- --nocapture` command and all 19 tests passed again.

| Removed behavior | Exact test (`GRANT_TEST`) |
| --- | --- |
| Grant expiry upper bound | `chat_service::grant_refresh_tests::an_expired_grant_refuses_new_messages_and_preserves_cursor_owners_and_admitted_requests` |
| Durable revocation write | `chat_service::grant_refresh_tests::a_revoked_grant_refuses_new_messages_and_preserves_cursor_owners_and_admitted_requests` |
| Storage maximum-duration guard | `chat_runtime::grants::tests::grant_above_maximum_is_rejected_and_config_changes_affect_an_open_state` |
| Every member after the first per channel | `chat_service::sender_grants::tests::all_members_records_each_current_id_explicitly_and_never_covers_a_later_joiner` |
| Running worker's sender-scope refresh | `chat_service::grant_refresh_tests::a_running_provider_worker_picks_up_a_grant_without_restart_or_provider_failure` |
| Locked scope comparison for an in-flight checkpoint | `chat_service::grant_refresh_tests::a_grant_while_receive_is_blocked_replays_the_filtered_message_without_advancing_its_cursor` |
| Fresh queue marker detection inside the composer | `submission::tests::a_new_queue_placeholder_proves_submission_while_the_paste_hint_hides_the_status` |
| Transient peer-read retry | `subagents::tests::identity::a_transient_peer_read_failure_after_submission_is_retried_and_delivered` |

### Repository validation

```sh
CARGO_NET_OFFLINE=true make validate
```

The first attempt exited 2 because the existing build cache root lacked Cargo's
required `CACHEDIR.TAG`; Cargo refused the launchers' normal clean operation.
The nested target directories already had Cargo-generated tags. After confirming
that the root contained only Cargo outputs, a canonical generated tag was copied
to the missing root. No validation command or Cargo check was bypassed.

The second attempt exited 2 on the public-rustdoc contract: a new comment used
the development-history word "legacy". The comment now describes the optional
document's absence directly. The focused contract then passed (exit 0, one test):

```sh
python3 -m pytest \
  py/tests/test_packaging_infrastructure.py::test_public_api_docs_are_standalone_without_banning_native_package_terms -q
```

The final attempt passed (exit 0): all 28 selected DAG steps passed, with none
failed, aborted or dependency-skipped. The inventory recorded 4,043 primary
cases: all 1,478 Rust cases passed, all four package cases passed, and the Python
inventory had 2,557 passes and four preexisting expected-failure cases reported
as skipped. This includes all 1,308 `agentctl` cases and all 72 Slack-provider
cases, including the corrected native helper roundtrip and six grant CLI tests.
The differential checks and both distribution smoke checks also passed.

The final DAG ran these crate commands, with host target
`x86_64-unknown-linux-gnu`, `RUST_TEST_THREADS=1`, `CARGO_NET_OFFLINE=true`, and
the scheduler's bounded build width:

```sh
python3 scripts/case_timings.py run-cargo -- cargo test --release --no-fail-fast \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p agentctl
python3 scripts/case_timings.py run-cargo -- cargo test --release --no-fail-fast \
  --manifest-path rs/Cargo.toml --target x86_64-unknown-linux-gnu \
  --target-dir rs/target/validation-tests -p chat-subscription-slack
```

Those passed alongside the core, plugin and fake-provider crate suites.
Workspace rustfmt and workspace clippy with `-D warnings` also passed in the
final DAG. The later change to this report only records those completed results;
the Rust sources, tests, operator guides and checks are unchanged.

## Review corrections and provider contract

Independent review caught two additional cases before final validation:

- An unresolved Slack name without a helper could have been stored as an ID and
  broken provider startup. The command now refuses it before writing. Subprocess
  tests include names, email addresses, another provider's ID syntax, short and
  oversized Slack IDs, and successful valid raw IDs.
- A grant while provider receive was blocked could have committed a filtered
  checkpoint before refreshing. The scope comparison now shares the grant
  writers' exclusive admission lock. A real running-worker test checks no ACK or
  cursor advance for the stale item, the exact resumed cursor and sender IDs,
  successful guest replay, and uninterrupted worker health.

The bundled Slack provider resumes strictly after its saved event sequence.
Inclusive providers must preserve the saved boundary's canonical replay evidence
when their sender list changes. Existing boundary-mismatch refusal and explicit
recovery remain in force; no comparator or replay guard was relaxed.
