# merge-queue

A local merge queue. You mark changes ready to land. The queue orders them and splits them into
batches, using the exact Git conflict graph plus priority and age. It validates each batch's exact
combined tree and lands only trees that passed. Validation is a set of individual tests, and the
queue records which ones ran. Focused runs leave a tracked obligation to widen to the complete
suite, and a red batch is bisected by rerunning only the tests that failed.

```bash
cargo build --release --manifest-path rs/Cargo.toml -p merge-queue
merge-queue init --remote origin --base main
merge-queue submit 42 --head feature/x --tested-with all
merge-queue plan            # batches, held changes and why, conflict clusters
merge-queue plan --json     # schema merge-queue-plan/v1
```

It runs entirely on the host that holds the clone: no service, no webhook, no inbound connection.
The only dependency is `git` 2.40 or newer.

This release plans. The `run` command, which validates candidates speculatively and lands them, is
next.

The operator documentation is `merge-queue quickstart` and `merge-queue userguide`, also in
[`common/docs/merge-queue/`](../../common/docs/merge-queue/QUICKSTART.md). For comparison with
bors, bors-ng, Zuul, Marge-bot and Prow Tide, see
[`RELATED_WORK.md`](../../common/docs/merge-queue/RELATED_WORK.md).
