---
title: 'vibe-talk-screenshot-harness-real-states: shared screenshot harness serves real Discuss, really shrinks the keyboard, and settles transient UI'
status: closed
priority: 0
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T12:00:00-05:00
updated_at: 2026-11-18T12:00:00-05:00
---

# Description

[watermelon-full-preview] Owner QA direction (thread 185: dynamic testing; screenshots must show real states) and repeated liaison findings across many lanes' screenshot sets:

1. Matrix screenshot fixtures show "HTTP 404 not_found: not served by this check" in the Discuss pane in nearly every shot. Per-test browser fixtures answer unhandled routes with that 404, and lane matrix scripts copy the pattern, so Discuss previews/summaries never really answer in the shots reviewers are shown.
2. Lanes keep producing "keyboard open" shots that are not really shrunk (identical to keyboard-closed).
3. Shots leave transient UI open (the Voice/Channel/Documents switcher popup over Pace/Read, stray toasts).

Prior work to build on: the fake chat upstream now serves timeline reads and the fake chat router is keyed as the provider it stands in for, so a real server booted with the fake upstream answers channel reads end to end. The shared screenshot harness (`vibe-talk/scripts/screenshots.py`, launched by `vibe-talk/scripts/run.sh --screenshots`) already boots a real server that way, but it has no Discuss state, no keyboard state, and no settle step, so lanes keep writing their own matrix scripts with faked routes instead.

# Design

- Extend the shared harness rather than adding another copy. `run.sh --screenshots` additionally starts a loopback model stub (`vibe-talk/scripts/model_stub.py`: one plain HTTP endpoint answering the Messages API shape) and configures the throwaway server with a summaries model section pointing at it, so Discuss preview and Summarize answer through the real server router with only the external model stubbed. The fake upstream serves the channel timeline; no page route is intercepted for Discuss.
- One shared keyboard helper in the harness: shrink by setting the viewport to height minus 336 and wait until the visual viewport height equals the shrunk height (condition wait only). The standard matrix scenes call it, and a run FAILS by name when a keyboard-open shot's viewport height equals the closed one's.
- A settle step in the harness closes popups (the view switcher cards, open menus) and lets transient status clear before capture, unless the shot targets that transient UI (a scene flag says so).
- New standard matrix scenes in the shared scene table: Discuss open with a real preview, Discuss summarized with the stub model's real answer, and keyboard open/closed pairs, at the Android phone width and the small laptop width. A short doc in the harness README says how lanes produce the standard matrix with these scenes instead of writing a new script.

# Acceptance Criteria

- A standard matrix run on the shared harness shows Discuss with real content (preview counts and a summary answer served by the real router from the loopback model stub); no shot shows "not served by this check", a 404, or an error banner for Discuss.
- Keyboard-open shots are really shrunk (visual viewport height is the closed height minus 336), and the run fails by name if a keyboard-open shot's viewport height equals the closed one's; the offline self-test proves that check can go red.
- No shot leaves the view switcher popup or a transient toast over the page, unless the shot targets that UI; the settle step is exercised by the matrix run.
- The harness README documents the standard matrix command (themes, profiles, scenes) so lanes reuse it.
- `make -C vibe-talk screenshots-self-test` passes with new offline controls for the keyboard check and the settle targeting; a real matrix run's images are inspected one by one.

# Notes

Owner-priority, deployed on landing. Dynamic-testing standing rule (thread 185): proof is end-to-end through the real server router sending the page's exact requests, stubbing only external upstreams (the model, the chat upstream). Screenshot matrix standing acceptance (thread 124): light and dark, keyboard really shrunk and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in phone shots, every image looked at.

## Resolution

[watermelon-full-preview] Landed on main as de2fdf32 (gate pass on 8d026f58, ledger 2026-11-18T19:53:55Z). The shared harness now: (1) boots the real server with the fake chat upstream plus a loopback model stub (`scripts/model_stub.py`) and, for Discuss runs, a serving message cache seeded by the server's own history walk, so Discuss preview/Summarize answer through the real router — matrix states 73-76 assert no fixture refusal and the summary state asserts the stub's answer; (2) one shared `shrink_keyboard` helper (height − 336, condition wait on visualViewport) whose check fails the run by name on an unshrunk open shot, proven red in `--self-test` (61 controls); (3) `settle_ui` before every capture (switcher cards, menus, status and freshness pills, spared when the scene photographs them). `scripts/README.md` documents the standard matrix. Evidence: 16/16 matrix images (light/dark × android-412/laptop-1280 × 73-76) inspected — real Discuss content, keyboard-open really shrunk (1280x464, 579 CSS px phone), no stray popups; all screenshot steps green in gate. Note: the /fake-seed endpoint from the brief is not on main; the equivalent real seeding (serving cache + history walk) was used instead. Follow-up accepted from coordb: mocks bind port 0 (claims cannot exclude non-claiming binders).
