# Agent Team Timeline browser checks

This directory runs the production timeline assets against a small, deterministic
Codex-team fixture. The fixture server uses only Node's standard library and exposes
no repository files outside `py/agent_team_timeline/static/`.

Install and run from this directory:

```bash
npm install
npx playwright install chromium
npm test
```

Use `npm run test:headed` while iterating. The browser download and `node_modules/`
are intentionally untracked; this directory does not vendor generated npm or browser
artifacts.

The suite treats DOM observability as a small UI testing contract. A check skips with
a precise reason until its contract exists, then enforces the behavior. Production UI
code should expose:

- `[data-testid="timeline"]` with `data-view-start-ms`, `data-view-end-ms`,
  `data-selection-scope`, `data-selected-agent-id`, and
  `data-selected-phase-id` as applicable;
- phase groups with `data-phase-id` and `data-agent-id`;
- `#timeline-svg` with `data-track-mode="packed|per-agent"` and a numeric
  `data-lane-count`;
- `[data-testid="timeline-context-menu"]`, with actions using `role="menuitem"`;
- `[data-testid="transcript-role-filters"]`, and transcript cards with a normalized
  `data-role` (`user`, `assistant`, `agent`, `tool`, `system`, or `other`).

The fixture intentionally has four agents: one coordinator, two overlapping agents,
and a third agent whose lifetime starts exactly when the first ends. That makes the
packed-lane assertion cover both overlap and half-open lifetime reuse.

