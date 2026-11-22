---
title: 'herdr-runtime-state-out-of-synced-config: keep herdr''s session, log and socket files out of the synced config directory'
status: open
priority: 2
issue_type: task
labels:
- herdr
created_at: 2026-11-22T06:30:00-05:00
updated_at: 2026-11-22T06:41:00-05:00
---

# Description


Herdr writes session-history.json (hundreds of MB, rewritten continuously), session.json, its client and server logs and its sockets into its config directory. When that directory is synced between hosts, both hosts write the same files and the sync tool makes conflict copies (thousands so far). Only config.toml should be shared.

# Design


Phase 1 (recommended, sufficient): exclude these herdr runtime paths from sync; keep config.toml and release-notes.json synced. Exclude: `.config/herdr/session.json`, `.config/herdr/session-history.json`, `.config/herdr/session-snapshots/`, `.config/herdr/sessions/`, `.config/herdr/herdr-server.log`, `.config/herdr/herdr-client.log`, `.config/herdr/herdr.log`, `.config/herdr/herdr.sock`, `.config/herdr/herdr-client.sock`, `.config/herdr/.plugins.lock`.

`sessions/` covers every named session's json/history/logs/snapshots. Socket entries are belt-and-braces. Phase 1 takes effect at the next sync cycle: no herdr restart, no socket move, no tool changes. Session history still grows on local disk as today; it simply stops being synced, which is the reported harm. No sync-tool command has been run by the investigating lane; phase 1 is with the owner for approval because the path list is shared owner config.

Phase 2 (optional, only if sockets must also physically leave the config dir): at the next natural server restart, add a drop-in to the server unit setting `HERDR_SOCKET_PATH` and `HERDR_CLIENT_SOCKET_PATH` under the per-host runtime dir (`/run/user/<uid>/herdr/…`, never synced, self-cleaning). First re-verify on a throwaway whether the server creates a missing socket parent directory. Phase 2 also requires the tab-order fallback change, both herdr-run editions' env pass-through, unit env for watch/restore/bridges, and shell env for interactive attach. Session files remain in the config dir, so phase 1 is still required; phase 2 alone fixes nothing about the history file.

Rejected: moving the config dir via `XDG_CONFIG_HOME` for the server (moves config.toml out of sync, breaks session discovery for outside clients, messy rollback); `HERDR_CONFIG_PATH` relocation (verified: moves only the file, not the state).

Order (both hosts): (1) first host: add the 10 excludes with the sync tool, verify that all 10 landed in its exclude list and config.toml/release-notes.json were not excluded; (2) wait one sync cycle, confirm no new herdr conflict files; (3) second host: change nothing by hand, verify after its next sync that the shared list shows the same excludes and no new conflicts appear; (4) deleting leftover conflict copies is a separate owner-approved cleanup, not part of this change. Phase 2, if approved, is per host at that host's next natural restart (never forced), code/unit changes landed first, verified from a pane, a plain shell and `herdr-run` before the host is declared done. Hosts can proceed in either order; they do not share sockets.

Rollback: phase 1: re-include any excluded path with the sync tool; it takes effect at the next sync, no restart involved. Phase 2: delete the drop-in and revert the fallback/pass-through changes at the next release; the server returns to config-dir sockets at its following restart. Session files never move in either phase, so no data migration exists in either direction.

Risks: exclude-addition semantics for a subpath of the included `.config` tree are inferred from the existing exclude list's shape, not executed; verifying the exclude list after the first additions is the gate. After exclusion each host keeps its own `session.json` (intended: layout restore becomes per-host); the owner should confirm no workflow depends on layout syncing. A future herdr update could change path behaviour; re-run the throwaway check after any version bump before relying on phase 2. Staying synced, `release-notes.json` lets two hosts overwrite each other's startup flag; exclude it too if strictly-local state is preferred.

# Acceptance Criteria


- A written plan with the mechanism verified on a throwaway herdr instance, approved before any change to the live hosts.

# Notes


Filed by the coordinator from a consumer's report; refined in place by the investigating lane with the verified plan above. Awaiting owner approval of phase 1; no live change has been made. The earlier blanket statement that only config.toml should stay synced is refined here: release-notes.json also stays synced unless the owner directs otherwise.

## Investigation (verified 2026-11-22)


Version note: the installed binary file is named for 0.9.3 but reports 0.9.5 (`--version`, and release-notes.json in the config dir says 0.9.5); it appears to have self-updated in place. All findings were verified against the actual binary on throwaway instances with isolated home, config and runtime directories, each stopped by its literal PID. The live server, its files and sockets were never touched.

Path model:

