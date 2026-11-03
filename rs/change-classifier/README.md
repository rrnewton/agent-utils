# change-classifier

Classifies the paths a change touches against a test-footprint policy: `skip` (every path is
inert), `selective` (the nodes the change selects) or `full` (a forcing path, an unknown path, or
nothing to go on), with the components it touches and each path's class. Test selection and the
merge queue share it, so both read the same buckets.

```bash
cargo build --release --manifest-path rs/Cargo.toml -p change-classifier
change-classify classify --policy ci/test-footprints.json --base origin/main
change-classify check-vectors      # the golden glob and rule-order contract
```

The operator documentation is `change-classify quickstart` and `change-classify userguide`, also
in [`common/docs/change-classifier/`](../../common/docs/change-classifier/QUICKSTART.md); for
comparison with path filters and build-graph selectors, see
[`RELATED_WORK.md`](../../common/docs/change-classifier/RELATED_WORK.md).
