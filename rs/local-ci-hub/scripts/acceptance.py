#!/usr/bin/env python3
"""Generic oracle-versus-Rust acceptance for the local-ci-hub front door.

Parameterized only by a deployment config file (the `--config` JSON the
front door itself reads) and three caller-supplied oracle shell
commands. No deployment path, field name, producer name or oracle
location is baked in; the caller keeps those in the deployment's own
files. See rs/local-ci-hub/README.md (Deployment, Acceptance).

Checks (any failure exits 1 and names the first mismatches):
  - folded terminal sets are identical (oracle rows reduced to
    run_id/commit/result/host/finished_at);
  - verdict agreement with looser == 0 (stricter commits listed);
  - per-run_id digests equal, including null-digest counts;
  - index stability: cold, fresh-index and warm-index verdicts are
    byte-identical.

No timing property is asserted; wrap the Rust invocations externally
if wall/CPU/RSS figures are wanted.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

TERMINAL_KEYS = ("run_id", "commit", "result", "host", "finished_at")


def run_checked(argv: list[str], label: str) -> bytes:
    done = subprocess.run(argv, capture_output=True, check=False)
    if done.returncode != 0:
        sys.stderr.write(done.stderr.decode("utf-8", errors="replace"))
        raise SystemExit(f"acceptance: {label} failed with exit {done.returncode}")
    return done.stdout


def run_oracle(command: str, label: str) -> str:
    done = subprocess.run(command, shell=True, capture_output=True, text=True, check=False)
    if done.returncode != 0:
        sys.stderr.write(done.stderr)
        raise SystemExit(f"acceptance: {label} failed with exit {done.returncode}")
    return done.stdout


def load_jsonl(text: str, label: str) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    for lineno, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise SystemExit(f"acceptance: {label} line {lineno} is not JSON: {exc}") from exc
        if not isinstance(value, dict):
            raise SystemExit(f"acceptance: {label} line {lineno} is not a JSON object")
        rows.append(value)
    return rows


def terminal_text(row: dict[str, object]) -> str:
    view = {key: row[key] for key in TERMINAL_KEYS if key in row}
    return json.dumps(view, sort_keys=True, separators=(",", ":"))


def qualified_set(rows: list[dict[str, object]], bare_lines: list[str]) -> set[str]:
    qualified: set[str] = set()
    for row in rows:
        commit = row.get("commit")
        if isinstance(commit, str) and row.get("qualified") is True:
            qualified.add(commit)
    for line in bare_lines:
        text = line.strip()
        if text and not text.startswith("{"):
            qualified.add(text)
    return qualified


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, help="deployment config JSON for the front door")
    parser.add_argument("--work-dir", required=True, help="scratch directory for Rust outputs")
    parser.add_argument("--local-ci-hub", default="local-ci-hub", help="front-door binary path")
    parser.add_argument("--oracle-fold", required=True, help="shell command printing folded rows as JSONL")
    parser.add_argument("--oracle-verdicts", required=True, help="shell command printing qualified commits")
    parser.add_argument("--oracle-digests", required=True, help="shell command printing digest JSONL")
    args = parser.parse_args()

    config_path = Path(args.config)
    work = Path(args.work_dir)
    work.mkdir(parents=True, exist_ok=True)
    hub = args.local_ci_hub

    # Cold verdicts must not reuse the config's file index. Write a
    # work-local copy with the index removed; the front door's own
    # validation still checks every other key when it reads the copy.
    try:
        config_data = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"acceptance: cannot read --config {config_path}: {exc}") from exc
    if not isinstance(config_data, dict):
        raise SystemExit(f"acceptance: --config {config_path} is not a JSON object")
    cold_data = dict(config_data)
    cold_data["index"] = None
    cold_config = work / "acceptance-cold-config.json"
    cold_config.write_text(json.dumps(cold_data, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    def rust(config: Path, extra: list[str], *subcommand: str) -> bytes:
        return run_checked([hub, "--config", str(config), *extra, *subcommand], " ".join(subcommand))

    index_path = work / "acceptance-index.sqlite3"
    for suffix in ("", "-wal", "-shm"):
        stale = Path(str(index_path) + suffix)
        if stale.exists():
            stale.unlink()

    terminals_raw = rust(cold_config, [], "ledger", "rows", "--terminals")
    verdicts_cold = rust(cold_config, [], "ledger", "verdicts")
    digests_raw = rust(cold_config, [], "ledger", "digest-rows")
    verdicts_indexed = rust(cold_config, ["--index", str(index_path)], "ledger", "verdicts")
    verdicts_warm = rust(cold_config, ["--index", str(index_path)], "ledger", "verdicts")

    outputs = {
        "rust-terminals.jsonl": terminals_raw,
        "rust-verdicts.jsonl": verdicts_cold,
        "rust-digests.jsonl": digests_raw,
    }
    for name, blob in outputs.items():
        (work / name).write_bytes(blob)

    failures: list[str] = []

    oracle_fold_text = run_oracle(args.oracle_fold, "oracle fold")
    oracle_rows = load_jsonl(oracle_fold_text, "oracle fold")
    oracle_terminals = sorted(terminal_text(row) for row in oracle_rows)
    rust_terminals = sorted(
        line.strip()
        for line in terminals_raw.decode("utf-8", errors="replace").splitlines()
        if line.strip()
    )
    if oracle_terminals != rust_terminals:
        only_oracle = sorted(set(oracle_terminals) - set(rust_terminals))[:5]
        only_rust = sorted(set(rust_terminals) - set(oracle_terminals))[:5]
        failures.append(
            f"fold terminals differ (oracle={len(oracle_terminals)} rust={len(rust_terminals)} "
            f"only-oracle={only_oracle} only-rust={only_rust})"
        )

    oracle_verdict_text = run_oracle(args.oracle_verdicts, "oracle verdicts")
    oracle_rows_v = load_jsonl(
        "\n".join(line for line in oracle_verdict_text.splitlines() if line.strip().startswith("{")),
        "oracle verdicts",
    )
    oracle_bare = [line for line in oracle_verdict_text.splitlines() if line.strip() and not line.strip().startswith("{")]
    oracle_qualified = qualified_set(oracle_rows_v, oracle_bare)
    rust_verdicts = load_jsonl(verdicts_cold.decode("utf-8", errors="replace"), "rust verdicts")
    rust_qualified = qualified_set(rust_verdicts, [])
    all_commits = sorted({str(r.get("commit")) for r in rust_verdicts if r.get("commit")} | oracle_qualified)
    looser = sorted(rust_qualified - oracle_qualified)
    stricter = sorted(oracle_qualified - rust_qualified)
    agree = sum(1 for c in all_commits if (c in oracle_qualified) == (c in rust_qualified))
    if looser:
        failures.append(f"looser must be 0, got {len(looser)}: {looser[:10]}")

    oracle_digest_text = run_oracle(args.oracle_digests, "oracle digests")
    oracle_digests = load_jsonl(oracle_digest_text, "oracle digests")
    rust_digests = load_jsonl(digests_raw.decode("utf-8", errors="replace"), "rust digests")

    def digest_map(rows: list[dict[str, object]], label: str) -> dict[str, object]:
        out: dict[str, object] = {}
        for row in rows:
            run_id = row.get("run_id")
            if not isinstance(run_id, str):
                raise SystemExit(f"acceptance: {label} row without string run_id: {row!r}")
            if run_id in out:
                raise SystemExit(f"acceptance: {label} repeats run_id {run_id}")
            out[run_id] = row.get("digest")
        return out

    oracle_map = digest_map(oracle_digests, "oracle digests")
    rust_map = digest_map(rust_digests, "rust digests")
    oracle_null = sum(1 for v in oracle_map.values() if v is None)
    rust_null = sum(1 for v in rust_map.values() if v is None)
    if set(oracle_map) != set(rust_map):
        failures.append(
            f"digest run_id sets differ (oracle={len(oracle_map)} rust={len(rust_map)} "
            f"only-oracle={sorted(set(oracle_map) - set(rust_map))[:5]} "
            f"only-rust={sorted(set(rust_map) - set(oracle_map))[:5]})"
        )
    else:
        mismatched = [rid for rid in oracle_map if oracle_map[rid] != rust_map[rid]]
        if mismatched:
            failures.append(f"digest values differ for {len(mismatched)} run_id(s): {sorted(mismatched)[:5]}")
    if oracle_null != rust_null:
        failures.append(f"null digest counts differ (oracle={oracle_null} rust={rust_null})")

    if not (verdicts_cold == verdicts_indexed == verdicts_warm):
        failures.append("index stability failed (cold/indexed/warm verdicts are not byte-identical)")

    print(
        f"acceptance: commits={len(all_commits)} agree={agree} looser={len(looser)} "
        f"stricter={len(stricter)} terminals oracle={len(oracle_terminals)} rust={len(rust_terminals)} "
        f"digests total={len(rust_map)} null={rust_null} index_stable={verdicts_cold == verdicts_indexed == verdicts_warm}"
    )
    if stricter:
        print(f"acceptance: stricter commits (each needs a documented reason): {stricter}")
    if failures:
        for failure in failures:
            print(f"acceptance: FAIL {failure}")
        return 1
    print("acceptance: OK (looser=0, terminals equal, digests equal, index stable)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
