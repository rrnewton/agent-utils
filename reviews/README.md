# Adversarial review evidence

These reports record independent post-port reviews of the paired command-line
tools and their distribution artifacts. They are evidence, not a substitute for
the executable contract: every reported behavior is also pinned by unit or
differential tests.

Reviews completed on 2026-08-05:

- [tick-hub](tick-hub.md)
- [packaging and documentation](packaging-and-docs.md)
- [safe-ci-dag-runner and cpuset-alloc](safe-ci-dag-runner.md)
- [pr-landing-planner](pr-landing-planner.md)

The standard reproducibility entry points are:

```sh
python3 scripts/embed_userguides.py --check
make check-python-packages
make check-rust-packages
make cross
```

Randomized differentials always record their fixture count and seed so a failed
case can be regenerated exactly.
