# herdr-run — related work

Searched 2026-08-06 before any design work, per the repository convention that a new utility must
first establish what already exists. Four places were checked: our own ad-hoc Herdr usage, Herdr's
own command-launch surface, our own terminal-scraping code, and the public ecosystem. Each finding
below is recorded with what was **adopted**, **rejected**, or **left alone**, so a later reader can
tell which parts of this tool are novel and which are reuse.

---

## 1. Our own ad-hoc Herdr usage — a downstream CI harness

The closest prior art, and the reason this utility is shaped the way it is.

`ci-hub validate-run` creates an owner-visible Herdr tab for each boxed validation run
(`ai_docs/ci-hub-owned-validate-panes-20260806.md`). It maintains one stable workspace
(one tab per validation run), creates a titled tab per run, and wraps **every** Herdr control call in
`systemd-run --user --wait --pipe --collect --quiet --setenv HOME=... --setenv PATH=...`.

**Adopted:**

- The stable-workspace-plus-per-unit-tab layout. `herdr-run` uses the same shape with one tab per
  *agent* rather than per run, because a shell pane is reusable in a way a log tail is not.
- The `systemd-run --user` escape itself — but for exactly one call, see below.
- The refusal to make the pane the producer of anything authoritative.

**Rejected — brokering every call through `systemd-run`.** Measured on `devbig014` 2026-08-06:
`herdr status --json` returns byte-identical JSON whether invoked directly from inside the agent
jail or through the `systemd-run --user` wrapper. The Herdr server's unix socket
(`~/.config/herdr/herdr.sock`) is reachable from inside the sandbox, so the broker costs a transient
unit per call (tens of ms, plus a unit name in the journal) and buys nothing. `herdr-run` therefore
defaults to `broker: direct` and keeps `broker: systemd-run` as a per-project config option for
hosts where the socket is not reachable.

**Kept as a hard requirement — brokering *server start*.** This is the one place the broker is
load-bearing, and it is a correctness issue rather than a preference. Panes are children of the
Herdr **server**. If a confined agent starts the server itself, every pane it creates inherits that
confinement, and the tool becomes an elaborate way to reproduce the same `403` — *silently*, because
nothing about such a pane looks different from a good one. `HerdrClient.ensure_server` therefore
always launches through `systemd-run --user`, which reparents the server onto the user manager
outside the jail. `herdr-run doctor` exists to catch the case where a bad server is already running.

**Not touched.** `pane_owner.py` stays as it is. It is validate-specific (it binds panes to
`validate-*.service` units, durable logs, and `safe-ci-*` cgroup discovery), and folding it into a
generic runner would drag validate's admission policy into a general utility.

---

## 2. Herdr's own launch functionality — does it beat raw send-keys?

**Yes, and it is used.** Enumerated from `herdr --help` and each subcommand's `--help`
(herdr 0.7.5, protocol 17):

| Surface | Verdict |
| --- | --- |
| `herdr pane run <PANE_ID> <COMMAND>...` | **Adopted.** Types and submits a command line as one operation. Strictly better than `pane send-text` + a synthetic `Enter`: no key-encoding, no bracketed-paste behaviour to reason about, no partial-line state if the caller dies mid-send. |
| `herdr pane process-info` | **Adopted as the primary readiness signal.** Returns `shell_pid`, `foreground_process_group_id`, and the foreground process list. |
| `herdr pane wait-output --match/--regex --timeout` | **Rejected for results, see §4.** Matching a sentinel on screen re-inherits every terminal-scraping problem the file-based design avoids. |
| `herdr pane read --source visible\|recent\|recent-unwrapped` | **Adopted, narrowly** — only for the prompt-dirt veto, never for results. |
| `herdr workspace/tab create --label --cwd --env --no-focus` | **Adopted** for idempotent bring-up. `--no-focus` matters: bring-up must not yank the human's focus. |
| `herdr agent *` (`start`, `prompt`, `wait`, `explain`) | **Rejected.** This family models *interactive AI agent panes* (lifecycle states, prompt submission), not shell command execution. Wrong abstraction. |
| `[[keys.command]]` with `type = "pane"` in `config.toml` | **Rejected.** Herdr can bind a command to a keystroke and open a temporary pane for it, which is the closest thing Herdr has to a built-in launcher — but it is keybinding-driven, has no programmatic invocation, and returns nothing to a caller. |

Net: Herdr supplies launching, reading, and process introspection. It supplies **no** request/response
execution primitive — nothing returns a command's exit code. That gap is what this utility fills, and
it is filled with files rather than with screen matching.

---

## 3. Our own terminal-scraping prior art — a coordinator-messaging script

Raw `tmux send-keys` delivery into the ORC coordinator TUI. Not a Herdr consumer, but the closest
prior art for *"decide whether a terminal is ready to receive input"*, and mostly a cautionary tale.

