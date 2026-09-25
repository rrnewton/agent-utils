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
from agentctl import agent
from agentctl.errors import AgentDeliveryError
from agentctl.jsonx import as_mapping, get_str
from agentctl.launch_contract import RuntimeLaunchContract

_RPC_SCHEMA = "agentctl-worker-rpc/v2"
_MAX_RPC_REQUEST_BYTES = 1 << 20
_MAX_RPC_RESPONSE_BYTES = 8 << 20
_MAX_RPC_ERROR_BYTES = 32 << 10


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


def dispatch(request: dict[str, object]) -> dict[str, object]:
    """Execute one explicitly supported runtime operation."""
    if request.get("schema") != _RPC_SCHEMA:
        raise ValueError("runtime request has an unsupported schema")
    action = get_str(request, "action", "runtime request")
    fields = {
        "schema", "action", "name", "owner_token", "desired_paused",
        "owner_launch",
    }
    action_fields = {
        "start": {"brief"},
        "status": set(),
        "send": {"text", "model"},
        "read": {"mode", "since_turn", "tail"},
        "stop": set(),
        "reset": set(),
        "migrate": {"backend", "mode"},
        "repair": set(),
        "pause": set(),
        "resume": set(),
    }
    expected_fields = action_fields.get(action)
    if expected_fields is None:
        raise ValueError(f"unsupported runtime operation: {action}")
    if set(request) != fields | expected_fields:
        raise ValueError(f"runtime {action} request has an invalid field set")
    name = get_str(request, "name", "runtime request")
    owner_token = get_str(request, "owner_token", "runtime request")
    try:
        owner_launch = RuntimeLaunchContract.from_document(
            request.get("owner_launch")
        )
    except ValueError as exc:
        raise ValueError(f"runtime request has an invalid owner launch: {exc}") from exc
    paused = request.get("desired_paused")
    if not isinstance(paused, bool):
        raise ValueError("desired_paused must be a boolean")
    if (action == "pause" and not paused) or (action == "resume" and paused):
        raise ValueError(f"runtime {action} request contradicts desired_paused")
    def finish(result: object) -> dict[str, object]:
        value: object = (
            asdict(result)
            if is_dataclass(result) and not isinstance(result, type)
            else result
        )
        record = lib.read_registry().get(name)
        if record is None and action != "stop":
            raise lib.AgentOperationError(
                "owner_launch_lost",
                f"runtime {name!r} disappeared during {action}",
            )
        if record is not None and (
            record.owner_token != owner_token
            or record.owner_launch is None
            or record.owner_launch != owner_launch
        ):
            raise lib.AgentOperationError(
                "owner_launch_mismatch",
                f"runtime {name!r} changed immutable launch during {action}",
            )
        return {
            "result": value,
            "record": record.to_public_dict() if record is not None else None,
        }

    if action == "start":
        with lib.owner_start_operation(name, owner_token, owner_launch):
            existing = lib.read_registry().get(name)
            if existing is None:
                result = lib.bring_up_agent(
                    name, cwd=owner_launch.cwd, harness=owner_launch.harness,
                    model=owner_launch.model,
                    brief=_optional_text(request, "brief"),
                    backend=owner_launch.backend,
                    mode=owner_launch.mode,
                    harness_args=owner_launch.harness_args,
                    owner_token=owner_token,
                    bypass_permissions=owner_launch.permission_mode == "bypass",
                    expected_owner_launch=owner_launch,
                )
            else:
                lib.bind_owner_launch(name, owner_token, owner_launch)
                result = lib.status_snapshot(name, run_gc=False)
            lib.verify_owner_launch(name, owner_token, owner_launch)
            return finish(result)
    else:
        # A committed stop removes the live inner registry row before its
        # response can reach the outer process.  On retry, the immutable
        # launch-bound stop receipt is the authority.  The per-agent operation
        # lock serializes verification through the last mutation and response
        # projection, so no second RPC can change the generation in between.
        with lib.owner_launch_operation(
            name, owner_token, owner_launch,
            allow_terminal_receipt=action == "stop",
        ) as owned:
            if action != "stop" and owned is None:
                raise lib.AgentOperationError(
                    "owner_launch_unbound",
                    f"runtime {name!r} has no live immutable owner launch",
                )
            if action == "status":
                assert owned is not None  # narrowed by the fail-closed check above
                lib.reconcile_automation_pause(owned, paused)
                result = lib.status_snapshot(name, run_gc=False)
            elif action == "send":
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                result = lib.send_message_to_agent(
                    name, get_str(request, "text", "send"),
                    model=_optional_text(request, "model"),
                )
            elif action == "read":
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                result = lib.read_agent_output(
                    name,
                    mode=get_str(request, "mode", "read"),
                    since_turn=_optional_int(request, "since_turn"),
                    tail=_optional_int(request, "tail"),
                )
            elif action == "stop":
                result = lib.stop_owned_agent(
                    name, owner_token,
                    launch_fingerprint=owner_launch.fingerprint(),
                )
            elif action == "reset":
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                result = lib.reset_agent_context(name)
            elif action == "migrate":
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                mode = _optional_text(request, "mode")
                result = lib.migrate_agent(
                    name,
                    to_backend=get_str(request, "backend", "migrate"),
                    to_mode="tui" if mode == "interactive" else mode,
                )
            elif action == "repair":
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                result = lib.recreate_window(name)
            elif action in ("pause", "resume"):
                assert owned is not None
                lib.reconcile_automation_pause(owned, paused)
                result = {"paused": paused}
            else:
                raise ValueError(f"unsupported runtime operation: {action}")
            record = lib.read_registry().get(name)
            if record is not None:
                lib.verify_owner_launch(
                    name, owner_token, owner_launch,
                )
            return finish(result)


