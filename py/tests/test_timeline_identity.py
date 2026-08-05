from __future__ import annotations

from agent_team_timeline.archive import narrow_json
from agent_team_timeline.identity import (
    HostIdentity,
    ProjectIdentity,
    SiteIdentity,
    canonical_repository_url,
    infer_structured_identity,
    merge_site_identity,
    parse_identity_overrides,
    site_identity_from_json_obj,
)


def test_repository_remotes_become_safe_browser_urls() -> None:
    assert canonical_repository_url("git@github.com:rrnewton/dev-hermit.git") == (
        "https://github.com/rrnewton/dev-hermit"
    )
    assert canonical_repository_url(
        "https://GitHub.com/rrnewton/agent-utils.git?token=secret#fragment"
    ) == "https://github.com/rrnewton/agent-utils"


def test_structured_codex_metadata_infers_projects_without_prompt_scanning() -> None:
    projects, hosts = infer_structured_identity(
        (
            {
                "cwd": "/home/newton/work/dev-hermit",
                "git": {
                    "repository_url": "https://github.com/rrnewton/dev-hermit.git"
                },
            },
            {
                "cwd": "/home/newton/work/agent-utils",
                "git": {
                    "repository_url": "git@github.com:rrnewton/agent-utils.git"
                },
                "hostname": "devbig014.example.com",
            },
        )
    )

    assert [(item.label, item.primary) for item in projects] == [
        ("dev-hermit", True),
        ("agent-utils", False),
    ]
    assert [item.hostname for item in hosts] == ["devbig014.example.com"]
    assert all(item.source == "session_metadata" for item in projects)
    assert all(item.source == "session_metadata" for item in hosts)


def test_identity_merge_accumulates_distinct_projects_and_hosts() -> None:
    previous = SiteIdentity(
        "codex-hermit",
        (
            ProjectIdentity(
                "dev-hermit",
                "https://github.com/rrnewton/dev-hermit",
                True,
                "session_metadata",
            ),
        ),
        (HostIdentity("devbig014.example.com", "explicit"),),
        "America/New_York",
        "explicit",
    )
    inferred = (
        ProjectIdentity(
            "agent-utils",
            "https://github.com/rrnewton/agent-utils",
            True,
            "session_metadata",
        ),
    )
    explicit, explicit_hosts = parse_identity_overrides(
        ("Hermit=https://github.com/facebookexperimental/hermit.git",),
        ("devbig015",),
    )

    merged = merge_site_identity(
        "codex-hermit",
        "America/New_York",
        "explicit",
        inferred,
        (),
        explicit,
        explicit_hosts,
        previous,
    )

    assert [item.label for item in merged.projects] == [
        "dev-hermit",
        "agent-utils",
        "Hermit",
    ]
    assert [item.label for item in merged.projects if item.primary] == ["Hermit"]
    assert [item.hostname for item in merged.hosts] == [
        "devbig014.example.com",
        "devbig015",
    ]


def test_site_identity_json_round_trip() -> None:
    identity = SiteIdentity(
        "codex-hermit",
        (
            ProjectIdentity(
                "dev-hermit",
                "https://github.com/rrnewton/dev-hermit",
                True,
                "explicit",
            ),
        ),
        (HostIdentity("devbig014", "explicit"),),
        "America/New_York",
        "explicit",
    )

    parsed = site_identity_from_json_obj(
        narrow_json(identity.to_json_obj()), "site-identity.json"
    )

    assert parsed == identity


def test_default_timezone_rerun_preserves_explicit_provenance() -> None:
    previous = SiteIdentity(
        "codex-hermit",
        (),
        (),
        "America/New_York",
        "explicit",
    )

    unchanged = merge_site_identity(
        "codex-hermit",
        "America/New_York",
        "default",
        (),
        (),
        (),
        (),
        previous,
    )
    changed = merge_site_identity(
        "codex-hermit",
        "UTC",
        "default",
        (),
        (),
        (),
        (),
        previous,
    )

    assert unchanged.display_timezone_source == "explicit"
    assert changed.display_timezone_source == "default"