| Path | Location | Controlled by |
|---|---|---|
| config file | `~/.config/herdr/config.toml` | `HERDR_CONFIG_PATH` overrides the file only |
| config dir | `$XDG_CONFIG_HOME/herdr`, else `~/.config/herdr` | `XDG_CONFIG_HOME` or `HOME` |
| default session dir | the config dir itself | — |
| named session dir | `<config dir>/sessions/<name>/` | `--session <name>` / `HERDR_SESSION` |
| session state | `session.json`, `session-history.json`, `session-snapshots/` inside the session dir (inside the synced config dir: the defect) | no env or config override exists |
| logs | `herdr-server.log`, `herdr-client.log` (help also names `herdr.log`) inside the session dir | follows session dir |
| sockets (default) | `herdr.sock`, `herdr-client.sock` inside the session dir | follows session dir |
| sockets (override) | anywhere | `HERDR_SOCKET_PATH`, `HERDR_CLIENT_SOCKET_PATH` env on the server process |
| state dir | `$XDG_STATE_HOME/herdr`, else `~/.local/state/herdr` (only `agent-detection/`, `client-shell/`) | `XDG_STATE_HOME` honoured, but session state does not go here |
| runtime dir | — | `XDG_RUNTIME_DIR` is ignored (verified: dir stayed empty, sockets stayed in config dir) |

Verified negatives: no config.toml key for session/state/socket/log location (full `--default-config` dump checked; only `[worktrees] directory` exists); `HERDR_CONFIG_PATH` does not relocate session data; no state-dir env override. The server injects `HERDR_SOCKET_PATH`, `HERDR_CLIENT_SOCKET_PATH`, `HERDR_BIN_PATH` and session/pane IDs into every pane it spawns, so pane children follow the server wherever its socket is. Throwaway evidence: with socket overrides, sockets were created at the overridden paths, `session.json` plus a snapshot appeared in the named session dir after workspace creation, `agent-detection/status.toml` appeared under the XDG state dir, and sockets were removed on clean shutdown.

Live inventory (read-only): the config dir holds `config.toml` and `release-notes.json` (keep synced) plus runtime files: `session-history.json` (~237 MB), `session.json`, `session-snapshots/`, `sessions/`, `herdr-server.log`, `herdr-client.log`, both sockets and `.plugins.lock`. The server runs from a user systemd unit installed by `agentctl install-units` (login-shell `server` command, `KillMode=process`).


## Sync-tool findings (read-only)


The hosts' dotfile sync tool manages a shared include list plus a per-path exclude list (verified read-only): the whole `.config` tree is included, and runtime files of other tools are already excluded per-path by precedent (for example other tools' session dirs and state files). Excluding a new runtime path is a one-line addition to that exclude list. Nothing under `~/.local/state/herdr` needs action: that tree is not in the include list at all, so the state dir is already unsynced. The path list is shared between the hosts, so one host's change is the set's change and the other host picks it up on its next sync. Only 8 herdr conflict files remained locally at investigation time; the larger historical total is the reporter's figure. The exact commands for the owner's sync tool live in the lane's private handoff, not here.


## Agent-utils audit


| Consumer | How it finds herdr | If the socket moves (phase 2) |
|---|---|---|
| `agentctl` (most paths) | shells to the herdr binary (`--herdr-bin`); the binary resolves `HERDR_SOCKET_PATH` else its config-dir default | needs the new socket in its environment |
| `agentctl` tab order (`rs/agentctl/src/tab_order.rs`) | `$HERDR_SOCKET_PATH`, else hardcoded `~/.config/herdr/herdr.sock`; also checks `HERDR_BIN_PATH` | code change: update the hardcoded fallback |
| `herdr-run` (Rust `rs/herdr-run/src/client.rs` and Python mirror `py/herdr_run/client.py`) | fixed home candidates for the binary; invokes the herdr CLI via `systemd-run` setting only `HOME` (plus worker threads); can auto-start a server unit the same way | code change (both editions): pass the socket env through, or it misses the moved server and auto-starts a second one at the old socket |
| foreign/subagents backend (`py/agentctl/foreign/lib.py`) | env-only: `HERDR_ENV` + `HERDR_SOCKET_PATH` existence check, then `herdr status` | fine inside panes (server injects env); outside panes needs env supplied |
| `herdr-agent`, `herdr-subagents`, `herdr-chat`, chat bridges, watchers | drive the herdr CLI / `agentctl --herdr-bin`; no socket path of their own | inherit whatever env their unit or shell provides |
| systemd units from `agentctl install-units` (`rs/agentctl/src/units.rs`: server, watch, restore, bridge drop-in) | run outside panes, so no injected env; rely on the binary default | unit change: add socket `Environment=` to generated units and reinstall, or set the user-manager environment |
| restore (`agentctl restore`, restore-drill tests) | via `--herdr-bin` wrapper plus injected `HERDR_SOCKET_PATH`; one test references the live default socket path | test fixture update only if the default moves |

Phase 1 needs zero changes above: nothing moves on disk.
