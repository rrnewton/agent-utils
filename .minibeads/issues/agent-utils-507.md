---
title: 'terminal-backend-traits: abstract Herdr behind clean Rust traits with a first-class tmux backend'
status: open
priority: 2
issue_type: feature
assignee: mu-term
created_at: 2026-10-25T12:48:01.186541330+00:00
updated_at: 2026-10-25T12:48:01.186541330+00:00
---

# Description

Owner: agentctl should not leak its dependence on Herdr; tmux support should be improved; the multiplexer interface needs clean Rust traits. Deliver rs/term-backend (Multiplexer + PaneTerminal traits, Herdr + tmux + Fake backends, backend selection with detection), wire the durable direct-pane transport and capabilities through backend selection, and document backends in the user guide and README.

# Acceptance Criteria

term-backend crate tests green incl. real-tmux private-socket integration; agentctl Herdr behaviour unchanged (existing suites green); herdr-agent direct send/drain/status/read runs on tmux pane ids; capabilities reports herdr+tmux and the selected backend; docs updated.
