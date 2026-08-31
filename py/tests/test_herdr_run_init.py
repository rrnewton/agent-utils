"""`herdr-run init`: the generated `.herdr-run.yaml` is the configuration reference.

The guide points AT this file instead of restating the knobs, so the file has to actually carry
all of them, and adopting it has to change nothing until somebody edits it. Both are asserted here
rather than left to review.
"""

from __future__ import annotations

import json
import os

import pytest

from herdr_run.cli import main
from herdr_run.config import CONFIG_FILENAMES, KNOWN_KEYS, Config, load_config
from herdr_run.errors import EXIT_CONFIG, ConfigError
from herdr_run.init import config_template, write_config_template


def test_the_template_carries_every_configuration_key() -> None:
    """The promise that lets the guide point at the generated file instead of restating it.

    A new configuration key that nobody adds to the template turns the file into a partial answer,
    and a partial answer is worse than none because it looks complete.
    """
    template = config_template()
    for key in KNOWN_KEYS:
        assert f"\n{key}:" in template, f"the generated .herdr-run.yaml never sets {key}"


def test_the_written_template_parses_to_the_built_in_defaults(tmp_path: object) -> None:
    """Adopting the generated file must change nothing until somebody edits it."""
    pytest.importorskip("yaml")
    root = str(tmp_path)
    path = write_config_template(root, force=False)
    assert path == os.path.join(root, ".herdr-run.yaml")
    written = load_config(explicit_path=path, start_dir=root)
    expected = Config(source_path=written.source_path, project_root=written.project_root)
    assert written == expected


def test_the_template_names_the_allow_everything_mode_and_the_human_only_rule() -> None:
    """The two things the template has to say out loud."""
    template = config_template()
    assert 'allow: ["*"]' in template
    assert "ALLOW-EVERYTHING MODE" in template
    assert "a human-only knob" in template
    assert "DO NOT LET AN AGENT EDIT THIS SECTION" in template
    assert "worktrees/slotNN/" in template


@pytest.mark.parametrize("filename", CONFIG_FILENAMES)
def test_an_existing_configuration_is_not_clobbered_without_force(
    filename: str, tmp_path: object
) -> None:
    root = str(tmp_path)
    existing = os.path.join(root, filename)
    with open(existing, "w", encoding="utf-8") as handle:
        handle.write("workspace: mine\n")

    with pytest.raises(ConfigError) as excinfo:
        write_config_template(root, force=False)
    assert excinfo.value.exit_code == EXIT_CONFIG
    assert "pass --force" in str(excinfo.value)
    with open(existing, encoding="utf-8") as handle:
        assert handle.read() == "workspace: mine\n", f"{filename} was modified by a refused init"

    if filename != ".herdr-run.yaml":
        # Writing the .yaml would silently take precedence over the .yml that is already there;
        # two files and no warning is a puzzle, not a config.
        assert "takes precedence over it" in str(excinfo.value)
        assert not os.path.exists(os.path.join(root, ".herdr-run.yaml"))


def test_force_overwrites_an_existing_configuration(tmp_path: object) -> None:
    root = str(tmp_path)
    with open(os.path.join(root, ".herdr-run.yaml"), "w", encoding="utf-8") as handle:
        handle.write("workspace: mine\n")
    path = write_config_template(root, force=True)
    with open(path, encoding="utf-8") as handle:
        assert handle.read() == config_template()


def test_init_writes_and_then_refuses_through_the_cli(
    tmp_path: object,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = str(tmp_path)
    monkeypatch.chdir(root)

    assert main(["init"]) == 0
    captured = capsys.readouterr()
    assert captured.out.startswith(f"wrote {os.path.join(root, '.herdr-run.yaml')}\n")
    assert "HUMAN-ONLY knob" in captured.out
    assert captured.err == ""

    assert main(["init"]) == EXIT_CONFIG
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "--force" in captured.err

    assert main(["--json", "init", "--force"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document == {"created": True, "path": os.path.join(root, ".herdr-run.yaml")}


def test_init_writes_even_when_the_discovered_configuration_is_broken(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """The reason to reach for `init` is often that what is already there does not parse.

    Refusing to write a fresh template because the old one is broken would be exactly the wrong
    moment to be strict, so `init` runs before any configuration is loaded.
    """
    root = str(tmp_path)
    nested = os.path.join(root, "slot")
    os.makedirs(nested)
    with open(os.path.join(root, ".herdr-run.yaml"), "w", encoding="utf-8") as handle:
        handle.write("allow: [\n")
    monkeypatch.chdir(nested)

    assert main(["init"]) == 0
    assert capsys.readouterr().err == ""
    assert os.path.isfile(os.path.join(nested, ".herdr-run.yaml"))
