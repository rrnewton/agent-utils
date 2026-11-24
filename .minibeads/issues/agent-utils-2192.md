---
title: 'agentctl-harness-shell-tool-sandbox: sandbox only the harness shell tool, not the whole harness'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- harness
- sandbox
assignee: mu-bashbox
created_at: 2026-11-24T05:41:03-05:00
updated_at: 2026-11-24T05:41:03-05:00
---

# Description

Masking or boxing the whole harness fails for Claude Code and Codex, because their launchers run a setuid helper that dies inside a user namespace (setresuid EINVAL; see the harness support matrix in `common/docs/agentctl/harness-support-matrix.md`, Boxed launch row). The owner direction (2026-11-24) is to keep looking for a combination that allows starting agents while sandboxing their file-system mounts, and to try per-harness support for keeping the harness bash tool always running in a desired sandbox while the harness itself runs outside the box. This issue prototypes that split on one throwaway test lane: the harness runs unboxed, and only its shell tool is wrapped into a wrkslots slot view.

# Design

Prototype only, on one throwaway test lane launched in a test registry (never a live lane). The harness stays outside the box; its shell tool runs inside `wrkslots run <SLOT> --isolation <mode> -- bash -c '<command>'`. Claude Code uses a PreToolUse hook on Bash returning updatedInput with that wrap, plus permission rules limiting Edit and Write to the slot. Codex uses its own sandbox settings pointed at the same view (workspace-write roots or the equivalent). Muse uses whatever hook it offers; if none, that is recorded as the finding. Isolation mode is userns (no root) first; root mode is used only through wrkslots' own helper invocation and only after the root-isolation box work reports it working. The write-up designs agentctl profile options (opt-in per profile) for a follow-up, but implements only what the prototype needs. The test lane keeps MCP servers off (strict MCP with an empty config) so no build daemons start in the owner's checkout, and uses per-test config directories instead of any shared config.

# Acceptance Criteria

- Claude Code prototype: a PreToolUse hook on Bash wraps each shell command into the slot view, verified against the installed version's hook format, with Edit and Write limited to the slot by permission rules.
- Codex prototype: sandbox settings pointed at the same slot view (or a recorded finding that the equivalent does not exist), and Muse: its hook used, or a recorded finding that it offers none.
- Three checks per harness, with evidence: a write outside the slot fails via the shell tool and via the edit tool; normal build and test commands (cargo build and pytest of a small project inside the slot) succeed; per-command overhead over N=20 runs reported as median and p90 in CPU time and wall time.
- The design is written up as opt-in agentctl profile options for a follow-up; no profile changes land on live lanes, and the harness support matrix is updated in the same commit as any code that changes a cell.
- Which isolation modes work per harness is stated from measurement (userns first; root only once the root-isolation box works, via wrkslots' helper, with no direct sudo).

# Notes

[watermelon-full-preview] mu-bashbox. Number pre-assigned by the coordinator. Owner priority (Slack 2026-11-24): keep looking for a combination that allows starting agents while sandboxing their file-system mounts; it would not matter where the agent harness itself runs if its bash tool always ran in a desired sandbox, with per-harness support for that. Prior work: the harness support matrix Boxed launch row (whole-harness userns boxing fails for Claude Code and Codex on the launcher setuid step; Muse boxed form works; root-isolation relay is the current mount-sandboxed route), and the wrkslots sandbox modes (userns, root, cgroup) in `py/wrkslots/sandbox.py`. Related root-isolation box work is tracked separately; this prototype uses userns first and switches to root mode only when that work reports working.

Prototype outcome (2026-11-24, throwaway lane, userns only; full evidence in `ai_docs/reports/research/2026-11-24-harness-shell-tool-sandbox.md`): Claude Code and Codex both confine shell and edit tools to the slot view via a PreToolUse `updatedInput` wrap plus edit denies (cargo and pytest pass inside); Codex 0.170 edits surface only as `exec` JS, so the matcher must include `exec` or edits escape. Muse 1.4.27 has no tool hook found and its native shell sandbox did not confine writes outside its workspace: finding `none`. Per-command cost of a fresh `wrkslots run --isolation userns` is a median 5.7 s wall / 4.5 s CPU (N=20), which blocks shipping per-call wrapping; the follow-up needs a persistent-view entry path. Root mode remains untested pending the root-isolation box signal. This issue stays open for: the persistent-view prerequisite, root-mode retest, and the opt-in profile options designed in the report. No live-lane profiles changed and no shipped code changed, so no harness-support-matrix cell moves yet.
