"""One judgement-only one-shot attempt per harness, with its own argv.

Each builder uses the harness's strict non-interactive form and
extracts plain final text from its structured output. Every attempt
is tool-free or read-confined by construction: Muse's isolation
preset, Claude Code's empty tool list, and Codex with its shell
and MCP surfaces disabled inside a read-only sandbox. Effects belong
to the caller, never to an agent's own tools.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

from run_agent_prompt.process import TIMED_OUT_EXIT, BackendProcesses

#: Reasoning-effort levels each harness accepts (help-verified).
EFFORT_LEVELS: dict[str, tuple[str, ...]] = {
    "muse": ("none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"),
    "claude": ("low", "medium", "high", "xhigh", "max"),
    "codex": ("minimal", "low", "medium", "high", "xhigh", "ultra"),
}

#: ``muse exec`` flags keeping one call a pure prompt-to-answer
#: transformation.
MUSE_ISOLATION_FLAGS: tuple[str, ...] = (
    "--preset",
    "empty",
    "--disable-shell",
    "--disable-write",
    "--disable-web-tools",
    "--disable-reminders",
    "--disable-request-user-input",
    "--no-session-log",
    "--no-foreign-personal-context",
    "--disable-muse-llm-rules",
)

_ENV_KEYS = (
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "TZ",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "http_proxy",
    "https_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "no_proxy",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CONFIG_DIR",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_ORG_ID",
    "OPENAI_PROJECT_ID",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    "AZURE_OPENAI_API_VERSION",
    "CODEX_HOME",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_CLIENT_CERT",
    "CODEX_CLIENT_KEY",
    "MUSE_API_KEY",
    "MUSE_CLIENT_CERT",
    "MUSE_CLIENT_KEY",
    "MUSE_CUSTOM_HEADERS",
    "THRIFT_TLS_CL_CERT_PATH",
    "THRIFT_TLS_CL_KEY_PATH",
    "X2P_AGENT_PROXY_ADDRESS",
    "META_AI_GATEWAY_URL",
    "META_CLAUDE_AI_GATEWAY",
    "META_CLAUDE_USE_AI_GATEWAY",
    "META_CODEX_AI_GATEWAY",
    "META_CODEX_USE_AZURE_DIRECT",
    "META_CODEX_USE_OPENAI_DIRECT",
    "META_MUSE_OPENAI_ROUTE",
)


def child_env(source: Mapping[str, str] | None = None) -> dict[str, str]:
    """The minimal allowlisted environment a harness attempt inherits.

    Only the explicit names above cross into the child. In particular,
    no `CLAUDE_CODE_*` session or IDE variables are inherited. Values
    are never logged by callers.
    """

    source = os.environ if source is None else source
    return {key: source[key] for key in _ENV_KEYS if key in source}


@dataclass(frozen=True)
class AttemptResult:
    """How one harness attempt ended; `answer` only when answered."""

    harness: str
    status: str  # answered | failed | timeout | empty | spawn-error | invalid-effort
    exit_code: int | None
    duration_s: float
    answer: str | None = None
    model: str | None = None


def _find_model(value: object) -> str | None:
    """First plausible model string a harness event stream names."""

    if isinstance(value, dict):
        for key in ("model", "model_id"):
            found = value.get(key)
            if isinstance(found, str) and found:
                return found
        models = value.get("models")
        if isinstance(models, list):
            for entry in models:
                if isinstance(entry, dict):
                    for key in ("model", "model_id", "id", "name"):
                        found = entry.get(key)
                        if isinstance(found, str) and found:
                            return found
        for item in value.values():
            if (found := _find_model(item)) is not None:
                return found
    elif isinstance(value, list):
        for item in value:
            if (found := _find_model(item)) is not None:
                return found
    return None


def _timed(
    processes: BackendProcesses,
    args: Sequence[str],
    *,
    input_text: str,
    cwd: Path,
    env: Mapping[str, str],
    timeout_seconds: float,
) -> tuple[subprocess.CompletedProcess[str] | None, float, OSError | None]:
    started = time.monotonic()
    try:
        completed = processes.run(
            args,
            input_text=input_text,
            cwd=cwd,
            env=env,
            timeout_seconds=timeout_seconds,
        )
    except OSError as error:
        return None, time.monotonic() - started, error
    return completed, time.monotonic() - started, None


def _verdict(
    harness: str,
    completed: subprocess.CompletedProcess[str] | None,
    spawn_error: OSError | None,
    duration_s: float,
    answer: str | None,
    model: str | None,
) -> AttemptResult:
    if spawn_error is not None:
        return AttemptResult(harness, "spawn-error", None, duration_s)
    assert completed is not None
    returncode = completed.returncode
    if returncode == 0 and answer is not None and answer.strip():
        return AttemptResult(harness, "answered", returncode, duration_s, answer, model)
    if returncode == TIMED_OUT_EXIT:
        status = "timeout"
    elif returncode != 0:
        status = "failed"
    else:
        status = "empty"
    return AttemptResult(harness, status, returncode, duration_s, model=model)


def attempt_muse(
    processes: BackendProcesses,
    command: Sequence[str],
    *,
    prompt: str,
    prompt_path: Path,
    workspace: Path,
    model: str | None,
    effort: str,
    timeout_seconds: float,
    env: Mapping[str, str],
) -> AttemptResult:
    """One `muse exec --json` call; the run_terminal text is the answer."""

    prompt_path.write_text(prompt, encoding="utf-8")
    args = [
        *command,
        "exec",
        "--json",
        "--prompt-file",
        str(prompt_path),
        "--reasoning-effort",
        effort,
        "--workspace",
        str(workspace),
        *MUSE_ISOLATION_FLAGS,
    ]
    if model:
        args[args.index("--reasoning-effort"):args.index("--reasoning-effort")] = [
            "--model",
            model,
        ]
    completed, duration_s, spawn_error = _timed(
        processes,
        args,
        input_text="",
        cwd=workspace,
        env=env,
        timeout_seconds=timeout_seconds,
    )
    if spawn_error is not None:
        return _verdict("muse", completed, spawn_error, duration_s, None, model)
    terminal: str | None = None
    text: str | None = None
    found_model = model
    for line in completed.stdout.splitlines():  # type: ignore[union-attr]
        try:
            event: object = json.loads(line)
        except json.JSONDecodeError:
            continue
        if found_model is None:
            found_model = _find_model(event)
        if not isinstance(event, dict):
            continue
        payload = event.get("payload")
        if isinstance(payload, dict) and payload.get("kind") == "run_terminal":
            terminal = payload.get("terminal") if isinstance(payload.get("terminal"), str) else None
            text = payload.get("text") if isinstance(payload.get("text"), str) else None
    answer = text if terminal == "completed" else None
    return _verdict("muse", completed, None, duration_s, answer, found_model)


def attempt_claude(
    processes: BackendProcesses,
    command: Sequence[str],
    *,
    prompt: str,
    workspace: Path,
    model: str | None,
    effort: str,
    timeout_seconds: float,
    env: Mapping[str, str],
) -> AttemptResult:
    """One `claude --print` call with no tools; the JSON result is the answer."""

    args = [
        *command,
        "--print",
        "--output-format",
        "json",
        "--safe-mode",
        "--restricted",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
        "--tools",
        "",
        "--effort",
        effort,
    ]
    if model:
        args += ["--model", model]
    completed, duration_s, spawn_error = _timed(
        processes,
        args,
        input_text=prompt,
        cwd=workspace,
        env=env,
        timeout_seconds=timeout_seconds,
    )
    if spawn_error is not None:
        return _verdict("claude", completed, spawn_error, duration_s, None, model)
    answer: str | None = None
    found_model = model
    try:
        root: object = json.loads(completed.stdout)  # type: ignore[union-attr]
    except json.JSONDecodeError:
        root = None
    if isinstance(root, dict):
        found_model = found_model or _find_model(root)
        if (
            root.get("type") == "result"
            and root.get("subtype") == "success"
            and root.get("is_error") is False
            and isinstance(root.get("result"), str)
        ):
            answer = root["result"]
    return _verdict("claude", completed, None, duration_s, answer, found_model)


def attempt_codex(
    processes: BackendProcesses,
    command: Sequence[str],
    *,
    prompt: str,
    workspace: Path,
    last_message_path: Path,
    model: str | None,
    effort: str,
    timeout_seconds: float,
    env: Mapping[str, str],
) -> AttemptResult:
    """One `codex exec` call; the last-message file is the answer."""

    last_message = last_message_path
    args = [
        *command,
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--disable",
        "shell_tool",
        "--disable",
        "apps",
        "--disable",
        "browser_use",
        "--disable",
        "browser_use_external",
        "--disable",
        "browser_use_full_cdp_access",
        "--disable",
        "in_app_browser",
        "--disable",
        "computer_use",
        "--disable",
        "image_generation",
        "--disable",
        "plugins",
        "--disable",
        "hooks",
        "--disable",
        "multi_agent",
        "--disable",
        "code_mode_host",
        "--disable",
        "code_mode_host_grpc",
        "--cd",
        str(workspace),
        "--json",
        "--output-last-message",
        str(last_message),
        "-c",
        "mcp_servers={}",
        "-c",
        "mcp_servers.meta_core.enabled=false",
        "-c",
        'web_search="disabled"',
        "-c",
        f'model_reasoning_effort="{effort}"',
    ]
    if model:
        args += ["--model", model]
    args.append("-")
    completed, duration_s, spawn_error = _timed(
        processes,
        args,
        input_text=prompt,
        cwd=workspace,
        env=env,
        timeout_seconds=timeout_seconds,
    )
    if spawn_error is not None:
        return _verdict("codex", completed, spawn_error, duration_s, None, model)
    found_model = model
    for line in completed.stdout.splitlines():  # type: ignore[union-attr]
        try:
            event: object = json.loads(line)
        except json.JSONDecodeError:
            continue
        if found_model is None:
            found_model = _find_model(event)
    answer: str | None = None
    if completed.returncode == 0:  # type: ignore[union-attr]
        try:
            answer = last_message.read_text(encoding="utf-8")
        except OSError:
            answer = None
    return _verdict("codex", completed, None, duration_s, answer, found_model)
