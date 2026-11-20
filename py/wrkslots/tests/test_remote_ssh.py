"""Per-remote SSH commands (#1940).

The declared settings reach a remote only as GIT_SSH_COMMAND on its
network operations. Paths are quoted with shlex; values that could
break out of the quoting refuse at config load. A shim SSH command
proves the environment reaches Git's transport. Temporary fixtures
only.
"""

from __future__ import annotations

import json
import os
import shlex
import stat
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _fixture, _git


def test_command_round_trips_through_shlex() -> None:
    parsed = wrkslots_cli._parse_remote_ssh_commands(
        {
            "origin": {
                "identity_file": "/keys/ghe-key",
                "identity_agent": "none",
                "proxy_command": "ssh -W %h:%p jump.example.invalid",
            }
        },
        "configuration.remote_ssh_commands",
    )
    config = object.__new__(wrkslots_cli.Config)
    object.__setattr__(config, "remote_ssh_commands", parsed)
    command = wrkslots_cli._remote_ssh_command(config, "origin")
    assert command is not None
    assert shlex.split(command) == [
        "ssh",
        "-i",
        "/keys/ghe-key",
        "-o",
        "IdentityAgent=none",
        "-o",
        "ProxyCommand=ssh -W %h:%p jump.example.invalid",
    ]
    assert wrkslots_cli._remote_ssh_command(config, "other") is None


@pytest.mark.parametrize(
    "field,value",
    [
        ("identity_file", "/keys/ghe;rm -rf /"),
        ("identity_file", "/keys/ghe key"),
        ("identity_file", "relative/key"),
        ("identity_file", "/keys/$(id)"),
        ("identity_agent", "/tmp/agent;id"),
        ("proxy_command", "ssh jump\nrm -rf /"),
        ("proxy_command", "  "),
    ],
)
def test_injection_values_refuse(field: str, value: str) -> None:
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._parse_remote_ssh_commands(
            {"origin": {field: value}}, "configuration.remote_ssh_commands"
        )


def test_empty_entry_refuses() -> None:
    with pytest.raises(wrkslots_cli.StateError):
        wrkslots_cli._parse_remote_ssh_commands(
            {"origin": {}}, "configuration.remote_ssh_commands"
        )


def test_config_loads_and_doctor_rows_hide_contents(tmp_path: Path) -> None:
    root = _ssh_fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), "node-a")
    assert config.remote_ssh_commands[0][0] == "origin"
    rows = wrkslots_cli._remote_ssh_doctor_rows(config)
    assert rows == [
        {
            "remote": "origin",
            "identity_file": "/keys/ghe-key",
            "identity_agent": "none",
            "proxy_command": "set",
        }
    ]
    assert "jump.example" not in str(rows)


def _ssh_fixture(tmp_path: Path) -> Path:
    root = _fixture(tmp_path / "case")
    config_path = root / ".wrkslots.yml"
    config_path.write_text(
        config_path.read_text()
        + "remote_ssh_commands:\n"
        + "  origin:\n"
        + "    identity_file: /keys/ghe-key\n"
        + "    identity_agent: none\n"
        + "    proxy_command: ssh -W %h:%p jump.example.invalid\n"
        + "ignorable_keys:\n"
        + "- remote_ssh_commands\n"
    )
    return root


def test_rust_validate_config_and_doctor_match(tmp_path: Path) -> None:
    import subprocess

    from wrkslots.tests.rust_binary import rust_wrkslotsd

    root = _ssh_fixture(tmp_path / "rust")
    env = dict(os.environ)
    validated = subprocess.run(
        [str(rust_wrkslotsd()), "validate-config", "--root", str(root)],
        capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert validated.returncode == 0, validated.stderr
    payload = json.loads(validated.stdout)
    assert payload["remote_ssh_commands"] == [
        {
            "remote": "origin",
            "identity_file": "/keys/ghe-key",
            "identity_agent": "none",
            "proxy_command": "ssh -W %h:%p jump.example.invalid",
        }
    ]
    doctor = subprocess.run(
        [
            str(rust_wrkslotsd()), "doctor-text", "--machine", "node-a",
            "--root", str(root), "--format", "json",
        ],
        capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert doctor.returncode == 0, doctor.stderr
    rows = json.loads(doctor.stdout)["remote_ssh_commands"]
    assert rows[0]["identity_file"] == "/keys/ghe-key"
    assert rows[0]["proxy_command"] == "set"
    assert "jump.example" not in doctor.stdout


def test_fetch_remote_honours_the_ssh_command(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    upstream = tmp_path / "case" / "upstream-repo"
    url = f"ssh://git@localhost{upstream}"
    _git(upstream, "remote", "set-url", "origin", url)
    shim = tmp_path / "fake-ssh"
    shim.write_text(
        "#!/bin/sh\n"
        "while [ $# -gt 1 ]; do shift; done\n"
        'exec sh -c "$1"\n'
    )
    shim.chmod(shim.stat().st_mode | stat.S_IEXEC)
    vcs = wrkslots_cli._GitVcs()
    authority = vcs.remote_authority(upstream, "origin")
    # A command that always fails refuses the fetch: the env reaches Git.
    with pytest.raises(wrkslots_cli.Refusal):
        vcs.fetch_remote(
            upstream, "origin", "refs/remotes/origin/main", authority,
            ssh_command="/bin/false",
        )
    # The shim answers as SSH against the local repository.
    vcs.fetch_remote(
        upstream, "origin", "refs/remotes/origin/main", authority,
        ssh_command=str(shim),
    )
    head = _git(upstream, "rev-parse", "refs/remotes/origin/main")
    assert head == _git(upstream, "rev-parse", "main")
