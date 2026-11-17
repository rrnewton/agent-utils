"""local-ci-hub: validation on a dev box, with local+remote evidence in one ledger."""
from __future__ import annotations


import argparse
import json
import shutil
import sys
from pathlib import Path

from . import health as health_mod
from . import host_limits
from . import land as land_mod
from . import receipt as receipt_mod
from . import remote as remote_mod
from . import runner
from .config import ConfigError, load
from .ledger import Ledger, digest

QUICKSTART = """local-ci-hub quickstart (toy/local-only loop, under 5 minutes)
  1. Write local-ci-hub.toml (see `local-ci-hub userguide`), or rely on local-only defaults.
  2. local-ci-hub doctor
  3. local-ci-hub run start --wait        # runs your validate command, records the ledger
  4. local-ci-hub receipt show HEAD       # what is proven about this commit?
  5. local-ci-hub ledger query            # the evidence behind the verdict
Health: local-ci-hub health. Remote (if configured): local-ci-hub remote status.
Landing primitives: local-ci-hub land acquire|release|verify."""

USERGUIDE = """local-ci-hub userguide
Objects: run (executions on this machine), ledger (append-only evidence),
receipt (verdict for one exact commit), remote (GitHub read/dispatch),
health (configured probes; scheduling is external), land (lock+verify).
Config: local-ci-hub.toml in the project root. [validate] command is yours;
[ledger] location/team/publish (none|git) and counted_repos (repository
identities this ledger counts; rows with no repo always count);
[remote] provider (none|github);
[receipt] qualifying is an AND list from {local-ledger-pass, remote-pass}
and release_payload is the canonical release identity a PASS must carry.
Host limits: one admin-installed host file caps concurrency, cleanup
debt, disk floor and per-run box ceilings for every instance on the
host; project requests only clamp down under it. With no host file the
tool runs on conservative built-in limp limits and warns on every run;
doctor reports the missing file as an error and shows every effective
value with its source. Admission is live, from measured headroom: a
refusal is no result, never a test failure. Sweep deletions defer
while a measurement hold or host quiet is active.
Local-only: provider=none and publish=none needs no network or credentials.
Every subcommand accepts --json. Exit codes: 0 ok/qualified/pass,
1 not-qualified or real test failure, 2 usage/config error,
75 could-not-run or refused (no result was produced)."""


def _out(obj: object, as_json: bool) -> None:
    if as_json or not isinstance(obj, (dict, list)):
        print(json.dumps(obj, indent=1, sort_keys=True) if isinstance(obj, (dict, list)) else obj)
        return
    if isinstance(obj, dict):
        for k, v in obj.items():
            print(f"{k}: {v}")
    else:
        for row in obj if isinstance(obj, list) else []:
            print(row)


