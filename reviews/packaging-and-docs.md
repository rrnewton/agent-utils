# Packaging and documentation adversarial review — 2026-08-05

## Review provenance

The initial artifact review was recorded with the native implementations in commit
`5ef91c55036227b5dc2997ef069784f337b4cc5e`. Three independent, read-only completion audits then
reviewed baseline `3de72236cb33929fc5a0be3014d7a354f2c5938a` from separate perspectives:

- package documentation and public API completeness;
- common-source and symlink topology plus release-archive behavior;
- implementation inventory, behavioral parity evidence, and installer claims.

The auditors reported the gaps below before implementation. Release validation runs on Linux
x86-64 with the declared Python 3.10 and Rust 1.85 minimums also exercised in CI.

A second three-way, read-only audit of published completion candidate
`6ddd9bca7cbed6387a407a0ac7760626e9be2dcb` challenged the finished package surface, parity
claims, and artifact checks. It found the final gaps recorded in items 8–10; each now has an
executable regression check.

## Findings resolved

1. `agent-team-timeline` was source-only and documented a broken aggregate install. It now has an
   independent manifest, README, user guide, license, command entry point, wheel, sdist, and isolated
   artifact smoke test. Its explicit exception is Rust parity, not package quality.
2. Twelve paired package documents, the timeline guide, and six licenses were duplicated regular
   files. Paired editions now render under `common/docs/<tool>/rendered/`; all package docs and all
   seven package licenses are exact checked symlinks to authoritative sources.
3. Release checks covered wheels but not source distributions. The Python checker now builds both,
   rejects links inside either artifact, rebuilds a wheel from each sdist, compares every packaged
   module to the differential-tested source, installs the rebuilt wheel alone, blocks network access,
   and smokes every command.
4. Python license metadata used a deprecated table and classifier. All four distributions use PEP
   639 metadata with a backend version that implements it.
5. Public API checks ignored missing documentation and implementation-history prose. Every Python
   public module/class/function and every Rust public item is now documented; package checks reject
   missing docs, sibling languages, source paths, unrelated products, templates, provenance, and
   development-history language.
6. The workspace claimed Rust 1.82 while locked dependencies required 1.85. The declared MSRV is
   now 1.85, inherited by all crates, compiler-enforced in a dedicated CI job, and used to test and
   package every crate.
7. Package checks did not prove that differential-tested code was what users installed. Wheel and
   crate inspection now compares every packaged source module byte-for-byte with the checked source;
   artifact startup also verifies installed origin and absence of sibling packages.
8. Declared non-Python resources were checked only for presence. Every declared wheel and sdist
   resource is now byte-compared with its authoritative source, direct and sdist-rebuilt wheels must
   agree, and a deliberately corrupted `timeline-core.js` wheel proves the check fails closed.
9. The repository dispatcher and dependency smoke test omitted the packaged `cpuset-alloc` companion
   command. `./bin/cpuset-alloc` now shares the tracked resolver, and all five Python entry-point
   modules receive the dependency-free startup probes.
10. An unused `safe_ci_dag_runner.analyze` module exposed an undeclared Python-only `main()` and
    public API. The supported portable `summary` command already owns that job, so the orphan surface
    was removed. A manifest-derived test now rejects any public `main()` not backed by a declared
    console entry point.

## Executable evidence

```sh
python3 scripts/embed_userguides.py --check
make check-python-packages
make check-rust-packages
make cross
```

The documentation check covers 12 rendered paired documents, two authoritative single-language
documents, and 21 exact package links. Artifact checks cover four wheels, four sdists, four wheels
rebuilt from sdists, and three registry crates. The repository CI additionally runs the full test,
strict typing, formatting, Clippy, Python 3.10, and Rust 1.85 contracts.

The completion run on Python 3.12 and Rust 1.96 passed 470 Python tests, 168 Rust tests, and 604
paired behavioral checks. The same 168 Rust tests and all three registry-package checks also passed
under Rust 1.85.
