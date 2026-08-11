---
name: pr-landing-planner
description: Produce an advisory PR landing plan from merge conflicts, exact-revision evidence, policy classes, assignments, and mechanism overlaps. Use before choosing or assigning a landing batch; the planner never mutates a PR.
---

# PR landing planner

**Follow skill `hermit-validation-authority` in the consuming dev-hermit workspace for
validation authority and evidence semantics.**

Use the planner to produce a shared, machine-readable landing plan. It detects real merge conflicts,
classifies supplied validation evidence, surfaces mechanism overlaps, and forms conflict-safe groups.
Its output is advisory and does not authorize or perform repository mutations.

For live data, provide the repository, target branch, local clone, and caller-owned landing context:

```sh
pr-landing-planner plan \
  --repo OWNER/NAME \
  --base BASE \
  --git-dir /path/to/clone \
  --landing-context context.json \
  --format json
```

The context carries caller-supplied validation references plus policy and
assignment facts; the planner does not redefine their qualification. Review
state is collected separately; verify any required approval against the final
revision before publication. A fixture can exercise the same planning path
without network data:

```sh
pr-landing-planner plan --fixture /path/to/fixture.yaml --landing-context context.json
```

Consult the installed CLI for the complete input and output contract:

- `pr-landing-planner quickstart`
- `pr-landing-planner --help`
- `pr-landing-planner --userguide`

Use [PR landing operations](../pr-landing-operations/SKILL.md) only after reading the consuming
repository's authorization, review, validation, and merge rules.
