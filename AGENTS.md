# Agent Instructions

## Publication Policy

Routine changes to this repository go directly to `rrnewton/agent-utils:main`.
Do not open a pull request merely out of habit: a PR queue is an unowned wait
state, and this repository must remain linear and current for its consumers.

Serialize writers. Before changing or publishing anything, establish that no
other agent is currently landing agent-utils work. Fetch `origin/main`
immediately before the commit, push only a fast-forward update with an explicit
`HEAD:refs/heads/main` refspec, then fetch again and ancestry-verify the pushed
SHA. Never force-push, rewind, amend shared history, or bypass a rejected
non-fast-forward push.

Direct-to-main does not mean unvalidated-to-main. Before every push, run the
repository contract, including the Python/Rust behavioral cross-check:

```bash
python3 scripts/embed_userguides.py --check
cargo fmt --all --manifest-path rs/Cargo.toml -- --check
make both
make check
make test
python3 -m mypy cross/differential.py
python3 cross/differential.py --tool safe-ci-dag-runner
make cross
make check-packages
```

A PR is allowed only for one of these explicit reasons:

1. a genuinely high-risk change needs pre-main review; or
2. the change must be reviewed atomically with an in-flight consumer change.

Record that reason on the PR. Keep at most one exceptional PR open, and land or
close it before opening another.

## Repository Boundary

This direct-main policy applies to `agent-utils` and to parent-only tooling in
`dev-hermit`. It does **not** apply to the Hermit product repository.
`rrnewton/hermit:main` is protected by a pull-request and required-check
ruleset, and Hermit changes must follow `hermit/AGENTS.md`: feature branch,
pull request, required review and checks, then the permitted merge path. Never
use an admin bypass, force-push, or direct push to reinterpret this policy as a
way around Hermit's ruleset.
