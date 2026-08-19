# Agent Instructions

## Publication Policy

Routine changes to this repository go directly to `rrnewton/agent-utils:main`.
Do not open a pull request merely out of habit: a PR queue is an unowned wait
state, and this repository must remain linear and current for its consumers.

**Use ordinary git.** There is no wrapper, no publication script, and no
repository-installed hook standing between you and `main`:

```bash
git pull --rebase origin main
git push origin main
```

If the push is rejected as non-fast-forward, someone landed while you were
working: `git pull --rebase` and push again. Never force-push, never rewind,
never amend shared history, and never reach for `--no-verify` to get past a
refusal.

**Keep `main` linear.** Rebase; do not merge. `git pull --rebase` (not a bare
`git pull`) is the habit that makes this automatic — a merge commit on `main`
is rejected by the server, so a bare pull will simply waste your time.

### Protection lives on GitHub, not in this checkout

`main` is protected by a repository **ruleset** — `main history protection`,
active on the default branch with no bypass actors — enforcing:

| Rule | Effect |
|---|---|
| `deletion` | `main` cannot be deleted |
| `non_fast_forward` | no force-push, no rewind |
| `required_linear_history` | no merge commits |

That is server-side, so it holds on **every** path — every clone, every host,
every agent, the web UI, and the REST API alike. This is deliberately different
from the client-side guard that used to live here: a hook only binds checkouts
that installed it, so it gave the appearance of enforcement while leaving the
web UI, the API, a fresh clone, and `--no-verify` wide open. Real protection
belongs where it cannot be opted out of.

There is deliberately **no** `required_status_checks` rule, because that would
block the direct-to-main workflow this repository is built around. CI is a
signal for a human, not a gate.

### Validate before you push

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

### When a PR is warranted

A PR is allowed only for one of these explicit reasons:

1. a genuinely high-risk change needs pre-main review
   (`Exception-Reason: high-risk-preland-review`); or
2. the change must be reviewed atomically with an in-flight consumer change
   (`Exception-Reason: atomic-consumer-change`).

Record that reason on the PR **as its own line** in the description, using the
exact slug above — prose that merely mentions the reason is not a recorded
reason, because nothing can read it. Keep at most one exceptional PR open, and
land or close it before opening another.

## Repository Boundary

This direct-main policy applies to `agent-utils` and to parent-only tooling in
`dev-hermit`. It does **not** apply to the Hermit product repository.
`rrnewton/hermit:main` is protected by a pull-request and required-check
ruleset, and Hermit changes must follow `hermit/AGENTS.md`: feature branch,
pull request, required review and checks, then the permitted merge path. Never
use an admin bypass, force-push, or direct push to reinterpret this policy as a
way around Hermit's ruleset.

## Naming: every feature gets a slug

Every issue and pull request names its subject with a **slug**: lowercase
letters, digits, and hyphens, nothing else. No spaces, no underscores, no
capitals, typically two to four words.

    voice-page-safe-area
    elevenlabs-mock
    container-lifecycle
    launcher-preflight

**Put the slug first in the title**, then a plain sentence saying what is
wanted:

    voice-page-safe-area: honour the notch inset on a physical phone
    elevenlabs-mock: a local agent that really calls the MCP tools

**One slug per feature, and it does not change.** The issue, the branch, the
pull request, and any follow-up issue all reuse the slug it was given first.
That is the whole point: the slug, not the number, is what a human recognises
months later, and it is what makes two references to the same work visibly the
same work.

### Always say the number AND the slug

When writing to the owner, in a comment, or in a commit body, refer to work as
`#<number> <slug>` — both, every time:

    #123 voice-page-safe-area

Not `#123` on its own, not `PR 123`, not `pr123`, and not "the hover issue".
A bare number is unreadable without opening it, and a bare description cannot
be looked up. The pair is short enough that there is no reason to abbreviate
it, and it survives being quoted out of context — which is exactly where the
reference is most needed and least recoverable.

This applies to chat with the owner as much as to anything written down. An
agent that reports "landed the fix for 123" has told the reader nothing they
can act on without a round trip.

### What this does not change

Commit **titles** keep the existing `<project>: <sentence>` form —
`gent-talk: make /voice read as an application`. They are not slugged, because
the project prefix already does that job and commit titles should stay
readable as prose. When a commit closes or advances an issue, name it in the
**body** as `#<number> <slug>`, alongside the agent identifier that
`gent-talk/AGENTS.md` already requires.
