"""Per-project configuration tests: defaults, discovery, and validation.

The defaults matter as much as the parsing: a project with no config file must still get the
intended conservative policy, because that is the state most consumers will run in.
"""

from __future__ import annotations

import os

import pytest

from herdr_run.config import (
    MAX_RETENTION_DAYS,
    MAX_TIMEOUT_SECONDS,
    Config,
    find_config_file,
    load_config,
    parse_config,
)
from herdr_run.errors import ConfigError


# --- defaults ------------------------------------------------------------------------------------


def test_defaults_match_the_intended_policy() -> None:
    config = Config()
    assert config.workspace == "agent-cmds"
    assert config.tab_name == "{agent}"
    assert config.allow == ("git", "gh")
    assert config.prefixes == ("with-proxy",)
    # Cargo is disabled by default. This limits a project's explicit trust widening.
    assert "build" not in config.allow_subcommand["cargo"]
    assert "fetch" in config.allow_subcommand["cargo"]
    assert config.readiness == "both"
    # Named as a literal 32 rather than compared against DEFAULT_MAX_PANES: a test that read the
    # production constant would agree with any value the constant ever took, including a typo, and
    # so would pin nothing at all.
    assert config.max_panes == 32


def test_the_allow_wildcard_must_stand_alone() -> None:
    """``allow: ["*", "git"]`` reads narrower than it is, so it is a configuration error."""
    with pytest.raises(ConfigError, match="must be the only entry"):
        parse_config({"allow": ["*", "git"]}, source_path="x.yaml", project_root="/tmp")

    wildcard = parse_config(
        {"allow": ["*"]}, source_path="x.yaml", project_root="/tmp"
    )
    assert wildcard.allows_any_program()
    assert not Config().allows_any_program()


def test_no_config_file_anywhere_yields_defaults(tmp_path: object) -> None:
    root = str(tmp_path)
    config = load_config(explicit_path=None, start_dir=root)
    assert config.source_path is None
    assert config.allow == ("git", "gh")


# --- discovery -----------------------------------------------------------------------------------


def test_finds_config_in_an_ancestor_directory(tmp_path: object) -> None:
    root = str(tmp_path)
    nested = os.path.join(root, "a", "b", "c")
    os.makedirs(nested)
    path = os.path.join(root, ".herdr-run.yaml")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("workspace: from-ancestor\n")
    assert find_config_file(nested) == path


def test_nearest_config_wins(tmp_path: object) -> None:
    root = str(tmp_path)
    nested = os.path.join(root, "slot")
    os.makedirs(nested)
    for directory, label in ((root, "outer"), (nested, "inner")):
        with open(
            os.path.join(directory, ".herdr-run.yaml"), "w", encoding="utf-8"
        ) as handle:
            handle.write(f"workspace: {label}\n")
    config = load_config(explicit_path=None, start_dir=nested)
    assert config.workspace == "inner"


def test_project_root_is_the_config_directory(tmp_path: object) -> None:
    root = str(tmp_path)
    nested = os.path.join(root, "deep", "path")
    os.makedirs(nested)
    with open(os.path.join(root, ".herdr-run.yaml"), "w", encoding="utf-8") as handle:
        handle.write("workspace: x\n")
    config = load_config(explicit_path=None, start_dir=nested)
    assert config.project_root == root


def test_missing_explicit_path_is_an_error(tmp_path: object) -> None:
    with pytest.raises(ConfigError, match="not found"):
        load_config(
            explicit_path=os.path.join(str(tmp_path), "nope.yaml"),
            start_dir=str(tmp_path),
        )


# --- parsing / validation ---------------------------------------------------------------------------


def test_parses_every_supported_key() -> None:
    document = {
        "workspace": "cmds",
        "tab_name": "{project}-{agent}",
        "cwd": "sub",
        "allow": ["git"],
        "prefixes": [],
        "deny_global": {"git": ["-c"]},
        "deny_subcommand": {"git": ["daemon"]},
        "deny_anywhere": ["--upload-pack"],
        "value_options": {"git": ["-C"]},
        "spool_dir": "out",
        "timeout_seconds": 30,
        "ready_timeout_seconds": 5,
        "readiness": "process",
        "prompt_tail": "% ",
        "shells": ["zsh"],
        "broker": "systemd-run",
    }
    config = parse_config(document, source_path="/tmp/x.yaml", project_root="/tmp")
    assert config.workspace == "cmds"
    assert config.allow == ("git",)
    assert config.prefixes == ()
    assert config.readiness == "process"
    assert config.broker == "systemd-run"
    assert config.timeout_seconds == 30.0


def test_empty_document_is_defaults() -> None:
    config = parse_config(None, source_path="/tmp/x.yaml", project_root="/tmp")
    assert config.allow == ("git", "gh")


def test_unknown_key_is_rejected() -> None:
    """A typo'd `allowlist:` must not silently fall back to the default allowlist."""
    with pytest.raises(ConfigError, match="unknown key"):
        parse_config(
            {"allowlist": ["git"]}, source_path="/tmp/x.yaml", project_root="/tmp"
        )


def test_empty_allowlist_is_rejected() -> None:
    with pytest.raises(ConfigError, match="EMPTY allowlist"):
        parse_config({"allow": []}, source_path="/tmp/x.yaml", project_root="/tmp")


def test_cargo_allow_requires_its_positive_subcommand_map() -> None:
    with pytest.raises(ConfigError, match="cargo is allowed but has no positive"):
        parse_config(
            {
                "allow": ["git", "cargo"],
                "allow_subcommand": {"custom-tool": ["inspect"]},
            },
            source_path="/tmp/x.yaml",
            project_root="/tmp",
        )


