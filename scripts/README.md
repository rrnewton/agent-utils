# Repository checks and documentation tooling

These scripts enforce contracts that span otherwise independent packages:

They are repository workflow, evidence, and acquisition helpers rather than commands in the
independently installable paired-tool inventory. The agent-log archive fetcher is acquisition
support for the explicitly Python-only timeline/trace family and is intentionally not exposed
through the top-level `bin/` command surface while that family is still evolving.

| Script | Purpose |
| --- | --- |
| `embed_userguides.py` | Render shared documentation and verify package links and standalone prose. |
| `check_deps.py` | Smoke every command in a minimal environment to catch import-time dependency failures. |
| `check_no_any.py` | Reject explicit `typing.Any` use at typed boundaries. |
| `check_python_packages.py` | Build each wheel and sdist, rebuild from the sdist, then inspect, install, and smoke every Python distribution in isolation. |
| `check_rust_packages.py` | Package and inspect each Rust crate in isolation. |
| `agent-log-archive/fetch_agent_logs.py` | Strict-typed, non-deleting local/SSH archive fetcher with manifests, metrics, and immutable receipts (`.sh` is its compatibility launcher). |
| `irq_survey.py` | Re-derive the per-CPU interrupt numbers the core allocator's ranking rests on: is there a signal, does it change placement, and how far does an independent window drift. |
| `rebase-delta-guard` | Bracket a rebase and prove its complete text and binary delta is unchanged. `record` captures the exact intended onto SHA; recorded series must be linear and descend from their base, while `check` accepts four explicit snapshot revisions. During a stopped rebase, use `conflict-files` instead of raw `git diff --name-only --diff-filter=U`: it reads index modes and omits conflicted submodule gitlinks before file-oriented tools can recurse into them. |

Run the focused checks directly:

```sh
python3 scripts/embed_userguides.py --check
python3 scripts/check_deps.py
python3 scripts/check_no_any.py .
python3 scripts/irq_survey.py distribution
make check-packages
```

Publication is ordinary git — `git pull --rebase origin main && git push origin
main`. `main` is protected server-side by a GitHub ruleset (no deletion, no
force-push, no merge commits); see `AGENTS.md`.

Use `make check`, `make test`, and `make cross` for the wider repository contract.
