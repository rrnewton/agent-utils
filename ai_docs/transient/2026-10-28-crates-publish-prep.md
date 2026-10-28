# crates-publish-prep: publishing the Rust tools on crates.io

Working design, 2026-10-28. `#626 crates-publish-prep`. Measured on `main` at `2ec42281` with
cargo 1.99. Preparation only: nothing has been published, and nothing will be until the owner
approves an exact crate list (section 5).

## 0. Summary

- Four tools are ready now: **dagrun** (with `cpuset-alloc`), **tick-hub**, **pr-landing-planner**
  and **herdr-run**. All four names are free on crates.io, their archives pass the repository's
  package check, and `cargo publish --dry-run` passes against the live crates.io index.
- **agentctl is not ready.** Its name belongs to an unrelated 2025 placeholder crate, and its
  archive carries references that should not ship in a release that cannot be deleted (below).
- **vibe-talk is not ready** but is close to it: its archive builds standalone, but it is
  `publish = false`, has no LICENSE file of its own, and ships tests, scripts and deployment files.
- **`agent-utils` is free.** Make it the meta package: one `cargo install agent-utils` builds
  every tool as a binary of that one package (section 3).
- `cargo publish` cannot run from an agent here: the pane allowlist admits only `git` and `gh`,
  this host refuses the crates.io API to agent identities, and an upload needs the owner's token.
  The owner runs the final commands (section 4).

## 1. Readiness per crate

Archive sizes are from `cargo package --no-verify --offline` (compressed / files). "Dry run" is
`cargo publish --workspace --dry-run --locked` over `rs/`, which packages, verifies (compiles) and
stops before upload for every crate not marked `publish = false`. It passed for all nine such
crates. A dry run does not check name ownership.

| Crate | Binaries | Version | Archive | Verdict | Blocker |
|---|---|---|---|---|---|
| `dagrun` | `dagrun`, `cpuset-alloc` | 0.15.0 | 616 KiB / 69 | **READY** | none |
| `tick-hub` | `tick-hub` | 0.2.0 | 50 KiB / 19 | **READY** | none |
| `pr-landing-planner` | `pr-landing-planner` | 0.1.0 | 78 KiB / 21 | **READY** | none |
| `herdr-run` | `herdr-run` | 0.4.0 | 253 KiB / 48 | **READY** | none (see the naming note in section 2) |
| `term-backend` | library | 0.1.0 | 21 KiB / 12 | ready, hold | only `agentctl` uses it; publish with it |
| `chat-subscription` | library | 0.1.0 | 23 KiB / 8 | ready, hold | as above |
| `chat-subscription-plugin` | library | 0.1.0 | 73 KiB / 9 | ready, hold | as above |
| `agent-queue` | library | 0.1.0 | 40 KiB / 12 | NOT READY | name taken (an unrelated job-queue crate) |
| `agentctl` | `agentctl`, `herdr-agent` | 0.1.0 | 1.6 MiB / 185 | NOT READY | name taken; private references in the archive |
| `agent-teams` | `agent-teams` | 0.1.0 | 93 KiB / 26 | NOT READY | `publish = false`; name taken; depends on `agent-queue` (also taken); no LICENSE; README is source-tree build steps |
| `agent-usage` | `agent-usage` | 0.1.0 | 64 KiB / 31 | NOT READY | `publish = false`; name taken; no LICENSE, keywords or categories |
| `gh-paced` | `gh-paced` | 0.1.0 | 249 KiB / 27 | NOT READY | `publish = false`; no README or LICENSE; not in the package check |
| `wrkslotsd` | `wrkslotsd` | 0.1.0 | 223 KiB / 49 | NOT READY | `publish = false`; no README, LICENSE, keywords or categories; internal observer mid-port (`#562 rust-primary-wrkslots`); publish as `wrkslots` once primary |
| `chat-subscription-slack` | `agentctl-slack-plugin`, `agentctl-slack-outbound` | 0.1.0 | 147 KiB / 29 | NOT READY | `publish = false`; needs a separately installed Slack client; no LICENSE, keywords or categories |
| `chat-subscription-fake` | `chat-subscription-fake-plugin` | 0.1.0 | 11 KiB / 7 | never | test fixture |
| `vibe-talk` | `vibe-talk`, `vibe-talk-mock-elevenlabs` | 0.1.0 | 6.2 MiB / 424 | NOT READY | listed below |