Its readiness check keys on rendered TUI text, and the documented failure modes are exactly the ones
a prompt-regex readiness check would hit here:

- The composer's border title **changed between Orc builds** (`Input (Enter, …)` →
  `Paste not available here` → back), so any check keyed on it breaks on upgrade.
- A pane-selection hint that could only ever match panes running `orc` was pointed at agent panes,
  matched the empty set, and the resulting "found 0 coordinators" was misread as an outage. A
  *filtered* view was mistaken for the *true* state.

**Adopted:** the lesson, not the code. Readiness here keys primarily on the pane's foreground
process group — an observable property of the running process, invariant under prompt themes, colour,
shell upgrades, and redraw quirks. The rendered-text check is demoted to a **veto that must produce
positive evidence of dirt**, and reports `abstain` when it cannot understand the prompt rather than
guessing in either direction. `meta.json` records which verdict each signal gave, so a run never
implies a check that did not actually happen.

**Not extended.** That script messages one coordinator TUI specifically; there is still
no supported agent-to-agent relay, and this tool does not become one. It runs shell commands.

---

## 4. Why results are files, not screen scrapes

`herdr pane wait-output` plus a sentinel would have been the obvious way to build this. It was
rejected on four independent grounds, each of which is enough on its own:

1. **Wrapping.** A pane is ~80 columns and hard-wraps. Captured live during this search, a single
   `git ls-remote` line came back from `--source visible` split across five rows mid-token
   (`https://github.com/rrnew` / `ton/<repo>`). `--source recent-unwrapped` reassembles it, but that
   is a rendering mode, not a guarantee.
2. **Truncation.** Scrollback is finite. Long output (`git log`, `gh pr list`) is silently cut, and
   silent truncation reads as a complete result.
3. **Corruption.** Progress meters, `\r` redraws, and colour all land in the text. `git`/`gh` write
   plenty of both.
4. **No exit code.** The decisive one. A command's exit status is nowhere on screen, so a scraper
   must *infer* success from output shape — a textbook proxy. `git push` printing text is not
   `git push` succeeding.

The filesystem is shared across the sandbox boundary, so the pane can simply redirect:

```sh
{ cd <cwd> && <command> ; } >stdout 2>stderr; printf '%s\n' "$?" >exit_code
```

The caller then reads exact bytes and an exact integer. Verified end to end during this search:
in-jail `with-proxy git ls-remote https://github.com/<org>/<private-repo> main` fails with `CONNECT tunnel
failed, response 403`; the identical command through a pane wrote `rc=0` and the real SHA
`2c54dfb5dc9f49423c26fc87c840ebf26570b737`.

Terminal reading survives in exactly one place — the prompt-dirt veto — where the question genuinely
is "what does the screen look like", and where being wrong costs a false refusal rather than a wrong
answer.

---

## 5. Public / web prior art

Searched for existing tools that let a sandboxed coding agent execute commands in an out-of-sandbox
terminal. Nothing reusable; the surveyed projects solve adjacent but different problems.

| Project | What it is | Why it does not apply |
| --- | --- | --- |
| [flowmux](https://github.com/grouzen/flowmux) | Terminal-native AI agent multiplexer | Runs *agents* in panes. No command-execution API, no result capture, no policy layer. |
| [cmux](https://dev.to/neuraldownload/cmux-the-terminal-built-for-ai-coding-agents-3l7h) | Terminal built for running AI agents in parallel | Same category as Herdr itself — it is the substrate, not the utility over it. |
| [agentvm](https://github.com/Gitlawb/agentvm) | Pure-bash CLI running agents in parallel isolated environments | Isolation *per agent*; does not address executing out of an existing sandbox. |
| [ai-sandbox-devcontainer](https://github.com/ComposableSecurity/ai-sandbox-devcontainer) | Sandboxed devcontainer for AI agents | Solves the inverse problem — tightening confinement, including egress allowlists. Useful as a reminder that the boundary this tool crosses is one others deliberately build. |

The general pattern — "agent multiplexer" — is well populated. The specific pattern — *a narrow,
allowlisted, audited door through an agent's own confinement, using an out-of-sandbox terminal as the
transport* — has no obvious public implementation. Absence of a match is a weak signal (the search
was one pass over public results), but there was nothing to adopt.

---

## Summary of what is genuinely new here

Herdr already provides the transport (`pane run`) and the process introspection (`process-info`).
The parts this utility adds are:

1. **The allowlist**, applied to split argv with every token re-quoted, so injection is prevented by
   construction rather than by a metacharacter blocklist.
2. **File-based result capture**, giving exact stdout/stderr and a real exit code.
3. **Two-signal fail-closed readiness**, with the weak signal restricted to vetoing.
4. **Idempotent bring-up** with a cache that is always re-validated against the live session.
5. **An audit trail that records refusals**, not only successes.
