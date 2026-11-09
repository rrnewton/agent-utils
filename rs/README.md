# agent-utils for Rust

This workspace contains the independently installable Rust editions of the
agent-utils command-line tools. Each crate owns its binaries, library API,
version, license, README, and embedded user guide.

| Crate | Binaries | Purpose |
|---|---|---|
| `term-backend` | library | Backend-neutral terminal-multiplexer traits with Herdr, tmux, and fake backends. |
| `chat-subscription` | library | Model ordered inbound events, replay gaps, and durable acknowledgement boundaries. |
| `chat-subscription-plugin` | library | Carry the subscription contract over a bounded, versioned process protocol. |
| `chat-subscription-slack` | `agentctl-slack-plugin`, `agentctl-slack-outbound` | Slack subscription plugin and bot-identity reply helper for `agentctl chat`. |
| `dagrun` | `dagrun`, `cpuset-alloc` | Run and inspect resource-aware CI DAGs; reserve hard-isolated CPU sets for benchmarks. |
| `tick-hub` | `tick-hub` | Evaluate cadenced reminders and health checks in one deterministic tick. |
| `pr-landing-planner` | `pr-landing-planner` | Produce advisory, conflict-aware pull-request landing plans. |
| `herdr-run` | `herdr-run` | Execute policy-admitted shell commands through Herdr panes. |
| `agent-queue` | library | Share one durable prompt-queue format and process-identity proof between agent tools. |
| `agent-harness` | library | Find and read the session transcripts coding-agent harnesses write as they run. |
| `agentctl` | `agentctl`, `herdr-agent`, `herdr-subagents` | Control persistent interactive Herdr agents through one session registry. |

Install one published command with Cargo:

```sh
cargo install dagrun
cargo install tick-hub
cargo install pr-landing-planner
cargo install herdr-run
cargo install agentctl
```

`herdr-run` reads `.herdr-run.yaml` with its own YAML-subset parser and depends on no YAML
library. The golden corpora under `rs/herdr-run/testdata/` pin its text, configuration, policy,
protocol, readiness, and run-record behaviour, and its unit tests load every one of them.

The two chat-subscription libraries are ordinary Cargo dependencies for provider
and host implementations; they do not install command-line programs.

For workspace development:

```sh
host_target="$(rustc -vV | sed -n 's/^host: //p')"
cargo build --release --workspace --manifest-path rs/Cargo.toml --target "$host_target"
cargo test --release --workspace --manifest-path rs/Cargo.toml --target "$host_target"
cargo clippy --release --workspace --all-targets --manifest-path rs/Cargo.toml \
  --target "$host_target" -- -D warnings
```

The explicit host target shares the cache layout used by the source launchers.

The tracked `rs/bin/<command>` links are source-checkout launchers, not copied
build artifacts. They ask Cargo to validate its incremental workspace cache,
verify and refresh binary provenance, and then replace themselves with the
resulting host-target executable. The target and target directory are explicit,
so ambient Cargo target selection cannot redirect a build away from the binary
that will run. Before execution, the launcher publishes a named,
content-addressed copy outside the deletable Cargo cache; containment code can
therefore safely re-execute its own path while another process cleans or rebuilds.
Direct invocations cannot silently use a binary from older checked-out source.

Cargo and the host-compiler probe run from filesystem root with absolute
workspace and target paths. This prevents a consumer repository that contains
the agent-utils checkout from injecting an enclosing `.cargo/config.toml` or
`rust-toolchain.toml`. Explicit environment choices and the normal user
`CARGO_HOME` configuration remain available. After validation, the utility
itself still starts in the caller's original working directory.

These source-checkout launchers target Linux development hosts and require Git,
Bash 4+, Cargo/Rust, GNU coreutils (`cp`, `readlink`, and `sha256sum`), and
util-linux `flock`. This development path does not affect the portability or
behavior of standalone binaries produced by `cargo install`.

`make clean` removes `rs/target` while holding the launcher lock. It intentionally
retains the content-addressed `rs/.agent-utils-snapshots` cache: an already
running containment command may still need its stable path for re-execution.
Those snapshots are never selected by name or age, only by a freshly verified
binary hash. They may be removed manually when no source-checkout Rust command
is running.

On the first `./setup rs` after upgrading from the former copied-binary layout,
setup removes only the known legacy `rs/bin/<command>.provenance` cache
files. Any other unexpected `rs/bin` entry is reported and refused.

Run `make check-rust-packages` from the repository root to build, inspect, and
smoke the registry archive for each crate independently.

## Registering a new `rs/bin` binary

Adding a `[[bin]]` to a workspace crate is not enough: a tracked source-checkout
command is registered in several places, and validation fails closed on any of
them missing. Work through all of these in the same change:

1. Add the `[[bin]]` target to the owning crate's `Cargo.toml`, map the command
   to its package and sibling tools in `rs/bin/cargo-runner`, and add the tracked
   `rs/bin/<command>` symlink to `cargo-runner`.
2. Add the command to `setup_rs` in `setup` — both the `rust_tools` array and
   the `find` allowlist that refuses unexpected `rs/bin` entries.
3. Record the tool in `validation/editions.json`: primary edition, mirror
   state, and both entrypoints as the mirror-state model requires (an `absent`
   mirror may not name an entrypoint; a `parity` mirror must name a cross job;
   a `paused` mirror must name `paused_at`). Then regenerate the resolver's
   copy with the editions checker's `--write-tsv` mode and run that checker by
   itself.
4. Bucket the command in the dispatch-wiring test (`RUST_TOOLS` for a bin in a
   published crate, `NON_PUBLISHED_RUST_BINARIES` for one in a
   `publish = false` crate); the test scans the Cargo manifests, so the bucket
   must match the crate's publish flag.
5. Register the binary in the Rust package checker and run it for the crate
   (`--crate <name>`); register the launcher in `validation/components.json`
   too.
6. Give the binary an embedded user guide in the crate and a rendered guide, so
   `--userguide` works from the built binary and the registry smoke passes.
