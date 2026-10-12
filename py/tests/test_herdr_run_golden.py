"""The shared golden corpora that pin the two herdr-run editions to one behaviour.

Every file under ``rs/herdr-run/testdata/`` is loaded here and by the Rust unit tests, so a rule
that changes in one edition fails the other edition's suite until both agree again:

- ``text_cases.json``: the quoting rule, numeric text, the exit-marker grammar, float text;
- ``yaml_cases.json``: the configuration YAML subset, accepted values and positioned refusals;
- ``readiness_cases.json``: prompt verdicts and prompt tails inferred from rc files;
- ``protocol_cases.json``: Herdr protocol calls, their argv, and their values or errors;
- ``record_cases.json``: reap scope, evidence folded from run records, session-cache loading.
"""

from __future__ import annotations

import json
import re
import subprocess
from collections.abc import Sequence
from pathlib import Path
from typing import cast

import pytest

from herdr_run import __version__
from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.errors import HerdrRunError
from herdr_run.readiness import assess_prompt, infer_prompt_tail
from herdr_run.reap import evidence_from_runs
from herdr_run.session import _load_cache
from herdr_run.sweep import _scope_of
from herdr_run.textfmt import number_text, parse_exit_marker, quote
from herdr_run.yamlcore import YamlError, core_load

_REPOSITORY = Path(__file__).resolve().parents[2]
_TESTDATA = _REPOSITORY / "rs" / "herdr-run" / "testdata"


def _load(name: str) -> dict[str, object]:
    return cast(dict[str, object], json.loads((_TESTDATA / name).read_text(encoding="utf-8")))


TEXT = _load("text_cases.json")
YAML = _load("yaml_cases.json")
READINESS = _load("readiness_cases.json")
PROTOCOL = _load("protocol_cases.json")
RECORDS = _load("record_cases.json")


def _rows(document: dict[str, object], key: str) -> list[list[object]]:
    return cast(list[list[object]], document[key])


# --- text rules ----------------------------------------------------------------------------------


@pytest.mark.parametrize("row", _rows(TEXT, "quote"), ids=repr)
def test_quote(row: list[object]) -> None:
    value, expected = row
    assert quote(cast(str, value)) == expected


@pytest.mark.parametrize("row", _rows(TEXT, "number_text"), ids=repr)
def test_number_text(row: list[object]) -> None:
    value, expected = row
    assert number_text(cast(float, value)) == expected


@pytest.mark.parametrize("row", _rows(TEXT, "exit_marker"), ids=repr)
def test_exit_marker(row: list[object]) -> None:
    payload, expected = row
    assert parse_exit_marker(bytes(cast(list[int], payload))) == expected


@pytest.mark.parametrize("row", _rows(TEXT, "float_repr"), ids=repr)
def test_float_text(row: list[object]) -> None:
    value, expected = row
    assert repr(cast(float, value)) == expected


# --- the configuration YAML subset ----------------------------------------------------------------


