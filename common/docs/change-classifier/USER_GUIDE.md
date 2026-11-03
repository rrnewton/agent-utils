# change-classifier user guide

`change-classify` classifies the paths a change touches against a test-footprint policy. Test
selection uses the verdict to decide what to run; a merge queue uses the components and verdict
as features of each change. The library (`change_classifier` in `rs/change-classifier`) and the
command give the same answers.

## Commands

### `change-classify classify`

Classify one change.

| Option | Default | Meaning |
|---|---|---|
| `--policy FILE` | required | The policy file (below). |
| `--base REV` | none | The change is what `--head` changed since its merge base with `REV` (`git diff --name-status -M BASE...HEAD`). A rename or copy counts at both ends. |
| `--head REV` | `HEAD` | The change's head, with `--base`. |
| `--repo DIR` | `.` | The repository to diff in, with `--base`. |
| `--paths PATH...` | none | The changed paths instead of `--base`. `-` reads them from standard input, one per line; a `git diff --name-status` listing is accepted too. |
| `--json` | off | Print the whole classification as JSON (below). |

The text form prints the verdict, the components, the selected nodes and the reasons. Exit status
is 0 for any verdict and 2 for a usage or policy error.

### `change-classify glob PATTERN PATH`

Exit status 0 when `PATH` matches `PATTERN`, 1 when it does not.

### `change-classify check-vectors [FILE]`

Check golden vectors against this build: by default the vectors it ships
(`rs/change-classifier/testdata/vectors.json`). Each failing case is printed by name; exit status
1 when any fails.

### `change-classify quickstart`, `change-classify userguide`

Print the guides.

## The policy

A resolved test-footprint file, `"version": 2` (other versions are refused):

| Key | Meaning |
|---|---|
| `force_full` | Globs whose change forces the full suite: build and CI configuration. |
| `ci_irrelevant` | Globs that are inert when no footprint or backend rule matches them: documentation and the like. |
| `groups` | `{NAME: [node, ...]}`; `@NAME` in a node list expands to the group, recursively (a cycle stops). An unknown group refuses the policy when it loads. |
| `footprints` | `[{paths, nodes, e2e_all, e2e_backends, component}]`: a change to `paths` selects `nodes`. `e2e_all` and `e2e_backends` say which end-to-end cells it may affect. `component` is optional. |
| `backend_rules` | `[{id, tier, paths, NAME_paths, backends, lanes, extra_nodes}]`: `tier` is `backend` (the match selects `extra_nodes` and scopes `backends`, where `*` lists nothing) or `core` (the match forces the full suite). Any key `NAME_paths` other than `paths` holds globs relative to the directory `NAME/`, for a vendored or submodule tree. |

Other keys (`_comment`, `_why`, ...) are ignored; a footprint's own keys stay available to library
callers as `Footprint::raw`.

A footprint's component is its `component` key, else the first directory of its first glob when
that directory is literal (`core/**/*.rs` gives `core`), else `footprint-N` (its position). A
backend rule's component is `backend:ID`.

## Globs

Patterns match whole `/`-separated paths:

- `**` matches any run of characters, `/` included; `dir/**/x` also matches `dir/x`, and
  `**/x` matches `x` at the top.
- `*` matches any run of characters other than `/`, the empty run included.
- `?` matches one character other than `/`.
- Every other character matches itself. A pattern is never a suffix: `Cargo.toml` matches only
  the top-level file.

## The rules

Each path is matched in this order:

1. A `force_full` match: the path is `force-full`, and the verdict will be `full`.
2. Otherwise every footprint and every backend rule that matches contributes (nodes, end-to-end
   scope, lanes, components). Such a path is `matched`, even when `ci_irrelevant` would also
   match it: a Markdown file under a footprint's directory is not inert.
3. A path nothing matched is `inert` when `ci_irrelevant` matches it, and `unknown` otherwise.

The verdict, first rule that applies:

| Condition | Verdict |
|---|---|
| No paths at all | `full` |
| A `core`-tier backend rule matched | `full` |
| A path matched `force_full` | `full` |
| A path is unknown | `full` |
| Every path is inert | `skip` |
| No node was selected (a backend rule with no nodes, say) | `full` |
| Otherwise | `selective`, with the selected nodes |

The classifier never runs fewer tests than these rules allow; a consumer may widen a selective
verdict with its own policy (for example, allow selective runs only for some backends).

## JSON output

```json
{
  "verdict": "selective",
  "nodes": ["test.core"],
  "e2e_all": false,
  "e2e_backends": [],
  "backend_rules": [],
  "backend_lanes": [],
  "components": ["core"],
  "unknown": [],
  "paths": [{"path": "core/src/a.rs", "class": "matched", "footprints": [0]}],
  "reasons": ["1 node(s) selected"]
}
```

`class` is `force-full`, `matched`, `inert` or `unknown`; `footprints` (indexes) and
`backend_rules` (identifiers) are left out when empty.

## The golden vectors

`rs/change-classifier/testdata/vectors.json` (schema `change-classifier-vectors/v1`) holds glob
cases (`{pattern, path, match}`), `git diff --name-status` parsing cases (`{input, paths}`),
sample policies, and classification cases (`{name, policy, paths, expect}`, where `expect` names
the verdict and any of `nodes`, `e2e_all`, `e2e_backends`, `backend_rules`, `backend_lanes`,
`components`, `unknown` and per-path `classes`). They are the contract between this crate and
every consumer that shares it: a change to glob or rule-order semantics must keep them passing,
or change them together with a replay of each consumer's history, and the consumers are told
before it lands.
