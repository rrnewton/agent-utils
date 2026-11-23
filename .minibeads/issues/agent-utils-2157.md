---
title: 'wrkviz-ingest-orc-new-session-schema: ingest-orc refuses current Orc session databases'
status: open
priority: 2
issue_type: bug
labels:
- wrkviz
created_at: 2026-11-23T07:00:00-05:00
updated_at: 2026-11-23T07:00:00-05:00
---

# Description

wrkviz ingest-orc (py/wrkviz/orc.py) refuses current Orc session databases with "session.db: missing required tables: content_blocks". Newer Orc session schemas keep messages without a content_blocks table, while orc.py still requires it in two places. Older archived sessions ingest fine.

# Design

Detect the session schema version and read messages from whichever tables it has (content_blocks when present, the messages table otherwise), mapping both to the same timeline records; refuse only when neither shape is present, naming the tables found.

# Acceptance Criteria

- Tests with fixture session databases of both shapes ingest to the same timeline record shape; a database with neither shape refuses with a clear message.

# Notes

Found while planning an Orc subteam trial; filed by the coordinator.
