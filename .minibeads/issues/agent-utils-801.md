---
title: 'chat-latency-summaries: fixed-size coordinator response statistics by model, effort and adapter'
status: open
priority: 2
issue_type: feature
assignee: mu-transcript@devbig014
labels:
- agentctl
- chat
depends_on:
  agent-utils-714: blocks
created_at: 2026-11-03T01:33:23.392921411+00:00
updated_at: 2026-11-03T01:33:23.392921411+00:00
---

# Description

[opus 5.5] Owner request, filed for the devbig014 coordinator: fixed-size statistics on how quickly coordinators answer chat requests, broken down by model, reasoning effort and adapter (headless or TUI). The numbers will be used to judge a later trial that puts a liaison agent in front of a coordinator. Today every request has to be re-derived from raw bridge request and reply records, and those records age out under retention.

# Design

1. Tagging: at delivery time the bridge records the target agent's model, reasoning effort and adapter, read from the agentctl registry, on each request record. This touches the delivery path; coordinate the change with the #783 agentctl-start-pane-busy slices.
2. Summary: one bounded summary per (bridge, coordinator, model, effort, adapter, ET day). Each holds log-bucketed histograms for every stage: posted->admitted, admitted->ack, ->delivered, delivered->first reply, and posted->first reply. Budget about 1 KB per day. Keep summaries in their own subdirectory, apart from request records, so they survive request-record retention and never mix with fixed files.
3. Query: `agentctl chat latency --by profile --since T --until T` prints the summaries for a window, for before/after comparisons.
4. Backfill: a one-shot pass builds summaries from the existing bridge request and reply records.
The vibe-talk cache is optional input only: compute from it in place and never copy it. Put the code in new modules where possible.

# Acceptance Criteria

- Request records delivered after the change carry model, effort and adapter.
- A day's summary for one (bridge, coordinator, model, effort, adapter) stays near 1 KB however many requests it covers, and lives in its own subdirectory.
- Summaries outlive the request records they came from.
- `agentctl chat latency --by profile --since T --until T` shows per-stage distributions for the window, with --help covering every argument.
- The backfill builds summaries from existing records once, and running it again does not double-count.