What the ready crates already have. Every crate in `scripts/check_rust_packages.py` (the four
tools and the four agentctl libraries, plus agentctl) has a description, license, repository,
README, keywords and categories, a LICENSE file in the archive, and versioned path dependencies
(`version = "0.1.0", path = ...`), which Cargo rewrites to registry dependencies on upload. That
check builds each archive in isolation against archives of its dependencies and holds the
packaged documents to the standalone-documentation rules, and `scripts/validate.py` selects it
(per crate) for changes to those crates. Only `agentctl` has a `build.rs`, and it only records an optional source fingerprint from
the environment. No crate reads files outside its own directory.

**agentctl's private references.** The archive's source and embedded guides contain a default
WebSocket endpoint on a private organisation's network (the agentcloud adapter's
`DEFAULT_AGENTCLOUD_ENDPOINT`), about two dozen private model code names in guides and test
transcripts, a development host name in one regression comment, and the adapter for a private
managed runtime and its command-line clients. All of it is already public on GitHub, but a
crates.io version cannot be deleted, only yanked. `scripts/check_client_names.py` does not cover
hosts or endpoints; these turned up in a search of the extracted archives for host, endpoint and
code-name patterns, which the package check should run on every archive. Before agentctl's first release,
remove the default endpoint (require it from configuration), scrub the code names and host name,
and decide whether the managed-runtime adapter ships at all, for example behind a non-default
feature.

**vibe-talk.** A verified `cargo package` build passes, so the archive (build script grammars,
`include_str!` web assets, prompts) is self-contained. To make it publishable: remove
`publish = false`; add a LICENSE file; add an `include` list (`src`, `web`, `prompts`,
`syntaxes`, `build.rs`, the example configuration, `README.md`, LICENSE, and the four
`tests/fixtures` files that unit tests embed), which drops the 4.9 MB JavaScript test, 188 test
files, scripts, the Containerfile and systemd units (`contract/` is not needed to build); point
`readme` at a short crate README instead of the 900 KB repository README, while still including
`README.md`, which a unit test reads (`src/store/sqlite.rs`); add keywords and categories; add it to the package
check so its documents meet the same standalone rules; and update the yanked `chacha20 0.10.1` in
its lock file (only `cargo install --locked` would notice). It is its own workspace, which is
fine for publishing.

## 2. Names

Checked against the crates.io sparse index on 2026-10-28; owners identified from the published
archives' manifests.

| Package today | Status | Publish as | Binary |
|---|---|---|---|
| `dagrun`, `tick-hub`, `pr-landing-planner`, `herdr-run` | free | unchanged | unchanged |
| `term-backend`, `chat-subscription`, `chat-subscription-plugin` | free | unchanged | (libraries) |
| `vibe-talk`, `gh-paced`, `agent-utils` | free | unchanged | unchanged |
| (Python today) `wrkslots`, `wrkviz` | free | reserved for a future Rust edition | unchanged |
| `agentctl` | taken: 0.1.0 placeholder, AGPL, unrelated author (2025-04) | **`agent-utils-agentctl`** | `agentctl`, `herdr-agent` |
| `agent-queue` | taken: unrelated job queue (0.2.0, 2026-07) | **`agent-utils-queue`** | (library `agent_queue`) |
| `agent-teams` | taken: unrelated teams framework (0.1.0, 2026-02) | **`agent-utils-teams`** | `agent-teams` |
| `agent-usage` | taken: unrelated usage monitor (0.1.11) | **`agent-utils-usage`** | `agent-usage` |
| `minibeads` | taken by the owner's own repository | n/a (not in this tree) | `mb` |

The `agentctl-*` names are contested too: `agentctl-cli` and `agentctl-core` belong to another
project, and `agent-ctl` to a third. The `agent-utils-` prefix is free and says whose crate it
is, so use it for every collision and for agentctl's planned sub-crates.

**Keeping the binary name.** Only the package name changes. `[[bin]] name = "agentctl"` is already
explicit, and adding `[lib] name = "agentctl"` keeps every `use agentctl::...` unchanged.
Dependents rename on the dependency line:
`agentctl = { package = "agent-utils-agentctl", version = "...", path = "../agentctl" }`. Users
then run `cargo install agent-utils-agentctl` and get `agentctl`. Two unrelated crates,
`agentctl-cli` and `agent-ctl`, also install a binary named `agentctl` (and the taken
`agent-teams` and `agent-usage` crates install binaries of those names), and `cargo install`
refuses to overwrite another package's binary without `--force`.

