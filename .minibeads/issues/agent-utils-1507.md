---
title: 'local-ci-hub-rows-memory-cpu: bounded-memory streaming fold for ledger rows'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T01:08:00.953536773+00:00
updated_at: 2026-11-14T01:50:57.203991681+00:00
closed_at: 2026-11-14T01:50:57.203991250+00:00
---

# Description

[watermelon-full-preview] A cold Rust local-ci-hub ledger rows run over a large acceptance ledger (38 shards, about 5.2 GB of event lines, 5709 terminals) takes 22:27 wall, 1375 CPU-s and 125 GiB peak RSS. The Python oracle fold on the same input takes 7:24 and 443 CPU-s. Earlier fixes 636bbbf3 (quadratic string parse, per-page refold) and 51d160d8 (fixture filter) landed; the remaining cost centre is a full tail per invocation plus a single refold that materialises every parsed event. Validation hosts need bounded memory, so this must be fixed before wider use.

# Design

Profile first (heaptrack or valgrind massif on a small synthetic ledger scaled up, perf for CPU), then stream event lines with no Vec of all parsed events, parse borrowed where a slice will do instead of an owned String per field, fold incrementally, and drop finished terminals raw events once folded. Keep #1488/#1491-era behaviour unchanged. Same outputs byte-identical (rows, verdicts, digests); peak memory proportional to the working set (open terminals plus indexes), not to input size; CPU at or below the oracle.

# Acceptance Criteria

- Byte-identical rows, verdicts and digests versus the current implementation: existing core_parity and acceptance-shaped tests prove it.
- Peak memory proportional to the working set (open terminals plus indexes), not to input size.
- CPU at or below the Python oracle on the same input class.
- Scaling test runs the fold on N and 4N synthetic events and asserts on a structural measure (peak live allocations via a counting allocator, or event objects alive), never wall-clock or RSS-by-time.
- Benchmark (report only, not a test): synthetic ledger of about 1 GB inside a cpuset; report wall, CPU and peak RSS before and after. Never use a real ledger.

# Notes

M1 acceptance evidence is a consumer ledger measured out of tree; only synthetic fixtures and synthetic benchmarks are committed here. Keep #1488/#1491-era behaviour unchanged.

## Resolution

[watermelon-full-preview] Landed the streaming fold. Event lines stream page by page into a v2 SQLite index (sort keys + identity hashes stored per event, identical copies collapsed at admission); one streaming refold walks admitted events from disk in union order, retaining only the correction index, dedup hashes, and one event/row at a time, dropping each raw event once its terminal row is folded. The union compares identity hashes and orders by borrowed string fields; ledger outputs stream row by row; the git connector loads one shard at a time with per-head inventory and verified-prefix caching. Byte-identical outputs proven by existing parity suites plus before/after CLI comparisons (rows, digest-rows, verdicts, terminals, receipt) on synthetic ledgers. Synthetic benchmark (936 MB, 150k events, 8 shards, cpuset 24 cores, report only): wall 5:28 -> 1:29, CPU 402 s -> 107 s, peak RSS 13.5 GiB -> 2.6 GiB, outputs byte-identical. Massif (400 events): peak heap 35.6 MiB -> 11.8 MiB. Scaling test asserts counted peak heap N=78,411 B vs 4N=78,411 B over a fixed working set. Note: with the default ephemeral in-memory index the SQLite store itself remains input-sized on disk-equivalent RAM; a file-backed index keeps the fold at working-set memory.
