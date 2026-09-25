"""Private process boundary for an isolated persistent-turn runtime.

Each invocation inherits one explicit runtime home. No process-global module
paths are changed while an MCP server or another session is using them.
"""
from __future__ import annotations

import json
import sys
from dataclasses import asdict, is_dataclass
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from agentctl.foreign import lib
from agentctl.jsonx import as_mapping, get_str

_RPC_SCHEMA = "agentctl-worker-rpc/v2"


def _optional_text(request: dict[str, object], key: str) -> str | None:
    value = request.get(key)
    if value is not None and not isinstance(value, str):
        raise ValueError(f"{key} must be a string")
    return value


def _optional_int(request: dict[str, object], key: str) -> int | None:
    value = request.get(key)
    if value is not None and (not isinstance(value, int) or isinstance(value, bool)):
        raise ValueError(f"{key} must be an integer")
    return value


def _string_array(request: dict[str, object], key: str) -> list[str]:
    value = request.get(key, [])
    if not isinstance(value, list) or any(
        not isinstance(item, str) or not item or "\0" in item for item in value
    ):
        raise ValueError(f"{key} must be an array of nonempty NUL-free strings")
    return value


def dispatch(request: dict[str, object]) -> dict[str, object]:
    """Execute one explicitly supported runtime operation."""
    if request.get("schema") != _RPC_SCHEMA:
        raise ValueError("runtime request has an unsupported schema")
    action = get_str(request, "action", "runtime request")
    name = get_str(request, "name", "runtime request")
    owner_token = get_str(request, "owner_token", "runtime request")
    paused = request.get("desired_paused")
    if not isinstance(paused, bool):
        raise ValueError("desired_paused must be a boolean")
    permission_mode = _optional_text(request, "permission_mode")
    if permission_mode not in (None, "native", "bypass"):
        raise ValueError("permission_mode must be native or bypass")
    bypass_permissions = (
        None if permission_mode is None else permission_mode == "bypass"
    )
    result: object
    if action == "start":
        cwd = get_str(request, "cwd", "start")
        harness = get_str(request, "harness", "start")
        model = _optional_text(request, "model")
        backend = get_str(request, "backend", "start")
        harness_args = _string_array(request, "harness_args")
        existing = lib.read_registry().get(name)
        if existing is None:
            result = lib.bring_up_agent(
                name, cwd=cwd, harness=harness, model=model,
                brief=_optional_text(request, "brief"), backend=backend,
                mode="headless", harness_args=harness_args,
                owner_token=owner_token,
                bypass_permissions=bypass_permissions,
            )
        else:
            existing = lib.bind_owner_launch(
                name, owner_token, cwd=cwd, harness=harness, model=model,
                backend=backend, harness_args=tuple(harness_args),
                bypass_permissions=bypass_permissions,
            )
            result = lib.status_snapshot(name, run_gc=False)
    else:
        # A committed stop removes the live inner registry row before its
        # response can reach the outer process.  On retry, the immutable
        # token-bound stop receipt is the authority; requiring a live row first
        # would make the committed transaction unrecoverable.
        if action != "stop":
            lib.verify_owner_permission(name, owner_token, permission_mode)
        else:
            try:
                lib.verify_owner_permission(name, owner_token, permission_mode)
            except lib.AgentOperationError as exc:
                if exc.code != "unknown_agent":
                    raise
        if action == "status":
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = lib.status_snapshot(name, run_gc=False)
        elif action == "send":
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = lib.send_message_to_agent(
                name, get_str(request, "text", "send"),
                model=_optional_text(request, "model"),
            )
        elif action == "read":
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = lib.read_agent_output(
                name,
                mode=get_str(request, "mode", "read"),
                since_turn=_optional_int(request, "since_turn"),
                tail=_optional_int(request, "tail"),
            )
        elif action == "stop":
            result = lib.stop_owned_agent(name, owner_token)
        elif action == "reset":
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = lib.reset_agent_context(name)
        elif action == "migrate":
            lib.reconcile_automation_pause(name, owner_token, paused)
            mode = _optional_text(request, "mode")
            result = lib.migrate_agent(
                name,
                to_backend=get_str(request, "backend", "migrate"),
                to_mode="tui" if mode == "interactive" else mode,
            )
        elif action == "repair":
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = lib.recreate_window(name)
        elif action in ("pause", "resume"):
            lib.reconcile_automation_pause(name, owner_token, paused)
            result = {"paused": paused}
        else:
            raise ValueError(f"unsupported runtime operation: {action}")
    value: object = asdict(result) if is_dataclass(result) and not isinstance(result, type) else result
    record = lib.read_registry().get(name)
    if record is not None and record.owner_token != owner_token:
        raise lib.AgentOperationError(
            "owner_token_mismatch",
            f"runtime {name!r} changed session generation during {action}",
        )
    return {
        "result": value,
        "record": record.to_public_dict() if record is not None else None,
    }


def _main() -> int:
    """Exchange exactly one JSON request and response over stdin/stdout."""
    action: str | None = None
    owner_token: str | None = None
    try:
        request = as_mapping(json.load(sys.stdin), "runtime request")
        raw_action = request.get("action")
        action = raw_action if isinstance(raw_action, str) else None
        raw_owner_token = request.get("owner_token")
        owner_token = raw_owner_token if isinstance(raw_owner_token, str) else None
        result = dispatch(request)
    except (lib.AgentOperationError, ValueError, TypeError, OSError) as exc:
        json.dump({
            "schema": _RPC_SCHEMA,
            "action": action,
            "owner_token": owner_token,
            "ok": False,
            "error": {
                "code": getattr(exc, "code", "runtime_failure"),
                "message": str(exc),
            },
        }, sys.stdout)
        sys.stdout.write("\n")
        return 1
    json.dump({
        "schema": _RPC_SCHEMA,
        "action": action,
        "owner_token": owner_token,
        "ok": True,
        "payload": result,
    }, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
