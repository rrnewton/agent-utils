# Packaging and documentation adversarial review — 2026-08-05

The distribution review built artifacts outside the source tree and treated
their installed metadata, resources, entry points, documentation, and startup
behavior as the contract.

## Findings resolved

1. The `safe-ci-dag-runner` registry archive lacked a license file. Every crate
   now carries its license in the published archive.
2. The crate checker created archives without verifying them and reused the
   workspace target. It now uses an isolated target per crate, performs locked
   offline normalized-package verification, compares packaged docs to source,
   and smokes every packaged binary.
3. A missing wheel build backend produced an opaque installer traceback. The
   checker now preflights its offline backend with an actionable message.
4. Wheel checks now cover the interpreter requirement, dependencies, README,
   license, package resources, entry points, installed origins, module and
   console startup, embedded guides, and absence of sibling packages.
5. Documentation generation did not reject unknown template tokens or sibling
   tool names, and stale output suppressed useful lint diagnostics. Rendering is
   now prevalidated and every generated destination is linted independently.
6. Package-facing comments and public docstrings still exposed source-tree or
   sibling-language implementation details. Public artifact documentation is
   now edition-local; parity notes remain in repository-internal comments and
   tests.

## Evidence

- All 12 generated documents were current and standalone.
- Packaging infrastructure and dependency-free smoke tests: 22 passed.
- All packaging scripts and their focused tests passed strict type checking and
  bytecode compilation.
- Three wheels built offline, installed alone, and passed resource, metadata,
  import-origin, API, and command checks.
- Three registry crates passed locked offline package verification and binary
  smoke checks.
- Explicit package/index documentation scans found no sibling language,
  package manager, source path, or unrelated-project leakage.
