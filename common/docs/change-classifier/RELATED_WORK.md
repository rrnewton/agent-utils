# change-classifier: related work

This comparison is about **changed-path classification**: deciding, from the files a change
touches, which tests or components it affects. It covers public, open-source tools and was written
in 2026-11 from public documentation. No code was copied from any of them.

| Project (license) | Relevant capability | Relationship to change-classifier |
| --- | --- | --- |
| [dorny/paths-filter](https://github.com/dorny/paths-filter) (MIT) | A GitHub Action that matches changed files against named glob filters and reports which filters matched. | The same idea of named path buckets. It reports matches only; it has no fail-closed rule for an unmatched path and no notion of a forcing path. |
| GitLab CI `rules:changes` and GitHub Actions `paths` filters | Run a job only when listed paths change. | Per-job filters spread across workflow files. change-classifier keeps every rule in one policy and classifies the whole change at once. |
| [Nx affected](https://github.com/nrwl/nx) (MIT), [Turborepo](https://github.com/vercel/turborepo) `--filter` (MIT) | Map changed files to projects through a project graph, then run the affected projects' tasks. | Project ownership comes from their workspace model. change-classifier is model-free: the policy (often generated from a build graph elsewhere) holds resolved paths. |
| [Pants](https://github.com/pantsbuild/pants) `--changed-since` (Apache-2.0), [bazel-diff](https://github.com/Tinder/bazel-diff) and [target-determinator](https://github.com/bazel-contrib/target-determinator) (Apache-2.0) | Compute the build targets a change affects from the build tool's own dependency graph. | Exact for one build system. change-classifier works on paths alone, for any repository, and leaves graph resolution to whatever generates its policy. |

## How change-classifier differs

- **Fail closed.** A path that matches no rule makes the verdict `full`; only positive proof that
  every path is inert gives `skip`.
- **One fixed rule order.** Forcing paths first, then every matching footprint and backend rule,
  then the inert list. The order and the glob dialect are pinned by golden vectors.
- **Shared.** The same answer feeds test selection and a merge queue's batching features.