def _bounded_error(value: BaseException) -> str:
    """Format bounded exception arguments without constructing their full join."""
    marker = b"\n...[truncated by agentctl worker RPC]"
    budget = _MAX_RPC_ERROR_BYTES - len(marker)
    fragments: list[bytes] = []
    used = 0
    arguments = value.args or (type(value).__name__,)
    truncated = False
    for index, argument in enumerate(arguments):
        separator = b"" if index == 0 else b", "
        if used + len(separator) >= budget:
            truncated = True
            break
        text = argument if isinstance(argument, str) else f"<{type(argument).__name__}>"
        # Four UTF-8 bytes per code point bounds the intermediate allocation.
        # Drop an incomplete final code point rather than decoding it through
        # U+FFFD, whose three-byte encoding could exceed the byte contract.
        remaining = budget - used - len(separator)
        candidate = text[:remaining].encode(
            "utf-8", errors="replace",
        )
        encoded = candidate[:remaining].decode(
            "utf-8", errors="ignore",
        ).encode("utf-8")
        if len(candidate) > remaining or len(text) > remaining:
            truncated = True
        fragments.extend((separator, encoded))
        used += len(separator) + len(encoded)
        if truncated:
            break
    if len(fragments) == 0:
        truncated = True
    if len(arguments) > 1 and len(fragments) // 2 < len(arguments):
        truncated = True
    encoded = b"".join(fragments)
    if truncated:
        encoded = encoded[:budget] + marker
    return encoded.decode("utf-8")


def _write_response(document: dict[str, object]) -> bool:
    encoded = bytearray()
    oversized = False
    encoder = json.JSONEncoder(
        ensure_ascii=False, separators=(",", ":"), allow_nan=False,
    )
    for fragment in encoder.iterencode(document):
        block = fragment.encode("utf-8")
        if len(encoded) + len(block) + 1 > _MAX_RPC_RESPONSE_BYTES:
            oversized = True
            break
        encoded.extend(block)
    if oversized:
        encoded = (json.dumps({
            "schema": _RPC_SCHEMA,
            "action": document.get("action"),
            "owner_token": document.get("owner_token"),
            "ok": False,
            "error": {
                "code": "response_too_large",
                "message": "runtime response exceeds its byte limit",
            },
        }, separators=(",", ":")) + "\n").encode("utf-8")
    else:
        encoded.extend(b"\n")
    sys.stdout.buffer.write(encoded)
    sys.stdout.buffer.flush()
    return oversized


def _main() -> int:
    """Exchange exactly one JSON request and response over stdin/stdout."""
    action: str | None = None
    owner_token: str | None = None
    try:
        encoded = sys.stdin.buffer.read(_MAX_RPC_REQUEST_BYTES + 1)
        if len(encoded) > _MAX_RPC_REQUEST_BYTES:
            raise ValueError("runtime request exceeds its byte limit")
        request = as_mapping(
            agent._decode_json_bytes(encoded, "runtime request", "<stdin>"),
            "runtime request",
        )
        raw_action = request.get("action")
        action = raw_action if isinstance(raw_action, str) else None
        raw_owner_token = request.get("owner_token")
        owner_token = raw_owner_token if isinstance(raw_owner_token, str) else None
        result = dispatch(request)
    except (
        lib.AgentOperationError, AgentDeliveryError, ValueError, TypeError, OSError,
    ) as exc:
        _write_response({
            "schema": _RPC_SCHEMA,
            "action": action,
            "owner_token": owner_token,
            "ok": False,
            "error": {
                "code": getattr(exc, "code", "runtime_failure"),
                "message": _bounded_error(exc),
            },
        })
        return 1
    oversized = _write_response({
        "schema": _RPC_SCHEMA,
        "action": action,
        "owner_token": owner_token,
        "ok": True,
        "payload": result,
    })
    return 1 if oversized else 0


if __name__ == "__main__":
    raise SystemExit(_main())
