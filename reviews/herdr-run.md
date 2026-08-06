# herdr-run paired-port adversarial review — 2026-08-06

Independent implementation and security reviews challenged the Python and Rust packages after the
initial port. The review covered command admission, executable provenance, Herdr protocol
narrowing, workspace/tab lifecycle, concurrency, readiness, byte capture, audit behavior,
configuration parsing, package isolation, and installer-facing documentation.

## Outcome

No high-severity containment vulnerability was found. The package accurately describes its
allowlist as a cooperative safety rail rather than a security boundary against another process
running as the same user. The review did find correctness and hardening gaps that would have made
the initial port unsuitable to publish; all high-priority findings below were fixed in both
implementations before this report was accepted.

## Findings resolved

- Project-local pane locks were replaced by an account-global namespace rooted at the current
  account database home. Both ports derive the same lowercase SHA-256 pane lock name, so callers
  from different projects serialize against the same Herdr pane.
- Workspace/tab discovery and creation now run under one account-global resolution lock and repeat
  live lookup while holding it. Concurrent first runs cannot create duplicate labels.
- Spool, cache, run, and lock directories are forced to `0700`; command, output, metadata, cache,
  and lock files are forced to `0600`. The shell wrapper also sets `umask 077`.
- Every Herdr/systemd control subprocess has a 30-second bound. The Rust process transport drains
  both pipes concurrently, launches a fresh process group, and kills and reaps that group on
  timeout. Invalid subprocess bytes are decoded with replacement in both ports.
- Command/readiness timeouts have one shared one-year maximum. Huge finite values can no longer
  overflow a Rust monotonic-clock deadline into an infinite wait.
- Configuration parsing is strict YAML 1.2 core behavior in both ports: duplicate/merge/custom-tag
  inputs, wrong shapes, non-finite and excessive numbers, control characters, surrogates, invalid
  UTF-8, and unknown keys fail as typed configuration errors without a traceback or panic.
- Protocol responses require the documented list/object/string/integer shapes. Pane identity and
  pane/tab/workspace relationships are checked, duplicate labels fail closed, and cached ids are
  revalidated through unique live label resolution.
- Executable selection ignores caller `PATH` and `HOME`, uses the same fixed candidate order in both
  ports, canonicalizes each candidate, and rejects group/world-writable executables. The same
  validation applies to `systemd-run`.
- CLI differences found by review were pinned and removed: Python option abbreviations are off,
  Rust stops interpreting options after `--`, and environment-derived agent names are trimmed.
- Raw mode emits each captured byte stream once. JSON mode supplies both replacement-decoded text
  and byte-exact base64. Metadata failure after completion warns and yields `"meta": null` instead
  of hiding the command's output or status.
- Audit append failure is visible but best effort. A normal execution records admission before
  target resolution/launch and a final outcome afterward; refusals, dry runs, wrapper failures, and
  doctor probes are represented. Documentation no longer claims durable or tamper-proof logging.
- The fake Herdr implementation moved out of the Python distribution into tests. Python and Rust
  package artifacts contain only their own implementation and standalone language-specific docs.

## Executable evidence

The black-box differential currently exercises 87 paired cases covering startup/help/version and
argument parsing, command tokenization/rendering and policy refusal, successful and malformed
configuration, option placement, dry-run JSON/text output, audit JSONL, control-character classes,
Unicode, and stable exit behavior. The repository contract runs it through `make cross`.

Lifecycle behavior is also pinned independently in both unit suites: account-global first-create
and pane-lock concurrency, cache validation, split-pane refusal, readiness, control-process timeout,
private filesystem modes, byte capture, collision-safe spools, and metadata/audit degradation.
Package checks build and install each wheel/sdist and crate archive in isolation before exercising
its installed command and embedded guide.

## Deliberate limitations

The differential does not inject a fake executable through `PATH`: production resolution rejects
that override by design, and a test hook in the shipped command would weaken the provenance rule it
is meant to test. Therefore live Herdr lifecycle/protocol/concurrency parity is evidenced by
corresponding deterministic unit scenarios, not by the 87 black-box cases. A future external test
environment with an isolated account and real Herdr server can add end-to-end differential coverage
without adding a production bypass.

The default `git` and `gh` policy still grants the capabilities of those programs, including
repository hooks and ambient user configuration. Targeted deny rules remove common escape hatches,
but they are defense in depth rather than a positive capability model. Projects should narrow
`allow` and the deny maps for their use case.

Audit records are private operational evidence, not durable security logs: they are not fsynced,
replicated, immutable, or protected from the same UID. Enforceable policy and audit require a
separate broker plus sandbox rules that deny direct Herdr and user-systemd access.