@pytest.mark.parametrize(
    "document,pattern",
    [
        ({"allow": "git"}, "expected an array"),
        ({"allow": [1, 2]}, "must be a string"),
        ({"workspace": 42}, "must be a string"),
        ({"timeout_seconds": "soon"}, "must be a number"),
        ({"timeout_seconds": -1}, "must not be negative"),
        ({"timeout_seconds": MAX_TIMEOUT_SECONDS + 1}, "must not exceed"),
        ({"retention_days": -1}, "non-negative integer"),
        ({"retention_days": 1.5}, "non-negative integer"),
        ({"retention_days": "4"}, "non-negative integer"),
        ({"retention_days": MAX_RETENTION_DAYS + 1}, "must not exceed"),
        ({"readiness": "maybe"}, "must be one of"),
        ({"broker": "carrier-pigeon"}, "must be one of"),
        ({"deny_global": ["git"]}, "expected an object"),
    ],
)
def test_malformed_values_are_rejected(
    document: dict[str, object], pattern: str
) -> None:
    with pytest.raises(ConfigError, match=pattern):
        parse_config(document, source_path="/tmp/x.yaml", project_root="/tmp")


def test_null_optional_fields_are_accepted() -> None:
    config = parse_config(
        {"cwd": None, "prompt_tail": None},
        source_path="/tmp/x.yaml",
        project_root="/tmp",
    )
    assert config.cwd is None
    assert config.prompt_tail is None


@pytest.mark.parametrize(
    "schema",
    ["{", "{nope}", "{agent.__class__}", "{agent!r}", "{agent:>10}", "{0}"],
)
def test_tab_name_rejects_everything_except_plain_named_placeholders(
    schema: str,
) -> None:
    with pytest.raises(ConfigError, match="tab_name schema"):
        parse_config(
            {"tab_name": schema}, source_path="/tmp/x.yaml", project_root="/tmp"
        )


def test_tab_name_allows_literal_braces_and_both_named_placeholders() -> None:
    config = parse_config(
        {"tab_name": "{{commands}}-{project}-{agent}"},
        source_path="/tmp/x.yaml",
        project_root="/tmp",
    )
    assert config.tab_name == "{{commands}}-{project}-{agent}"


def test_yaml_uses_core_12_scalar_resolution(tmp_path: object) -> None:
    """The YAML-1.1 words yes/on/off stay strings; 0o10 is the YAML-1.2 octal spelling."""
    pytest.importorskip("yaml")
    root = str(tmp_path)
    path = os.path.join(root, ".herdr-run.yaml")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("workspace: yes\nallow: [git, on, off]\ntimeout_seconds: 0o10\n")

    config = load_config(explicit_path=path, start_dir=root)
    assert config.workspace == "yes"
    assert config.allow == ("git", "on", "off")
    assert config.timeout_seconds == 8.0


@pytest.mark.parametrize(
    "document,pattern",
    [
        ("allow: [git]\nallow: [gh]\n", "duplicate mapping key"),
        (
            "deny_global:\n  git: [-c]\n  git: [--exec-path]\n",
            "duplicate mapping key",
        ),
        (
            "defaults: &defaults\n  workspace: inherited\n<<: *defaults\n",
            "merge keys are not supported",
        ),
        ("timeout_seconds: .inf\n", "non-finite YAML numbers"),
        ("timeout_seconds: -.Inf\n", "non-finite YAML numbers"),
        ("ready_timeout_seconds: .nan\n", "non-finite YAML numbers"),
    ],
)
def test_strict_yaml_rejects_ambiguous_policy_documents(
    tmp_path: object, document: str, pattern: str
) -> None:
    pytest.importorskip("yaml")
    root = str(tmp_path)
    path = os.path.join(root, ".herdr-run.yaml")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(document)

    with pytest.raises(ConfigError, match=pattern):
        load_config(explicit_path=path, start_dir=root)


@pytest.mark.parametrize("value", [float("inf"), float("-inf"), float("nan")])
def test_decoded_nonfinite_timeouts_are_rejected(value: float) -> None:
    with pytest.raises(ConfigError, match="finite"):
        parse_config(
            {"timeout_seconds": value}, source_path="/tmp/x.yaml", project_root="/tmp"
        )


def test_huge_yaml_integer_is_a_typed_config_error(tmp_path: object) -> None:
    pytest.importorskip("yaml")
    path = os.path.join(str(tmp_path), ".herdr-run.yaml")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("timeout_seconds: " + "9" * 5_000 + "\n")
    with pytest.raises(ConfigError, match="invalid YAML"):
        load_config(explicit_path=path, start_dir=str(tmp_path))


def test_yaml_unicode_surrogate_is_a_typed_config_error(tmp_path: object) -> None:
    pytest.importorskip("yaml")
    path = os.path.join(str(tmp_path), ".herdr-run.yaml")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write('workspace: "\\uD800"\n')
    with pytest.raises(ConfigError, match="surrogate"):
        load_config(explicit_path=path, start_dir=str(tmp_path))


# --- the one shipped configuration document is a valid config ---------------------------------------


def test_shipped_config_template_parses() -> None:
    """The single configuration document the package ships must stay loadable.

    This used to point at a second file, ``examples/project.yaml``. A partial example alongside
    the template ``init`` writes is the duplicate-that-drifts the template exists to prevent, so
    the example is gone and the assertion now guards the template itself — the one artefact the
    guide points at.
    """
    yaml = pytest.importorskip("yaml")
    from importlib.resources import files

    text = (files("herdr_run") / "config_template.yaml").read_text(encoding="utf-8")
    config = parse_config(
        yaml.safe_load(text), source_path="template", project_root="/tmp"
    )
    assert config.workspace == "agent-cmds"
    assert config.allow == ("git", "gh")
