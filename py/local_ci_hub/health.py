"""Health: run configured probes once; scheduling is external."""
from __future__ import annotations


import subprocess

from .config import Config, short_host


def builtin_load_probe() -> dict[str, object]:
    """Return the built-in host load probe result."""
    try:
        import os
        load = os.getloadavg()[0]
        return {"name": "load", "status": "pass" if load < 32 else "fail", "load1": round(load, 2), "host": short_host()}
    except Exception as exc:
        return {"name": "load", "status": "unknown", "error": str(exc)}


def run_probes(cfg: Config, only: str = "") -> dict[str, object]:
    """Run configured health probes once and summarize."""
    results = [builtin_load_probe()]
    for probe in cfg.probes:
        name = str(probe.get("name", "probe"))
        if only and name != only:
            continue
        cmd_raw = probe.get("cmd") or probe.get("command") or []
        cmd: list[str] = cmd_raw.split() if isinstance(cmd_raw, str) else [str(c) for c in cmd_raw] if isinstance(cmd_raw, list) else []
        try:
            r = subprocess.run(list(cmd), cwd=cfg.root, capture_output=True, text=True, timeout=int(str(probe.get("timeout_secs", 60))))
            results.append({"name": name, "status": "pass" if r.returncode == 0 else "fail",
                            "exit_code": r.returncode, "summary": (r.stdout or r.stderr).strip().splitlines()[-1][:200] if (r.stdout or r.stderr).strip() else ""})
        except Exception as exc:
            results.append({"name": name, "status": "unknown", "error": str(exc)})
    overall = "fail" if any(r["status"] != "pass" for r in results) else "pass"
    return {"schema": "local-ci-hub/health/v1", "status": overall, "probes": results}
