"""Append-only validation ledger: JSONL events, fold, queries."""
from __future__ import annotations


import hashlib
import json
import re
import subprocess
import uuid
from datetime import datetime, timezone
from pathlib import Path

from .config import Config, short_host

SCHEMA = "validate-ledger/v1"
_ROW_REF_LOCATOR = re.compile(r"run-evidence/([A-Za-z0-9_-]+)/([0-9a-f]{64})\.json\.zst")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_MAX_ROW_BYTES = 48 * 1024 * 1024
_MAX_COMPRESSED_BYTES = _MAX_ROW_BYTES + 1024 * 1024
_ROW_REF_FIELDS = frozenset({"codec", "sha256", "bytes", "locator", "compressed_sha256", "compressed_bytes"})
# Repository identities this (Hermit) ledger counts, mirrored from ci-hub
# `qualifying_receipt.py` `_HERMIT_REPOS`: an absent `repo` is a Hermit row.
# Schema-3 `repo=reverie` rows physically inside Hermit shards are Reverie
# evidence and must not count here.
_HERMIT_REPOS = (None, "hermit", "rrnewton/hermit")


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def canonical_bytes(event: dict[str, object]) -> bytes:
    """Serialize an event to its canonical digest bytes."""
    return json.dumps(event, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(event: dict[str, object]) -> str:
    """Return the SHA-256 digest of an event canonical bytes."""
    return hashlib.sha256(canonical_bytes(event)).hexdigest()


class Ledger:
    """An append-only JSONL validation ledger."""
    def __init__(self, cfg: Config):
        self.cfg = cfg

    def _files(self) -> list[Path]:
        f = self.cfg.ledger_file
        if f.exists():
            return [f]
        d = self.cfg.ledger_dir
        return sorted(d.rglob("*.jsonl")) if d.exists() else []

    def events(self) -> list[dict[str, object]]:
        """Perform this ledger, receipt, or run operation as documented above."""
        out: list[dict[str, object]] = []
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

    def rows(self) -> list[dict[str, object]]:
        """Fold events into terminal rows. Enrichments never change result/commit;
        corrections only apply to an existing run; digest/schema failures are flagged, not trusted."""
        rows: dict[str, dict[str, object]] = {}
        order: list[str] = []
        seen_ids: dict[str, str] = {}
        for ev in self.events():
            eid_raw = ev.get("event_id")
            eid = eid_raw if isinstance(eid_raw, str) else None
            body = canonical_bytes({k: v for k, v in ev.items() if k != "digest"})
            if eid and eid in seen_ids and seen_ids[eid] != body.decode():
                continue  # conflicting duplicate event_id: first wins
            if eid:
                seen_ids[eid] = body.decode()
            rid_raw = ev.get("run_id")
            rid = rid_raw if isinstance(rid_raw, str) else None
            if not rid:
                continue
            kind = ev.get("kind") or ev.get("event_type") or "run.result"
            if kind == "run.enrich" and rid in rows:
                for k, v in ev.items():
                    if k not in ("result", "commit", "kind", "event_type") and rows[rid].get(k) in (None, ""):
                        rows[rid][k] = v
                continue
            if kind == "run.correct" and rid not in rows:
                continue  # orphan correction: never manufactures a row
            if rid not in rows:
                rows[rid] = {"run_id": rid}
                order.append(rid)
            legacy = ev.get("legacy_row")
            src: dict[str, object] = legacy if isinstance(legacy, dict) and legacy else ev
            evidence_ok = True
            codec_raw = ev.get("legacy_row_codec")
            codec: dict[str, object] = codec_raw if isinstance(codec_raw, dict) else {}
            if codec.get("codec") == "row-ref/v1":
                host_raw = ev.get("host")
                src, evidence_ok = self._resolve_ref(codec, host=host_raw if isinstance(host_raw, str) else None)
                if evidence_ok and (src.get("commit") not in (None, ev.get("commit")) or src.get("run_id") not in (None, rid)):
                    src, evidence_ok = {}, False  # referenced evidence must name the same commit and run
            merged = dict(rows[rid])
            merged.update({k: v for k, v in src.items() if k not in ("kind", "event_type", "legacy_row")})
            for k in ("commit", "host", "team", "run_id", "event_id", "finished_at"):
                if ev.get(k) is not None:
                    merged[k] = ev[k]
            if ev.get("outcome"):
                merged["result"] = str(ev["outcome"]).upper()
            elif src.get("result"):
                merged["result"] = str(src["result"]).upper()
            if "digest" in ev:
                expect = digest({k: v for k, v in ev.items() if k != "digest"})
                merged["_digest_ok"] = (expect == ev["digest"])
            merged["_evidence_ok"] = bool(evidence_ok and rows[rid].get("_evidence_ok", True))  # once refused, always refused
            rows[rid] = merged
        return [rows[r] for r in order if rows[r].get("repo") in _HERMIT_REPOS]

    def _resolve_ref(self, codec: dict[str, object], host: str | None = None) -> tuple[dict[str, object], bool]:
        """Dereference one row-ref/v1 evidence object; (row, ok). Refuses on any mismatch."""
        # Mirror ci-hub ledger_evidence.validate_ref: the locator is the
        # immutable host/content address of the row, not an arbitrary path.
        if set(codec) != _ROW_REF_FIELDS:
            return {}, False
        for name, limit in (("bytes", _MAX_ROW_BYTES), ("compressed_bytes", _MAX_COMPRESSED_BYTES)):
            value = codec.get(name)
            if not isinstance(value, int) or isinstance(value, bool) or not 0 < value <= limit:
                return {}, False
        for name in ("sha256", "compressed_sha256"):
            value = codec.get(name)
            if not isinstance(value, str) or _SHA256.fullmatch(value) is None:
                return {}, False
        loc = codec.get("locator")
        if not isinstance(loc, str):
            return {}, False
        named = _ROW_REF_LOCATOR.fullmatch(loc)
        if named is None or named.group(2) != codec.get("sha256"):
            return {}, False
        if host is not None and named.group(1) != host:
            return {}, False
        root = self.cfg.evidence_root
        if root is None or not loc or Path(loc).is_absolute() or ".." in Path(loc).parts:
            return {}, False
        path = root / loc
        try:
            blob = path.read_bytes()
        except OSError:
            return {}, False
        if hashlib.sha256(blob).hexdigest() != codec.get("compressed_sha256"):
            return {}, False
        if len(blob) != codec.get("compressed_bytes"):
            return {}, False
        proc = subprocess.run(["zstd", "-dc", str(path)], capture_output=True, timeout=60)
        if proc.returncode != 0:
            return {}, False
        raw = proc.stdout
        try:
            row = json.loads(raw)
        except Exception:
            return {}, False
        if not isinstance(row, dict) or hashlib.sha256(raw).hexdigest() != codec.get("sha256") or len(raw) != codec.get("bytes"):
            return {}, False
        return row, True

    def external_digest(self, row: dict[str, object]) -> str | None:
        """Return the configured external canonical digest for a row, or None.

        When a project configures receipt.digest_command, that command is
        the one canonical digest: it receives the row JSON on stdin and
        prints the digest. The built-in digest is only for this tool's
        own events when no external command is configured.
        """
        cmd = self.cfg.digest_command
        if not cmd:
            return None
        proc = subprocess.run(cmd, input=json.dumps(row).encode(), capture_output=True, timeout=60)
        if proc.returncode != 0:
            return None
        return proc.stdout.decode().strip().split()[-1] if proc.stdout.strip() else None

    def append_result(self, *, commit: str, result: str, run_id: str | None = None,
                      cell: str = "", command: str = "", host: str | None = None) -> dict[str, object]:
        """Append one run result to the spool for later publication."""
        run_id = run_id or f"run-{uuid.uuid4().hex[:12]}"
        ev: dict[str, object] = {
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

    def publish(self) -> dict[str, object]:
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

    def for_commit(self, commit: str) -> list[dict[str, object]]:
        """Perform this operation as documented in the module guide."""
        # Exact-commit binding: full SHA equality, or an unambiguous >=12-char prefix.
        out = []
        for r in self.rows():
            c = str(r.get("commit", ""))
            if c and (c == commit or (len(commit) >= 12 and c.startswith(commit))):
                out.append(r)
        return out

    def newest_green(self, cell: str = "") -> dict[str, object] | None:
        """Perform this operation as documented in the module guide."""
        greens = [r for r in self.rows() if r.get("result") == "PASS" and (not cell or r.get("cell", "") == cell)]
        return greens[-1] if greens else None

    def first_bad(self, cell: str = "") -> dict[str, object] | None:
        """Perform this operation as documented in the module guide."""
        seq = [r for r in self.rows() if not cell or r.get("cell", "") == cell]
        prev = None
        for r in seq:
            if prev and prev.get("result") == "PASS" and r.get("result") == "FAIL":
                return r
            prev = r
        return None
