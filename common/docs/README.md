# Shared documentation

The paired tools keep language-neutral prose here and inject only distribution-specific install and
invocation text:

| Tool | Shared sources | Generated package documents |
| --- | --- | --- |
| `dagrun` | `README.template.md`, `USER_GUIDE.template.md` | `rendered/python/`, `rendered/rust/` |
| `tick-hub` | `README.template.md`, `USER_GUIDE.template.md` | `rendered/python/`, `rendered/rust/` |
| `pr-landing-planner` | `README.template.md`, `USER_GUIDE.template.md` | `rendered/python/`, `rendered/rust/` |
| `herdr-run`, `herdr-agent` | `README.template.md`, `USER_GUIDE.template.md`, shared `AGENT_USER_GUIDE.md` | `rendered/python/`, `rendered/rust/` plus exact guide links |

Each template contains one `{{DISTRIBUTION}}` placeholder. The corresponding
`fragments/python/` or `fragments/rust/` document supplies package-specific text. Package trees link
to the generated documents; packaged artifacts contain ordinary files and remain self-contained.

`agent-team-timeline/` is a single-implementation tool, so its `README.md` and `USER_GUIDE.md` are
canonical directly. `dagrun/PLANNER_DESIGN.md` specifies the CPA allocator shared by that tool's
two implementations.

After editing a template, fragment, or single-implementation document, refresh and verify the
outputs:

```sh
python3 scripts/embed_userguides.py
python3 scripts/embed_userguides.py --check
```
