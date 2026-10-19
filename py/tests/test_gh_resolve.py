"""The shared GitHub CLI resolver: explicit override, then gh-paced, then gh on PATH."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import sys

import pytest

from wrkviz.gh_resolve import GhResolveError, resolve_gh


def _program(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!{sys.executable}\n{body}")
    path.chmod(0o755)
    return path


def _real_gh(directory: Path, login: str = "example-user") -> Path:
    """A stand-in gh whose `config get user -h github.com` prints a login."""

    return _program(
        directory / "gh",
        "import sys\n"
        "if sys.argv[1:] == ['config', 'get', 'user', '-h', 'github.com']:\n"
        f"    print({login!r})\n"
        "    sys.exit(0)\n"
        "sys.exit(3)\n",
    )


def _env(*directories: Path, **extra: str) -> dict[str, str]:
    return {"PATH": os.pathsep.join(str(d) for d in directories), **extra}


def test_plain_gh_on_path_when_gh_paced_is_absent(tmp_path: Path) -> None:
    _real_gh(tmp_path / "bin")

    assert resolve_gh(_env(tmp_path / "bin"), use_source_root=False) == ["gh"]


def test_an_explicit_override_wins(tmp_path: Path) -> None:
    _program(tmp_path / "bin" / "gh-paced", "")

    command = resolve_gh(
        _env(tmp_path / "bin", AGENT_UTILS_GH="/opt/tools/my gh-wrapper --flag"),
        use_source_root=False,
    )

    assert command == ["/opt/tools/my", "gh-wrapper", "--flag"]


def test_installed_gh_paced_wraps_the_real_gh_with_its_active_login(tmp_path: Path) -> None:
    paced = _program(tmp_path / "paced" / "gh-paced", "")
    # A gh-paced wrapper script earlier on PATH must not be wrapped again.
    _program(tmp_path / "wrapper" / "gh", "# exec gh-paced --account someone -- \"$@\"\n")
    real = _real_gh(tmp_path / "real")

    command = resolve_gh(
        _env(tmp_path / "paced", tmp_path / "wrapper", tmp_path / "real"),
        use_source_root=False,
    )

    assert command == [
        str(paced), "--account", "example-user", "--real-gh", str(real.resolve()), "--",
    ]


def test_account_and_real_gh_variables_take_precedence(tmp_path: Path) -> None:
    paced = _program(tmp_path / "paced" / "gh-paced", "")
    _real_gh(tmp_path / "real", login="ignored-login")

    command = resolve_gh(
        _env(
            tmp_path / "paced",
            tmp_path / "real",
            GH_PACED_ACCOUNT="example-bot",
            GH_PACED_REAL_GH="/usr/bin/gh",
        ),
        use_source_root=False,
    )

    assert command == [str(paced), "--account", "example-bot", "--real-gh", "/usr/bin/gh", "--"]


def test_a_build_in_the_checkout_comes_before_an_installed_gh_paced(tmp_path: Path) -> None:
    checkout = tmp_path / "agent-utils"
    built = _program(checkout / "rs" / "target" / "x86_64-unknown-linux-gnu" / "release" / "gh-paced", "")
    _program(tmp_path / "paced" / "gh-paced", "")
    real = _real_gh(tmp_path / "real")

    command = resolve_gh(_env(tmp_path / "paced", tmp_path / "real"), root=checkout)

    assert command == [str(built), "--account", "example-user", "--real-gh", str(real.resolve()), "--"]


def test_gh_paced_without_a_login_refuses_rather_than_running_unpaced(tmp_path: Path) -> None:
    _program(tmp_path / "paced" / "gh-paced", "")
    _real_gh(tmp_path / "real", login="")

    with pytest.raises(GhResolveError, match="no GitHub login to pace"):
        resolve_gh(_env(tmp_path / "paced", tmp_path / "real"), use_source_root=False)


def test_no_home_directory_path_is_assumed(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # A gh-paced and gh under HOME/bin are invisible unless that directory is on PATH.
    home = tmp_path / "home"
    _program(home / "bin" / "gh-paced", "")
    _real_gh(home / "bin")
    monkeypatch.setenv("HOME", str(home))

    assert resolve_gh(_env(tmp_path / "empty"), use_source_root=False) == ["gh"]


def test_the_resolved_command_runs(tmp_path: Path) -> None:
    # End to end: a stand-in gh-paced receives the account, the real gh and gh's arguments.
    record = tmp_path / "argv"
    _program(
        tmp_path / "paced" / "gh-paced",
        f"import sys\nopen({str(record)!r}, 'w').write(' '.join(sys.argv[1:]))\n",
    )
    real = _real_gh(tmp_path / "real")
    command = resolve_gh(_env(tmp_path / "paced", tmp_path / "real"), use_source_root=False)

    import subprocess

    subprocess.run([*command, "api", "user"], check=True)

    assert record.read_text() == f"--account example-user --real-gh {real.resolve()} -- api user"
    assert shutil.which("gh-paced", path=str(tmp_path / "paced"))


def test_dagrun_github_artifacts_backend_runs_the_resolved_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from dagrun.sync import GitHubArtifactsBackend

    record = tmp_path / "argv"
    _program(
        tmp_path / "paced" / "gh-paced",
        f"import sys\nopen({str(record)!r}, 'w').write(' '.join(sys.argv[1:]))\nprint('[]')\n",
    )
    real = _real_gh(tmp_path / "real")
    monkeypatch.setenv("PATH", os.pathsep.join([str(tmp_path / "paced"), str(tmp_path / "real")]))
    for name in ("AGENT_UTILS_GH", "GH_PACED_ACCOUNT", "GH_PACED_REAL_GH"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr("dagrun.gh_resolve.source_root", lambda: None)

    GitHubArtifactsBackend("summary", "owner/repo", staging_dir=tmp_path)._gh(["api", "x"])

    assert record.read_text() == f"--account example-user --real-gh {real.resolve()} -- api x"


def test_planner_uses_gh_cmd_when_given_and_the_resolver_otherwise(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from pr_landing_planner.githubhost import GitHubHost

    _real_gh(tmp_path / "real")
    monkeypatch.setenv("PATH", str(tmp_path / "real"))
    monkeypatch.delenv("AGENT_UTILS_GH", raising=False)
    monkeypatch.setattr("pr_landing_planner.gh_resolve.source_root", lambda: None)

    assert GitHubHost(gh_cmd="/opt/gh-wrapper")._gh() == ["/opt/gh-wrapper"]
    assert GitHubHost()._gh() == ["gh"]
    monkeypatch.setenv("AGENT_UTILS_GH", "/opt/other-gh")
    assert GitHubHost()._gh() == ["/opt/other-gh"]
