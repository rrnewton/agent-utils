# Repository checks and documentation tooling

These scripts enforce contracts that span otherwise independent packages:

| Script | Purpose |
| --- | --- |
| `embed_userguides.py` | Render shared documentation and verify package links and standalone prose. |
| `check_deps.py` | Smoke every command in a minimal environment to catch import-time dependency failures. |
| `check_no_any.py` | Reject explicit `typing.Any` use at typed boundaries. |
| `check_python_packages.py` | Build each wheel and sdist, rebuild from the sdist, then inspect, install, and smoke every Python distribution in isolation. |
| `check_rust_packages.py` | Package and inspect each Rust crate in isolation. |

Run the focused checks directly:

```sh
python3 scripts/embed_userguides.py --check
python3 scripts/check_deps.py
python3 scripts/check_no_any.py .
make check-packages
```

Use `make check`, `make test`, and `make cross` for the wider repository contract.
