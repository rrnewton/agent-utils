"""Append-only validation ledger: JSONL events, fold, queries."""
from __future__ import annotations

import hashlib
import json
import uuid
from datetime import datetime, timezone
from pathlib import Path

from .config import Config, short_host

SCHEMA = "validate-ledger/v1"


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def canonical_bytes(event: dict) -> bytes:
    return json.dumps(event, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(event: dict) -> str:
    return hashlib.sha256(canonical_bytes(event)).hexdigest()


class Ledger:
    def __init__(self, cfg: Config):
        self.cfg = cfg

    def _files(self) -> list[Path]:
        f = self.cfg.ledger_file
        return [f] if f.exists() else []

    def events(self) -> list[dict]:
        out: list[dict] = []
        for f in self._files():
            for line in f.read_text(errors="replace").splitlines():
                line = line.strip()
                if not line:
                    continue
                try:
                    out.append(json.loads(line))
                except json.JSONDecodeError:
                    out.append({"schema": SCHEMA, "kind": "unreadable", "raw": line[:200]})
        return out

    def rows(self) -> list[dict]:
        """Fold events into terminal rows keyed by run_id (later events win per field)."""
        rows: dict[str, dict] = {}
        order: list[str] = []
        for ev in self.events():
            rid = ev.get("run_id")
            if not rid:
                continue
            if rid not in rows:
                rows[rid] = {"run_id": rid}
                order.append(rid)
            if ev.get("kind") in ("run.result", "run.correct", "run.enrich", None):
                for k, v in ev.items():
                    if k not in ("kind",):
                        rows[rid][k] = v
        return [rows[r] for r in order]

    def append_result(self, *, commit: str, result: str, run_id: str | None = None,
                      cell: str = "", command: str = "", host: str | None = None) -> dict:
        run_id = run_id or f"run-{uuid.uuid4().hex[:12]}"
        ev = {
            "schema": SCHEMA, "kind": "run.result", "event_id": uuid.uuid4().hex,
            "run_id": run_id, "commit": commit, "result": result.upper(),
            "cell": cell, "command": command, "host": host or short_host(),
            "team": self.cfg.team, "finished_at": _now(),
        }
        ev["digest"] = digest({k: v for k, v in ev.items() if k != "digest"})
        self.cfg.spool_dir.mkdir(parents=True, exist_ok=True)
        spool = self.cfg.spool_dir / f"{ev['event_id']}.jsonl"
        spool.write_text(json.dumps(ev, sort_keys=True) + "\n")
        return ev

    def publish(self) -> dict:
        """Move spooled events into the ledger file (idempotent by event_id)."""
        f = self.cfg.ledger_file
        f.parent.mkdir(parents=True, exist_ok=True)
        seen = {e.get("event_id") for e in self.events()}
        moved = 0
        if self.cfg.spool_dir.exists():
            for spool in sorted(self.cfg.spool_dir.glob("*.jsonl")):
                for line in spool.read_text().splitlines():
                    if not line.strip():
                        continue
                    ev = json.loads(line)
                    if ev.get("event_id") in seen:
                        continue
                    with f.open("a") as fh:
                        fh.write(json.dumps(ev, sort_keys=True) + "\n")
                    seen.add(ev.get("event_id"))
                    moved += 1
                spool.rename(spool.with_suffix(".published"))
        return {"published": moved, "ledger": str(f)}

    def for_commit(self, commit: str) -> list[dict]:
        return [r for r in self.rows() if r.get("commit", "").startswith(commit) or commit.startswith(r.get("commit", "~"))]

    def newest_green(self, cell: str = "") -> dict | None:
        greens = [r for r in self.rows() if r.get("result") == "PASS" and (not cell or r.get("cell", "") == cell)]
        return greens[-1] if greens else None

    def first_bad(self, cell: str = "") -> dict | None:
        seq = [r for r in self.rows() if not cell or r.get("cell", "") == cell]
        prev = None
        for r in seq:
            if prev and prev.get("result") == "PASS" and r.get("result") == "FAIL":
                return r
            prev = r
        return None
