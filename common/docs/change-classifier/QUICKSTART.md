# change-classifier quickstart

Test selection and a merge queue ask the same question of every change: which parts of the
repository does it touch, does it change the CI itself, is it documentation only, and does any
path match no rule at all? `change-classify` answers it from one policy file, so both read the
same buckets.

Build it from this repository (it needs only Git at run time):

```bash
cargo build --release --manifest-path rs/Cargo.toml -p change-classifier
```

Write a policy, a resolved test-footprint file (version 2):

```json
{
  "version": 2,
  "force_full": ["Cargo.lock", "ci/**"],
  "ci_irrelevant": ["**/*.md", "docs/**"],
  "footprints": [
    {"paths": ["core/**/*.rs", "core/Cargo.toml"], "nodes": ["test.core"]},
    {"paths": ["cli/**"], "nodes": ["test.cli"], "component": "command-line"}
  ]
}
```

Classify the change on the current branch:

```bash
change-classify classify --policy ci/test-footprints.json --base origin/main
# selective
# components: core
# nodes: test.core
#   1 node(s) selected
```

`skip` means every path is inert, `selective` lists the nodes the change selects, and `full`
means run everything: a forcing path (`force_full`), an unknown path, or nothing to go on. Add
`--json` for every path's class and the matched rules, or pass `--paths A B` (or `--paths -` to
read a list or a `git diff --name-status` listing from standard input) instead of `--base`.

## What to know next

- A pattern matches the whole path: `Cargo.toml` is only the top-level file, `*` stays within
  one directory, and `docs/**` matches everything under `docs/`.
- An unknown path always gives `full`. Add it to a footprint or to `ci_irrelevant`.
- `change-classify check-vectors` checks this build against the golden glob and rule-order
  vectors, the contract every consumer relies on.
- A merge queue uses the same policy with `merge-queue init --classifier-policy PATH`.
- `change-classify userguide` documents the policy format, the rules and the JSON output.