The rename also touches `rs/bin/cargo-runner`, the registry in `scripts/check_rust_packages.py`
(which asserts the package name equals the directory name), `-p agentctl` in validation and
docs, and the `agent-queue` dev-dependency lines. It also collides with the package check's
documentation rules: they reject the string `agent-utils` and any sibling crate's name in every
packaged document, so a renamed crate's README could not say `cargo install agent-utils-agentctl`,
and a meta package could not name itself or its tools. Those rules need a narrow exception (a
crate may name its own package and, for the meta package, the tools it installs), or the prefix
is reconsidered. That is a lane of its own, with the agentctl scrub.

**`herdr-run` and Herdr.** The upstream terminal multiplexer publishes the crate `herdr`.
`herdr-run` and `herdr-agent` are free and describe what they do, but they could read as
upstream's own. Keeping them is reasonable; the README should say they are independent clients.

## 3. Installing the whole suite

`cargo install agent-utils` installs only the binaries of the package named `agent-utils`, never
those of its dependencies. Two designs work:

**A. One package, every tool a binary (recommended).** Add an `agent-utils` package with a
feature per tool and a thin `[[bin]]` per command, each `required-features = ["<tool>"]`, calling
the tool's library entry point. Every tool's `main.rs` is already a 4-25 line shim over its
library, and each library reports its own version, so a meta binary prints the tool's own
version. Two preconditions:

- Each tool must expose exactly one `main`-equivalent library function that does everything its
  `main.rs` does today. dagrun's `main.rs` also dispatches hidden helper commands that dagrun
  re-executes through its own binary path, and herdr-run's calls `cli::prepare_process()` before
  `cli::main`; a meta binary that skipped either would break those paths.
- The meta package must live outside the `rs/` workspace (its own workspace, as vibe-talk is).
  Inside it, two packages would write the same `rs/target/release/dagrun`, which Cargo warns is
  a collision and which `rs/bin/cargo-runner` relies on.

Dependencies pin exact versions (`dagrun = { version = "=0.16.0", path = "../rs/dagrun" }`), so
one meta version names one tested set.

```sh
cargo install agent-utils                                       # every command-line tool
cargo install agent-utils --no-default-features -F dagrun,tick-hub
cargo install agent-utils -F vibe-talk                          # adds the server
```

- Compile time: shared dependencies (serde, serde_json, regex, sha2, libc) are compiled once.
  Measured here (release builds from a cold target directory, 316 cores, load average about 40):
  dagrun, tick-hub, pr-landing-planner, herdr-run and agentctl built together in 41.7 s wall;
  built one at a time they took 12.2 + 4.2 + 7.3 + 6.7 + 42.3 = 72.7 s. On this many-core host
  the shared build is no slower than agentctl alone, so the difference is mostly parallelism; the
  part that carries over to small machines is that shared dependencies compile once, not five times.
- Features: default is every command-line tool; vibe-talk stays opt-in, because it brings an HTTP
  and TLS stack and a C build (oniguruma, bundled SQLite), and its 1,214-line `main.rs` must first
  move behind a library entry point.
- Version skew: none inside one install. The cost is that every tool release needs a meta
  release, and a user who also ran `cargo install dagrun` gets the overwrite refusal above.

**B. An installer command.** An `agent-utils install [TOOL...]` binary that runs
`cargo install <crate> --version =<pinned> --locked` per tool. Each tool upgrades on its own and
`cargo install --list` shows each, but shared dependencies are rebuilt per tool, it needs Cargo
and the network at run time, and it can only list what it installed.

Use A for the one-liner the owner asked for and keep the per-tool crates for people who want one
tool. B adds little once A exists. The Python tools (`wrkslots`, `wrkviz`) cannot be installed by
Cargo; `wrkslots` joins A when its Rust edition becomes primary (`#562 rust-primary-wrkslots`).
vibe-talk and the Rust wrkslots cannot both join one dependency graph yet: they bundle SQLite
through different `libsqlite3-sys` versions (0.38.2 and 0.35.0), and Cargo allows only one
package per `links = "sqlite3"` value. Align their `rusqlite` versions first.

## 4. Release procedure

