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

`scripts/main_write.py` performs that whole sequence and is the sanctioned
publication path. Run `install-hooks` once per checkout; after that a bare
`git push` to `main` is refused unless it is running inside a serialized
`publish`:

```bash
python3 scripts/main_write.py install-hooks   # once per checkout
python3 scripts/main_write.py status          # is the queue clear? exit 0 = yes
python3 scripts/main_write.py publish         # lock, fetch, CAS, push, re-verify
```

`publish` takes a host-wide exclusive lock, fetches `origin/main`, refuses
anything that is not a fast-forward from that exact value, pushes, then
re-fetches and proves the pushed commit is contained. The pre-push hook
independently requires a live receipt from that operation: the lock must still
be held, the holder must be an ancestor of the push, and the remote must still
be at the value the holder fetched. Pushes to any ref other than
`refs/heads/main` are ordinary work and pass through untouched.

**Know what this does not cover.** It is a client-side control. It covers
`publish` and a bare push from any checkout that has run `install-hooks` — and
because the lock is host-wide per uid, every hooked checkout on one machine
contends on one lock. It does **not** cover `git push --no-verify`, a clone
that has not run `install-hooks`, another host, or the GitHub web UI and REST
API. Server-side, `main` carries anti-rewind rules only (`deletion`,
`non_fast_forward`, `required_linear_history`) with no required status check,
deliberately, so that direct-to-main keeps working. History loss is therefore
blocked on every path while serialization is enforced only on the ones listed.
Do not describe the remaining paths as closed.

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

1. a genuinely high-risk change needs pre-main review
   (`Exception-Reason: high-risk-preland-review`); or
2. the change must be reviewed atomically with an in-flight consumer change
   (`Exception-Reason: atomic-consumer-change`).

Record that reason on the PR **as its own line** in the description, using the
exact slug above — prose that merely mentions the reason is not a recorded
reason, because nothing can read it. Keep at most one exceptional PR open, and
land or close it before opening another.

`python3 scripts/main_write.py pr-exceptions` checks both halves of that rule
against the live open-PR list (exit 0 satisfied, 1 violated, 2 could not read —
never treat 2 as satisfied). It is deliberately **not** a precondition of
`publish`: an unrelated PR-hygiene lapse must not block a legitimate
publication. The `pr-exceptions` workflow runs it on a schedule, and it is
advisory rather than a required check, because `main` carries no
`required_status_checks` rule.

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
