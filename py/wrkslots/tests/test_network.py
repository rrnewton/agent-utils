"""Exercise slot networking through a command-scoped host wrapper."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests import test_lifecycle as lifecycle


def install_proxy_wrapper(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    tools = tmp_path / "network-tools"
    tools.mkdir()
    log = tmp_path / "proxy-calls.jsonl"
    wrapper = tools / "with-proxy"
    wrapper.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            """\
            import json
            import os
            import sys
            from pathlib import Path

            command = sys.argv[1:]
            with Path(os.environ['WRKSLOTS_TEST_PROXY_LOG']).open('a') as output:
                output.write(json.dumps(command) + '\\n')
            if command[0] == 'git':
                assert 'GIT_DIR' not in os.environ
                assert 'GIT_CONFIG_COUNT' not in os.environ
                assert os.environ['GIT_CONFIG_GLOBAL'] == '/dev/null'
                assert os.environ['GIT_CONFIG_SYSTEM'] == '/dev/null'
                assert os.environ['GIT_NO_REPLACE_OBJECTS'] == '1'
            if os.environ.get('WRKSLOTS_TEST_PROXY_FAIL') == command[0]:
                print('test proxy refused the command', file=sys.stderr)
                raise SystemExit(17)
            os.environ['WRKSLOTS_TEST_PROXY_MARKER'] = 'via-with-proxy'
            os.execvp(command[0], command)
            """
        ),
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tools}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("WRKSLOTS_TEST_PROXY_LOG", str(log))
    return log


def proxy_commands(log: Path) -> list[list[str]]:
    commands: list[list[str]] = []
    for line in log.read_text(encoding="utf-8").splitlines():
        value: object = json.loads(line)
        assert isinstance(value, list)
        command: list[str] = []
        for argument in value:
            assert isinstance(argument, str)
            command.append(argument)
        commands.append(command)
    return commands


def install_credential_helper(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    helper = tmp_path / "real-gh"
    helper.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            """\
            import sys

            assert sys.argv[1:] == ['auth', 'git-credential', 'get']
            request = sys.stdin.read()
            assert 'protocol=https' in request
            assert 'host=github.com' in request
            print('username=test-user')
            print('password=test-password')
            """
        ),
        encoding="utf-8",
    )
    helper.chmod(0o755)
    monkeypatch.setenv("FLEET_REAL_GH", str(helper))
    return helper.resolve()


def test_network_git_commands_use_wrapper_and_preserve_git_isolation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _project, repository, remote = lifecycle.make_project(tmp_path)
    expected = lifecycle.git(repository, "rev-parse", "HEAD").stdout.strip()
    lifecycle.git(repository, "update-ref", "-d", "refs/remotes/origin/main")
    log = install_proxy_wrapper(tmp_path, monkeypatch)
    helper = install_credential_helper(tmp_path, monkeypatch)
    monkeypatch.setenv("GIT_DIR", str(tmp_path / "unrelated-git-directory"))
    monkeypatch.setenv("GIT_CONFIG_COUNT", "1")
    monkeypatch.setenv("GIT_CONFIG_KEY_0", "core.useReplaceRefs")
    monkeypatch.setenv("GIT_CONFIG_VALUE_0", "true")

    vcs = wrkslots._GitVcs()
    vcs.fetch_remote(repository, "origin", "refs/remotes/origin/main")
    assert vcs.verify_ref(repository, "refs/remotes/origin/main", "fetched ref") == expected
    ref = "refs/heads/salvage/testhost/slot01/network-test"
    vcs.push_salvage(repository, "origin", expected, ref)
    assert vcs.verify_ref(remote, ref, "published ref") == expected
    assert vcs.head(repository) == expected
    assert vcs.status(repository) == ""

    commands = proxy_commands(log)
    assert [command[command.index("-C") + 2] for command in commands] == [
        "fetch", "ls-remote", "push", "ls-remote"
    ]
    assert all(command[:4] == [
        "git", "--no-replace-objects", "-c", "core.useReplaceRefs=false"
    ] for command in commands)
    expected_helper = f"credential.helper=!{helper} auth git-credential"
    assert all(
        command[4:8]
        == ["-c", "credential.helper=", "-c", expected_helper]
        for command in commands
    )


def test_github_credential_helper_works_with_global_git_config_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    helper = install_credential_helper(tmp_path, monkeypatch)
    arguments = wrkslots._github_credential_helper_args(os.environ)

    completed = subprocess.run(
        ["git", *arguments, "credential", "fill"],
        input="protocol=https\nhost=github.com\n\n",
        text=True,
        capture_output=True,
        check=False,
        env={**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null"},
    )

    assert completed.returncode == 0, completed.stderr
    assert completed.stdout.splitlines() == [
        "protocol=https",
        "host=github.com",
        "username=test-user",
        "password=test-password",
    ]
    assert arguments == (
        "-c",
        "credential.helper=",
        "-c",
        f"credential.helper=!{helper} auth git-credential",
    )


def test_invalid_configured_github_cli_refuses_before_network(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    missing = tmp_path / "missing-gh"
    monkeypatch.setenv("FLEET_REAL_GH", str(missing))

    with pytest.raises(wrkslots.Refusal, match="configured GitHub CLI is unavailable"):
        wrkslots._github_credential_helper_args(os.environ)


def test_create_hooks_and_recursive_submodules_inherit_wrapper_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = lifecycle.make_project(tmp_path)
    component_head, leaf_head = lifecycle.add_recursive_submodules(
        tmp_path, project, repository
    )
    hook = lifecycle.python_hook(
        "import os, subprocess, sys; from pathlib import Path; "
        "assert os.environ['WRKSLOTS_TEST_PROXY_MARKER'] == 'via-with-proxy'; "
        "subprocess.run([sys.executable, '-c', "
        "\"import os; assert os.environ['WRKSLOTS_TEST_PROXY_MARKER'] == 'via-with-proxy'\"], "
        "check=True); "
        "assert Path('component/leaf/leaf.txt').read_text() == 'leaf sentinel\\n'; "
        "Path('hook-completed').write_text('complete')"
    )
    lifecycle.update_configuration(
        project,
        post_provision_hooks=[
            "git -c protocol.file.allow=always submodule update --init --recursive",
            hook,
        ],
    )
    log = install_proxy_wrapper(tmp_path, monkeypatch)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PATH", f"network-tools{os.pathsep}{os.environ['PATH']}")
    secret = "proxy-secret-must-not-appear"
    monkeypatch.setenv("WRKSLOTS_TEST_PRIVATE_VALUE", secret)

    made = lifecycle.create(project)

    assert made.returncode == 0, made.stderr
    target = lifecycle.checkout(project)
    assert (target / "hook-completed").read_text(encoding="utf-8") == "complete"
    assert lifecycle.git(target / "component", "rev-parse", "HEAD").stdout.strip() == component_head
    assert lifecycle.git(target / "component" / "leaf", "rev-parse", "HEAD").stdout.strip() == leaf_head
    assert len(lifecycle.active_slots(project)) == 1
    commands = proxy_commands(log)
    assert [command[0] for command in commands] == ["git", "/bin/sh", "/bin/sh"]
    assert commands[0][commands[0].index("-C") + 2] == "fetch"
    assert secret not in made.stdout + made.stderr + log.read_text(encoding="utf-8")


def test_failed_fetch_wrapper_never_retries_direct_or_registers_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = lifecycle.make_project(tmp_path)
    log = install_proxy_wrapper(tmp_path, monkeypatch)
    monkeypatch.setenv("WRKSLOTS_TEST_PROXY_FAIL", "git")

    refused = lifecycle.create(project)

    assert refused.returncode == 3
    assert "test proxy refused the command" in refused.stderr
    assert lifecycle.active_slots(project) == []
    assert not lifecycle.checkout(project).exists()
    assert not lifecycle.create_journal_path(project).exists()
    commands = proxy_commands(log)
    assert len(commands) == 1
    assert commands[0][commands[0].index("-C") + 2] == "fetch"


def test_failed_hook_wrapper_preserves_journal_and_recovery_uses_wrapper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    hook = lifecycle.python_hook(
        "import os; from pathlib import Path; "
        "assert os.environ['WRKSLOTS_TEST_PROXY_MARKER'] == 'via-with-proxy'; "
        "Path('hook-completed').write_text('complete')"
    )
    project, _repository, _remote = lifecycle.make_project(
        tmp_path, post_provision_hooks=(hook,)
    )
    log = install_proxy_wrapper(tmp_path, monkeypatch)
    monkeypatch.setenv("WRKSLOTS_TEST_PROXY_FAIL", "/bin/sh")

    refused = lifecycle.create(project)

    assert refused.returncode == 3
    assert "test proxy refused the command" in refused.stderr
    assert lifecycle.active_slots(project) == []
    target = lifecycle.checkout(project)
    assert target.is_dir()
    assert not (target / "hook-completed").exists()
    assert lifecycle.create_journal_path(project).is_file()
    assert [command[0] for command in proxy_commands(log)] == ["git", "/bin/sh"]

    monkeypatch.delenv("WRKSLOTS_TEST_PROXY_FAIL")
    recovered = lifecycle.command(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )

    assert recovered.returncode == 0, recovered.stderr
    assert (target / "hook-completed").read_text(encoding="utf-8") == "complete"
    assert len(lifecycle.active_slots(project)) == 1
    assert not lifecycle.create_journal_path(project).exists()
    assert [command[0] for command in proxy_commands(log)] == [
        "git", "/bin/sh", "/bin/sh"
    ]


def test_create_and_remote_round_trip_work_without_installed_wrapper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    hook = lifecycle.python_hook(
        "import os; from pathlib import Path; "
        "assert os.environ['WRKSLOTS_TEST_NETWORK_ENV'] == 'inherited'; "
        "Path('hook-completed').write_text('complete')"
    )
    project, repository, remote = lifecycle.make_project(
        tmp_path, post_provision_hooks=(hook,)
    )
    tools = tmp_path / "tools-without-proxy"
    tools.mkdir()
    for name in ("git", "python3"):
        executable = shutil.which(name)
        assert executable is not None
        (tools / name).symlink_to(executable)
    monkeypatch.setenv("PATH", str(tools))
    monkeypatch.setenv("WRKSLOTS_TEST_NETWORK_ENV", "inherited")

    made = lifecycle.create(project)

    assert made.returncode == 0, made.stderr
    assert (lifecycle.checkout(project) / "hook-completed").read_text(encoding="utf-8") == "complete"
    assert len(lifecycle.active_slots(project)) == 1
    vcs = wrkslots._GitVcs()
    expected = vcs.head(repository)
    ref = "refs/heads/salvage/testhost/slot01/network-test"
    vcs.push_salvage(repository, "origin", expected, ref)
    assert vcs.verify_ref(remote, ref, "published ref") == expected
