# Timeline payload scaling

## Measured Hermit baseline

The 2026-08-12 full-corpus audit measured the generated schema-1 `data/timeline.json` at
196,704,084 bytes. Deterministic gzip level 6 reduced the same bytes to 27,043,927 bytes: 13.7% of
the identity representation, or an 86.3% transfer reduction. This is an important low-risk first
step, but the browser must still download one whole representation, inflate roughly 197 MB, parse
it, and retain the resulting object graph before it can draw any time range.

The audit also found detailed edge `full_text` values duplicating message bodies available through
the phase/detail transcript data. That duplication makes the bootstrap projection carry drill-down
prose which is unnecessary for an outer zoom level. Schema 1 must retain its current fields for
compatibility; a sharded projection should replace those copies with stable message/detail
references and load prose only for the selected range.

## Sharding measurements and recommendation

A schema-2 prototype decomposition put the global bootstrap at approximately 125 KB gzip. Daily
time shards averaged approximately 274 KB gzip, with the largest observed day approximately
827 KB gzip. Those sizes support range-driven loading without making normal pan/zoom interactions
depend on hundreds of tiny requests.

Implement the transition in stages:

1. Keep schema 1 and identity files, add deterministic gzip sidecars, HTTP content negotiation,
   strong validators, and browser revalidation. This is the implemented compatibility step.
2. Add a schema-2 bootstrap containing identity, overall range, team/agent indexes, aggregate
   activity bins, calendar-rollup availability, and a shard catalog with byte sizes and content
   digests. Continue emitting schema 1 during migration.
3. Put detailed phases, states, events, and structural/detailed edge records into daily UTC or
   display-calendar shards. Fetch the visible days plus a small adjacent prefetch window, retain
   resolved shards in an in-memory cache, and rely on immutable digest URLs or ETag revalidation
   across reloads. Panning must never refetch an unchanged cached shard.
4. Keep full transcript/detail documents separately addressable. Remove duplicated edge
   `full_text` from the schema-2 bootstrap and shards once stable references can resolve the same
   content on demand.

The loader should select schema 2 when its bootstrap is present and fall back to the existing
schema-1 monolith otherwise. This preserves exported archives and basic static-server deployment
while allowing a multi-week view to start with hundreds of kilobytes rather than tens or hundreds
of megabytes.
