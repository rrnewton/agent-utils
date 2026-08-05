# Summary backend comparison experiment (2026-08-05)

## Bottom line

This was **not a completed three-LLM comparison**. It was one controlled
comparison between the deterministic heuristic and Sol, plus an attempted Luna
run that never reached generation:

- The controlled heuristic and Sol runs used the same frozen input, pipeline
  revision, job partitioning, prompt version, batching, and reasoning setting.
  They are useful for comparing the free deterministic fallback with Sol's
  output, but the heuristic is code, not an LLM.
- Luna produced no summaries. The requested `gpt-5.6-luna` deployment was not
  available to the backend. Its eight failed receipts contain `usage: null`, so
  there is **no Luna token cost to report**; null usage is not evidence of zero
  spend.
- The Orc total of **2,704,708 successful tokens** (or **2,774,129 all-in**) is
  a different workload, not another model result. It must not be placed in a
  model-comparison cost chart beside the Codex-Hermit run.

The exact controlled inputs and results are retained under `~/temp/` for
inspection. The archive format and normal workflow are described in the
[project README](../README.md).

## Controlled workload

The immutable source archive was
`~/temp/codex-hermit-timeline-benchmark-source`. Its digest was:

```text
bcaa65af434f9d2bff148a6b6a92b46336e2465ca55f7ef696575cb36ee34b20
```

All three attempted modes ingested that digest. The snapshot contained:

| Measure | Count |
|---|---:|
| Source transcript files / agent tracks | 51 |
| Events | 4,181 |
| Outer tool calls | 12,920 |
| Imported interaction edges | 1,207 |
| Source bytes | 238,738,644 |

The summarizer produced the same 228 logical jobs for the controlled heuristic
and Sol runs: 172 work phases, 51 hindsight agent names, and five calendar
rollups. Both used the `agent-team-timeline-summary-v1` and
`agent-team-timeline-agent-name-v1` prompt versions at revision `f59efe6`, with:

| Control | Value |
|---|---:|
| Display timezone | `America/New_York` |
| Phase width | 30 minutes |
| Prior-context limit | 16,000 characters |
| Transcript limit | 30,000 characters |
| Summary batch size | 6 jobs |
| Name batch size | 12 agents |
| Workers | 3 |
| Reasoning effort | `medium` |

These settings yield 39 successful backend batches when starting from an empty
cache: 29 phase batches, five naming batches, and five ordered rollup batches.
The heuristic control was generated from revision `f59efe6`; the Sol benchmark
was run from the same working revision. The run metadata does not itself record
the Git SHA, so the latter is operator provenance rather than a property that
can be re-derived from the archive. Future experiments should persist the tool
SHA.

## Results

`total` is `input + output`. Cached input is already included in input, and
reasoning output is already included in output; neither should be added to the
total a second time.

| Attempt | Artifact directory |
|---|---|
| Deterministic heuristic | `~/temp/07_codex-hermit-controlled-heuristic` |
| Luna (incomplete) | `~/temp/codex-hermit-timeline-luna` |
| Sol control | `~/temp/04_codex-hermit-controlled-sol` |

| Attempt | Completed receipts | Failed receipts | Successful artifact total | Failed-call overhead | All recorded spend | Result |
|---|---:|---:|---:|---:|---:|---|
| Deterministic heuristic | 39 | 0 | 0 | 0 | 0 | Complete; not an LLM |
| `gpt-5.6-luna` | 0 | 8 | unknown | unknown | unknown | No summaries; deployment unavailable |
| `gpt-5.6-sol` | 39 | 6 | 2,253,434 | 396,252 | 2,649,686 | Complete |

The Luna row must not be rendered as zero tokens. Two top-level attempts
created eight failed batch receipts, all with null usage. The persisted error is
a backend exit before a usable response; the deployment diagnosis during the
experiment was HTTP 404/unavailable. There is neither a summary artifact nor a
metered token record to compare with Sol.

### Sol token detail

| Sol receipt set | Input | Cached input (subset) | Output | Reasoning output (subset) | Total |
|---|---:|---:|---:|---:|---:|
| 39 completed artifact receipts | 2,167,643 | 170,752 | 85,791 | 16,491 | 2,253,434 |
| 6 failed validation receipts | 380,405 | 29,696 | 15,847 | 1,386 | 396,252 |
| All recorded Sol calls | 2,548,048 | 200,448 | 101,638 | 17,877 | 2,649,686 |

Sol needed five failed top-level summarize attempts before a complete run. Six
individual calls returned token-bearing output that failed structural
validation, chiefly because work-summary bullets were not chronological or a
bullet timestamp fell outside its interval. Successful batches were cached
between attempts. Thus 2,253,434 is the generation cost behind the final
controlled artifacts, while 2,649,686 is the actual all-in recorded spend for
the experiment. The difference is real retry overhead, not cache replay.

The immediate replay was idempotent: 228 cache hits, zero misses, zero backend
batches, zero newly spent tokens, and a subsequent build changed zero files.

## Plain-language feature extension

