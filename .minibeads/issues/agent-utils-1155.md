---
title: 'wrkviz-role-classifier: label each agent session coordinator, implementor or reviewer with a confidence'
status: closed
priority: 2
issue_type: task
created_at: 2026-11-09T16:53:23.683078166+00:00
updated_at: 2026-11-11T10:05:10.496971672+00:00
closed_at: 2026-11-11T10:05:10.496971522+00:00
---

# Description

Owner request 2026-11-09 (reply 243). wrkviz should classify every agent session as coordinator, implementor or reviewer so token expenditure can be surveyed per role per day. Coordinators get sub-labels: liaison (chat-facing, no direct implementor or reviewer reports) and lieutenant (a coordinator that reports to another coordinator). Coordinators should delegate tool calls rather than block themselves, but that rule is not followed perfectly, so no single signal is a deterministic classifier.

# Design

Combine signals heuristically with an explicit confidence: keywords and briefs (lane-of lines, coordinator/reviewer/liaison wording, DONE:/BLOCKED: verdicts, review verdict language); topology (agentctl start/send graphs, who briefs whom, agent-teams edges, registry records with profile and created_by/managed_by); and tool-use volume and mix (edits and commits versus sends and reads). Agent names (coord prefixes, mu-/cl- lanes, reviewer- names) are one weak signal, never the rule. Output feeds a per-day, per-role token and dollar survey; the survey itself is reported in the harness, not in this issue.

# Acceptance Criteria

Classifier lands in wrkviz with tests in the existing test layout. Precision measured on a hand-checked sample of at least 50 sessions across roles and hosts, reported as a confusion table in the harness survey report and linked here. Low-confidence sessions are labelled unclassified rather than forced. No transcript store or registry is modified by classification.
