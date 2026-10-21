---
title: 'py-lockless-read-replaced: check the Python chat reader for a record replaced between open and fstat'
status: open
priority: 3
issue_type: task
created_at: 2026-10-21T00:12:36.213795646+00:00
updated_at: 2026-10-21T00:12:36.213795646+00:00
---

# Description

[opus 5.5] Follow-up from #394 ack-delivered-empty. The Rust chat reader refused a record whose opened file had no links left: a writer had renamed the record's next copy over it between the reader's open and its fstat. A delivery pass that reads its request without the state lock, beside the acknowledgement worker writing that request under the lock, failed that way and delivered nothing. The Rust edition now reads such a file as the complete earlier copy it is. The Python edition's bounded reader (agent.py _read_bounded_queue_json, chat_storage.read_text) still requires st_nlink == 1 at both fstats and an unchanged st_ctime_ns, which a rename over the file breaks. Python also runs acknowledgements on their own worker pool (chat_runtime.py), but whether any of its readers read a record outside the lock that its writers hold has not been checked. Acceptance: either name the lock that rules the interleaving out for each reader of a renamed record, or reproduce it with a forced interleaving and fix it as the Rust edition did (accept st_nlink 0 as the earlier copy; keep refusing more than one link).
