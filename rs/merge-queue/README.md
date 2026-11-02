# merge-queue

A local merge queue. You mark changes ready to land. The queue orders them and splits them into
batches, using the exact Git conflict graph plus priority and age. It validates each batch's exact
combined tree and lands only trees that passed. Validation is a set of individual tests, and the
queue records which ones ran. Focused runs leave a tracked obligation to widen to the complete
suite, and a red batch's culprit is found by rerunning only the tests that failed, then ejected
with a report while the rest lands.

```bash
cargo build --release --manifest-path rs/Cargo.toml -p merge-queue
merge-queue init --remote origin --base main
merge-queue submit 42 --head feature/x --tested-with all
merge-queue plan            # batches, held changes and why, conflict clusters
merge-queue plan --json     # schema merge-queue-plan/v1
merge-queue run --shadow    # validate for real, land on a simulated base
merge-queue run             # validate (two batches at once by default) and land
merge-queue widen           # discharge focused landings with one complete run
```

It runs entirely on the host that holds the clone: no service, no webhook, no inbound connection.
The only dependency is `git` 2.40 or newer.

The window of concurrent validations is a rolling dependent pipeline that can adapt (AIMD:
`init --max-depth N`), and `merge-queue bench` measures the landing rate against a serial lander
on a synthetic repository.

The operator documentation is `merge-queue quickstart` and `merge-queue userguide`, also in
[`common/docs/merge-queue/`](../../common/docs/merge-queue/QUICKSTART.md). For comparison with
bors, bors-ng, Zuul, Marge-bot and Prow Tide, see
[`RELATED_WORK.md`](../../common/docs/merge-queue/RELATED_WORK.md).