**Versions.** Keep per-tool versions; do not move to a workspace version. They already mean
something: each matches its Python twin (`dagrun` 0.15.0 in both editions, and
`py/tests/test_herdr_run_golden.py` enforces it for herdr-run), and the dagrun documentation pins
`dagrun = "0.15"` (`py/tests/test_packaging_infrastructure.py` enforces that). The bump just before
release is one commit per release on `main` that raises the minor version of each released tool
in both editions, then `make validate`. Per tool it edits:

- `rs/<tool>/Cargo.toml` and `py/<tool>/pyproject.toml`;
- the Python `__version__` in `py/<tool>/__init__.py`;
- the dependency snippets in `common/docs/<tool>/fragments/rust/` (`dagrun = "0.15"`,
  `tick-hub = "0.2"`, `pr-landing-planner = "0.1"`), then re-renders the docs. Those rendered
  files are the crates' README and embedded guide, so they ship; only dagrun's snippet is tested
  today, and the other two should be added to that test before the bump.

The cross-language differentials compare `--version` output byte for byte (`cross/differential.py`
for dagrun, cpuset-alloc, tick-hub and pr-landing-planner; `cross/herdr_differential.py` for
herdr-run), so a bump that moves one edition alone fails the gate.

**Order.** Leaves first. Since cargo 1.90, `cargo publish -p A -p B ...` (or `--workspace`)
publishes in dependency order and waits for each crate to appear in the index, so the order
needs no script. For the first release the four tools have no internal dependencies at all. The
later agentctl release goes `agent-utils-queue`, `term-backend`, `chat-subscription`, then
`chat-subscription-plugin`, then `agent-utils-agentctl`; `agent-utils` goes last.

**Dry run (an agent can run this).**

```sh
python3 scripts/check_rust_packages.py --crate dagrun --crate tick-hub \
    --crate pr-landing-planner --crate herdr-run              # offline, isolated builds
with-proxy cargo publish --dry-run --locked --manifest-path rs/Cargo.toml \
    -p dagrun -p tick-hub -p pr-landing-planner -p herdr-run  # needs the index only
```

**Publish (the owner runs this).** In a clean checkout of the validated bump commit, after it is
on `main` on GitHub: each archive records its commit in `.cargo_vcs_info.json`, and the tags
point at it. If a multi-package publish stops partway, the crates already uploaded stay
published; fix the cause and re-run with only the remaining `-p` flags.

```sh
cargo login                       # once; token from crates.io account settings, scope publish-new + publish-update
cargo publish --locked --manifest-path rs/Cargo.toml \
    -p dagrun -p tick-hub -p pr-landing-planner -p herdr-run
for t in dagrun-v0.16.0 tick-hub-v0.3.0 pr-landing-planner-v0.2.0 herdr-run-v0.5.0; do
    git tag -a "$t" -m "$t"; done
git push origin dagrun-v0.16.0 tick-hub-v0.3.0 pr-landing-planner-v0.2.0 herdr-run-v0.5.0
cargo install dagrun --locked --root "$(mktemp -d)"   # then run its --version
```

Tags are `<crate>-v<version>` on the bump commit; they are not branches, so the no-branch-backlog
rule is unaffected. If the owner's shell also needs the proxy, prefix the cargo commands with it;
`curl -sS -o /dev/null -w '%{http_code}\n' https://crates.io/api/v1/crates/serde` should print
`200` first. This host's agents get 403 from that API and cannot use `cargo publish` through
the pane runner, and a publish token should stay out of agent environments in any case.

## 5. Proposed first release (awaiting the owner's OK)

| Crate | Version after bump | Installs |
|---|---|---|
| `dagrun` | 0.16.0 | `dagrun`, `cpuset-alloc` |
| `tick-hub` | 0.3.0 | `tick-hub` |
| `pr-landing-planner` | 0.2.0 | `pr-landing-planner` |
| `herdr-run` | 0.5.0 | `herdr-run` |

Not in it: the agentctl family (rename and scrub first), vibe-talk (packaging fixes first), the
meta package (worth publishing once it covers agentctl), and the unpublished tools.

## Open questions for the owner

1. Approve the four crates and the minor-version bumps above, or name a different list.
2. Approve `agent-utils-` as the prefix for taken names (with the package-check exception it
   needs, section 2), and whether the agentctl libraries (`term-backend`, `chat-subscription`,
   `chat-subscription-plugin`) also take it.
3. Should agentctl ship the managed-runtime adapter at all, or only behind a non-default feature?
4. Can the owner's shell reach the crates.io API, and should anyone else co-own the crates
   (`cargo owner --add`)?