`~/temp/05_codex-hermit-sol-feature-complete` was copied from the controlled
Sol output and then updated by a later pipeline revision. It uses the same
source digest and model, but it is **not another model arm** and is not strictly
comparable to the controlled run: the technical rollup prompt changed and five
new plain-language rollups were added, giving ten changed/new jobs.

| Quantity | Tokens |
|---|---:|
| New successful spend for the feature migration | 242,331 |
| Token cost behind the final feature-complete artifact set | 2,375,037 |
| Superseded successful receipts retained for audit | 120,728 |
| All successful receipts accumulated in the directory | 2,495,765 |
| Inherited failed-call overhead | 396,252 |
| Cumulative all-in recorded spend | 2,892,017 |

Its replay had 233 cache hits, zero misses, zero backend batches, zero newly
spent tokens, and a zero-change build. Those numbers demonstrate idempotence;
they do not turn the feature migration into an apples-to-apples model test.

## Qualitative observations

The heuristic and Sol artifacts can be compared on identical transcript data,
but this inspection was not blinded or scored. One daily-summary comparison
illustrates the broad difference:

- The deterministic heuristic repeated the same sentence across multiple time
  buckets, exposed a raw timestamp and `ASSISTANT:` excerpt, and ended several
  items with mechanical truncation.
- Sol synthesized a coherent outcome, preserved concrete counts and findings,
  and produced a chronological work summary rather than copying nearby text.
- Sol's six validation failures show that better prose did not guarantee schema
  discipline; strict validation and durable failed-call accounting materially
  affected its real cost.
- Luna has no output and therefore no quality observation.

The later plain-language Sol sample is easier to approach than the technical
rollup, but it still contains project terms such as `AGENTS.md`, registry, and
skill adapter. That is a prompt/product observation, not evidence of model
superiority, because no other model produced the same plain-language jobs.

## Why the Orc number is separate

The Orc artifact is `~/temp/06_orc-hermit-day1-sol`. It summarizes a
different provider snapshot and date window with different transcript density,
lineage, prompts, and pipeline features. Its imported backing data contained
1,413 agent incarnations, 42,761 events, and 24,593 outer tool calls; the
selected day produced 219 phases, 18 displayed agents, and both technical and
plain-language rollups: 245 logical jobs in all. Its source digest was
`977e3d84f3a3c8c692485880ecf7f7494ccbc99386d1a8d2cfdf864ffe35e27c`.

| Orc Sol receipt set | Input | Cached input (subset) | Output | Reasoning output (subset) | Total |
|---|---:|---:|---:|---:|---:|
| 90 completed receipts | 2,608,801 | 326,656 | 95,907 | 12,596 | 2,704,708 |
| 1 failed receipt | 67,075 | 7,424 | 2,346 | 108 | 69,421 |
| All recorded calls | 2,675,876 | 334,080 | 98,253 | 12,704 | 2,774,129 |

Therefore, 2,704,708 is the cost behind successful Orc artifacts and 2,774,129
is its all-in cost. Neither number says whether Sol is cheaper or more expensive
than another model: there is no second model on the same Orc input, and this is
not the controlled Codex-Hermit workload.

## Evidence and audit method

The figures above were recomputed from JSON rather than copied from console
output:

- `manifest.json` supplies each source digest.
- `runs/*.json` supplies commands, job/cache counts, and replay results.
- `teams/<team>/summary_data/{cache,name_cache}/_usage/receipts/*.json`
  supplies per-call status and token counters.
- The successful artifact total in a completed summarize run is the deduplicated
  sum of receipts referenced by the returned artifacts. Summing every receipt
  instead gives historical spend, including failed and superseded calls.

All receipt totals were checked as `total_tokens == input_tokens +
output_tokens`. Cache-write input was zero in every receipt discussed here.
No dollar estimate is given because the artifacts do not carry a dated,
model-specific price schedule.

## Requirements for a valid three-model comparison

1. Pin and record one Git revision, prompt versions, source digest, timezone,
   date bounds, phase/context/transcript limits, batch sizes, worker count, and
   reasoning setting.
2. Select three actual LLM deployment identifiers and make a tiny availability
   probe for each before starting the full run. An unavailable arm remains a
   failed arm; do not silently substitute a model.
3. Start each arm from a fresh output directory containing the same immutable
   source snapshot and no summary cache. Do not compare against a live or later
   snapshot.
4. Apply the same retry cap and validation policy. Report both the deduplicated
   successful-artifact cost and all-in cost, including failed calls.
5. Replay every completed arm and require zero cache misses, zero backend calls,
   and zero changed build files.
6. Blind the model labels and score the same sampled phases, names, and rollups
   for factual accuracy, omitted work, chronology, terminology consistency,
   plain-language accessibility, opaque references, and unsupported claims.
7. Record wall time separately from tokens. Convert tokens to money only with a
   dated price table that distinguishes cached input, uncached input, and
   output for each model.

Until those steps produce three completed LLM arms, describe this evidence as a
heuristic-versus-Sol control with a failed Luna availability attempt—not a
three-model benchmark.
