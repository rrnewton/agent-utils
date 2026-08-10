# Adversarial review evidence

These reports record independent post-port reviews of the paired command-line
tools and their distribution artifacts. They are evidence, not a substitute for
the executable contract: every reported behavior is also pinned by unit or
differential tests.

Reviews completed from 2026-08-05 through 2026-08-09:

- [tick-hub](tick-hub.md)
- [packaging and documentation](packaging-and-docs.md)
- [safe-ci-dag-runner and cpuset-alloc](safe-ci-dag-runner.md)
- [pr-landing-planner](pr-landing-planner.md)
- [herdr-run](herdr-run.md)
- [herdr-agent](herdr-agent.md)
- [repository-local Rust source launchers](rust-source-launchers.md)

Supporting design research is kept outside distributable package trees:

- [agent-team-timeline related work](agent-team-timeline-related-work.md)
- [agent-team-timeline comparative analysis](research/agent-team-timeline-comparative-analysis.md)

The standard reproducibility entry points are:

```sh
python3 scripts/embed_userguides.py --check
make check-python-packages
make check-rust-packages
make cross
```

Randomized differentials always record their fixture count and seed so a failed
case can be regenerated exactly.