def main(argv: list[str] | None = None) -> int:
    """Run the local-ci-hub command line; returns the process exit code."""
    if "--userguide" in (argv if argv is not None else sys.argv[1:]):
        print(USERGUIDE)
        return 0
    p = argparse.ArgumentParser(prog="local-ci-hub", description="Validation on a dev box: run, ledger, receipt, remote, health, land.")
    p.add_argument("--config", type=Path)
    p.add_argument("--json", action="store_true")
    p.add_argument("--version", action="version", version="local-ci-hub 0.1.0")
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("quickstart")
    sub.add_parser("userguide")
    sub.add_parser("doctor")
    r = sub.add_parser("run"); rs = r.add_subparsers(dest="op", required=True)
    for op in ("start", "list", "stop", "status", "sweep"):
        q = rs.add_parser(op)
        q.add_argument("--wait", action="store_true"); q.add_argument("--commit", default=""); q.add_argument("--cell", default=""); q.add_argument("--admission-wait-secs", type=float, default=0.0); q.add_argument("run_id", nargs="?")
    led = sub.add_parser("ledger"); ls = led.add_subparsers(dest="op", required=True)
    for op in ("append", "publish", "query", "newest-green", "first-bad", "ingest", "digest"):
        q = ls.add_parser(op)
        q.add_argument("--commit", default=""); q.add_argument("--result", default="PASS"); q.add_argument("--cell", default="")
    rec = sub.add_parser("receipt"); rec.add_argument("op", choices=["show", "verify", "era-list"]); rec.add_argument("commit", nargs="?", default="")
    rem = sub.add_parser("remote"); rem.add_argument("op", choices=["status", "dispatch"]); rem.add_argument("--workflow", default=""); rem.add_argument("--ref", default="")
    hea = sub.add_parser("health"); hea.add_argument("--probe", default="")
    lan = sub.add_parser("land"); lan.add_argument("op", choices=["acquire", "release", "run", "verify"]); lan.add_argument("--holder", default="agent"); lan.add_argument("--commit", default=""); lan.add_argument("--target", default="HEAD"); lan.add_argument("command", nargs="*")
    args = p.parse_args(argv)
    if args.cmd == "quickstart":
        print(QUICKSTART); return 0
    if args.cmd == "userguide":
        print(USERGUIDE); return 0
    try:
        cfg = load(args.config)
    except ConfigError as exc:
        print(f"config error: {exc}", file=sys.stderr); return 2
    if args.cmd == "doctor":
        import os as _os
        import time as _time

        eff = host_limits.load_effective(reader="doctor")
        head = host_limits.sample_headroom()
        handles = eff.state_root / "runs"
        handles.mkdir(parents=True, exist_ok=True)
        v, y, pending = runner.count_live(handles, _time.time())
        free_bytes, free_components = host_limits.measure_free_bytes(cfg.root)
        gate_g, gate_cap, gate_note = host_limits.count_gate_runs()
        deferred, defer_sources = host_limits.sweep_deferral(_time.time())
        total = host_limits.mem_total_bytes()
        sanity: list[str] = []
        if total > 0 and eff.memory_max_bytes > total:
            sanity.append("run_scope.memory_max_bytes exceeds MemTotal")
        other_root = cfg.state_dir / "runs"
        other_handles = 0
        if other_root.is_dir() and other_root.resolve() != handles.resolve():
            other_handles = sum(1 for _ in other_root.glob("*.json"))
        host_section: dict[str, object] = {
            "limits": eff.as_dict(), "headroom": head.as_dict(),
            "live": {"v": v, "y": y, "p": pending, "cleanup_pending_max": eff.cleanup_pending_max},
            "free": {"bytes": free_bytes, "floor": eff.slot_fs_min_free_bytes,
                     "components": free_components},
            "gate": {"g": gate_g, "g_cap": gate_cap, "note": gate_note,
                     "reads_host_limits": False},
            "sweep_deferral": {"deferred": deferred, "sources": defer_sources},
            "other_root_handles": other_handles,
            "mem_total_bytes": total, "sanity_errors": sanity,
            "unknown_key_errors": list(eff.unknown_keys),
            "state_root_problems": host_limits.check_state_root(eff.state_root),
        }
        if other_handles:
            host_section["other_root_error"] = (
                f"run handles exist under another root {other_root}; "
                "cleanup debt there stays visible until swept or retired")
        checks = {
            "git": bool(shutil.which("git")), "config": str(cfg.path),
            "validate_command": cfg.validate_command, "provider": cfg.provider,
            "ledger": str(cfg.ledger_file), "gh_needed": cfg.provider != "none",
            "gh": bool(shutil.which("gh")) if cfg.provider != "none" else "not-required",
            "host_limits": host_section,
            "host": _os.uname().nodename.split(".")[0] if hasattr(_os, "uname") else "",
        }
        ok = bool(checks["git"]) and not eff.errors and not sanity and not other_handles
        _out({"schema": "local-ci-hub/doctor/v1", "ok": ok, "checks": checks}, args.json)
        return 0 if ok else 1
    if args.cmd == "run":
        res: object = {}
        if args.op == "start":
            res = runner.start(cfg, wait=True, commit=args.commit, cell=args.cell,
                               admission_wait_s=args.admission_wait_secs)
        elif args.op == "sweep":
            res = runner.sweep(cfg)
        elif args.op == "stop":
            res = runner.stop(cfg, args.run_id or "")
        else:
            res = runner.list_runs(cfg)
        _out(res, args.json)
        if isinstance(res, dict):
            if res.get("state") == "refused" or res.get("outcome") == "no_result":
                return 75
            if res.get("result") in ("FAIL", "TIMEOUT", "fail", "timeout"):
                return 1
            code = res.get("exit_code")
            if isinstance(code, int) and code in (0, 1, 75):
                return code
        return 0
    if args.cmd == "ledger":
        ledger = Ledger(cfg)
        if args.op == "append":
            res = ledger.append_result(commit=args.commit, result=args.result, cell=args.cell)
        elif args.op in ("publish", "ingest"):
            Ledger(cfg).publish(); res = {"rows": len(ledger.rows())}
        elif args.op == "digest":
            row = (ledger.for_commit(args.commit) or [{}])[0]
            res = {"commit": args.commit, "digest": ledger.external_digest(row) or digest(row), "source": "external" if ledger.external_digest(row) else "builtin"}
        elif args.op == "newest-green":
            res = ledger.newest_green(args.cell) or {"result": "none"}
        elif args.op == "first-bad":
            res = ledger.first_bad(args.cell) or {"result": "none"}
        else:
            rows_out = ledger.rows() if not args.commit else ledger.for_commit(args.commit)
            _out(rows_out, args.json); return 0
        _out(res, args.json); return 0
    if args.cmd == "receipt":
        if args.op == "era-list":
            import hashlib as _hl
            from .ledger import Ledger as _Led, canonical_bytes as _canon
            if cfg.era_records:
                print("era_records is already set; refusing to regenerate the frozen era list", file=sys.stderr)
                return 2
            entries: list[str] = []
            for row in _Led(cfg).rows():
                ver = row.get("schema_version")
                if isinstance(ver, int) and not isinstance(ver, bool) and ver >= 6:
                    continue  # only below-schema-6 rows can be era records
                body = {k: v for k, v in row.items() if not str(k).startswith("_") and k != "digest"}
                ident = row.get("record_id") or row.get("run_id")
                if not isinstance(ident, str) or not ident:
                    continue
                entries.append(f"{ident}:{_hl.sha256(_canon(body)).hexdigest()}")
                if args.commit and ident == args.commit:
                    break  # bounded by the explicit cutoff record
            if args.json:
                _out({"schema": "local-ci-hub/era-list/v1", "era_records": entries}, True)
            else:
                print("era_records = [" + ", ".join(f'"{e}"' for e in entries) + "]")
            return 0
        res = receipt_mod.show(cfg, args.commit)
        _out(res, args.json)
        return 0 if res["qualified"] else 1
    if args.cmd == "remote":
        res = remote_mod.dispatch(cfg, args.workflow, args.ref) if args.op == "dispatch" else remote_mod.status(cfg)
        _out(res, args.json); return 0 if res.get("status") in ("ok", "disabled") else 1
    if args.cmd == "health":
        res = health_mod.run_probes(cfg, args.probe)
        _out(res, args.json); return 0 if res["status"] == "pass" else 1
    if args.cmd == "land":
        if args.op == "acquire":
            res = land_mod.acquire(cfg, args.holder)
        elif args.op == "release":
            res = land_mod.release(cfg, args.holder)
        elif args.op == "run":
            cmd = args.command[1:] if args.command[:1] == ["--"] else args.command
            res = land_mod.run_locked(cfg, args.holder, cmd) if cmd else {"status": "usage-error", "reason": "land run needs a command after --"}
            _out(res, args.json); code = res.get("exit_code", 1); return code if isinstance(code, int) else 1
        else:
            res = land_mod.verify(cfg, args.commit, args.target)
        _out(res, args.json); return 0 if res.get("status") in ("acquired", "released", "free") or res.get("ancestor") else 1
    return 2