def _render(value: object) -> str:
    """The corpus's canonical rendering, written out the same way in ``yaml.rs``."""
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, str):
        return quote(value)
    if isinstance(value, list):
        return "[" + ", ".join(_render(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{" + ", ".join(f"{quote(key)}: {_render(item)}" for key, item in value.items()) + "}"
    raise TypeError(f"unexpected YAML value {value!r}")


@pytest.mark.parametrize(
    "case", cast(list[dict[str, str]], YAML["cases"]), ids=lambda case: case["name"]
)
def test_yaml_subset(case: dict[str, str]) -> None:
    if "error" in case:
        with pytest.raises(YamlError) as caught:
            core_load(case["text"])
        assert str(caught.value) == case["error"]
    else:
        assert _render(core_load(case["text"])) == case["value"]


# --- readiness -----------------------------------------------------------------------------------


@pytest.mark.parametrize("row", _rows(READINESS, "prompt"), ids=repr)
def test_prompt_verdict(row: list[object]) -> None:
    text, tail, verdict, reason, last_line = row
    signal = assess_prompt(cast(str, text), cast("str | None", tail))
    assert (signal.verdict, signal.reason, signal.last_line) == (verdict, reason, last_line)


@pytest.mark.parametrize("row", _rows(READINESS, "rc_files"), ids=repr)
def test_prompt_tail_from_rc_file(row: list[object], tmp_path: Path) -> None:
    payload, expected = row
    (tmp_path / ".bashrc").write_bytes(bytes(cast(list[int], payload)))
    assert infer_prompt_tail(Config(), home=str(tmp_path)) == expected


# --- the Herdr protocol --------------------------------------------------------------------------


def _protocol_value(client: HerdrClient, method: str, args: list[object]) -> object:
    text = [cast(str, item) for item in args if item is not None]
    if method == "workspace_id_for_label":
        return client.workspace_id_for_label(text[0])
    if method == "tab_id_for_label":
        return client.tab_id_for_label(text[0], text[1])
    if method == "panes":
        panes = client.panes(text[0] if text else None)
        return [[pane.pane_id, pane.tab_id, pane.workspace_id] for pane in panes]
    if method == "process_info":
        info = client.process_info(text[0])
        foreground = [list(entry) for entry in info.foreground]
        return [info.pane_id, info.shell_pid, info.foreground_pgid, foreground]
    if method == "create_workspace":
        return list(client.create_workspace(label=text[0], cwd=text[1]))
    if method == "create_tab":
        return client.create_tab(workspace_id=text[0], label=text[1], cwd=text[2])
    if method == "read":
        return client.read(text[0])
    if method == "run":
        client.run(text[0], text[1])
        return None
    if method == "pane_exists":
        return client.pane_exists(text[0])
    if method == "rename_tab":
        client.rename_tab(text[0], text[1])
        return None
    if method == "close_tab":
        client.close_tab(text[0])
        return None
    raise AssertionError(f"unknown protocol method {method}")


@pytest.mark.parametrize(
    "case",
    cast(list[dict[str, object]], PROTOCOL["cases"]),
    ids=lambda case: f"{case['method']}-{case['args']}",
)
def test_protocol(case: dict[str, object]) -> None:
    calls: list[list[str]] = []
    responses = list(cast(list[list[object]], case["responses"]))

    def runner(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        calls.append(list(command))
        assert responses, f"unexpected call {command}"
        status, stdout, stderr = responses.pop(0)
        return subprocess.CompletedProcess(list(command), cast(int, status), stdout, stderr)

    client = HerdrClient(herdr_bin=cast(str, PROTOCOL["herdr"]), run=runner)
    method = cast(str, case["method"])
    args = cast(list[object], case["args"])
    if "error" in case:
        with pytest.raises(HerdrRunError) as caught:
            _protocol_value(client, method, args)
        assert str(caught.value) == case["error"]
    else:
        assert _protocol_value(client, method, args) == case["ok"]
    assert calls == case["calls"]
    assert not responses


# --- run records and the session cache -----------------------------------------------------------


def _scope_config() -> Config:
    settings = cast(dict[str, str], RECORDS["scope_config"])
    return Config(workspace=settings["workspace"], tab_name=settings["tab_name"], project_root="/p")


@pytest.mark.parametrize("row", _rows(RECORDS, "scope"), ids=repr)
def test_reap_scope(row: list[object]) -> None:
    pane_id, records, expected = row
    got = _scope_of(cast(str, pane_id), cast(list[dict[str, object]], records), _scope_config())
    assert list(got) == expected


@pytest.mark.parametrize("row", _rows(RECORDS, "evidence"), ids=repr)
def test_recorded_evidence(row: list[object]) -> None:
    pane_id, records, flags, identity = row
    got_flags, got_identity = evidence_from_runs(
        cast(str, pane_id), cast(list[dict[str, object]], records)
    )
    assert list(got_flags) == flags
    shown = (
        None
        if got_identity is None
        else [got_identity.pid, got_identity.boot_id, got_identity.start_ticks]
    )
    assert shown == identity


@pytest.mark.parametrize("row", _rows(RECORDS, "cache"), ids=repr)
def test_session_cache_entry(row: list[object], tmp_path: Path) -> None:
    document, expected = row
    path = tmp_path / "session-cache.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    loaded = _load_cache(str(path), "agent-cmds\u0000a")
    shown = (
        None
        if loaded is None
        else [
            loaded.workspace_id,
            loaded.tab_id,
            loaded.pane_id,
            loaded.workspace_label,
            loaded.tab_label,
        ]
    )
    assert shown == expected


# --- one version for both editions ---------------------------------------------------------------


def _manifest_version(path: Path, section: str) -> str:
    """The ``version`` line of one TOML section, read without a TOML dependency."""
    current = ""
    for line in path.read_text(encoding="utf-8").splitlines():
        header = re.fullmatch(r"\[([^\]]+)\]", line.strip())
        if header:
            current = header.group(1)
            continue
        found = re.fullmatch(r'version\s*=\s*"([^"]+)"', line.strip())
        if found and current == section:
            return found.group(1)
    raise AssertionError(f"{path} has no version in [{section}]")


def test_python_and_rust_packages_share_one_version() -> None:
    python_project = _manifest_version(_REPOSITORY / "py" / "herdr_run" / "pyproject.toml", "project")
    rust_package = _manifest_version(_REPOSITORY / "rs" / "herdr-run" / "Cargo.toml", "package")
    assert __version__ == python_project == rust_package
