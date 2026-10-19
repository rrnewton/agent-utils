"""Tests for the bounded GitHub pull-request metadata cache."""

from __future__ import annotations

from collections.abc import Mapping
import json
import os
from pathlib import Path
import sys

import pytest

from wrkviz.github_metadata import (
    GhApiTransport,
    GitHubMetadataError,
    HttpResponse,
    MAX_API_RESPONSE_BYTES,
    MAX_BODY_EXCERPT_CHARS,
    PullRequestKey,
    PullRequestMetadata,
    PullRequestMetadataCache,
    fetch_pull_request,
    load_pull_request_metadata_cache,
    save_pull_request_metadata_cache,
)


class FakeTransport:
    """Capture a request and return a predetermined response."""

    def __init__(self, response: HttpResponse) -> None:
        self.response = response
        self.calls: list[tuple[str, dict[str, str], float]] = []

    def get(
        self, *, url: str, headers: Mapping[str, str], timeout_seconds: float
    ) -> HttpResponse:
        self.calls.append((url, dict(headers), timeout_seconds))
        return self.response


def _key() -> PullRequestKey:
    return PullRequestKey("sched-ext/scx", 3668)


def _record(
    *, etag: str | None = 'W/"cached"', fetched_at: str = "2026-08-05T10:00:00Z"
) -> PullRequestMetadata:
    return PullRequestMetadata(
        key=_key(),
        title="Drain ineligible tasks due to affinity restrictions",
        state="closed",
        draft=False,
        merged_at="2026-06-22T08:00:00Z",
        body_excerpt="Fix affinity handling.",
        base_ref="main",
        head_label="kkdwvd:mitosis-affn-viol",
        author="kkdwvd",
        updated_at="2026-06-22T08:01:00Z",
        etag=etag,
        fetched_at=fetched_at,
    )


def _api_body(*, body: str = "A useful fix.") -> bytes:
    value: dict[str, object] = {
        "title": "Drain ineligible tasks due to affinity restrictions",
        "state": "closed",
        "draft": False,
        "merged_at": "2026-06-22T08:00:00Z",
        "body": body,
        "base": {"ref": "main"},
        "head": {"label": "kkdwvd:mitosis-affn-viol"},
        "user": {"login": "kkdwvd"},
        "updated_at": "2026-06-22T08:01:00Z",
    }
    return json.dumps(value).encode("utf-8")


def test_cache_object_and_file_round_trip_is_keyed_and_idempotent(
    tmp_path: Path,
) -> None:
    cache = PullRequestMetadataCache((_record(),))
    path = tmp_path / "github" / "pulls.json"

    assert save_pull_request_metadata_cache(path, cache)
    assert not save_pull_request_metadata_cache(path, cache)

    loaded = load_pull_request_metadata_cache(path)
    assert loaded.get(_key()) == _record()
    root = loaded.to_json_obj()
    records = root["records"]
    assert isinstance(records, dict)
    assert list(records) == ["sched-ext/scx#3668"]


def test_absent_cache_is_empty(tmp_path: Path) -> None:
    cache = load_pull_request_metadata_cache(tmp_path / "missing.json")

    assert cache.records == ()


def test_cache_rejects_an_object_key_that_disagrees_with_record() -> None:
    value: object = {
        "schema_version": 1,
        "records": {"sched-ext/scx#7": _record().to_json_obj()},
    }

    with pytest.raises(ValueError, match="does not match"):
        PullRequestMetadataCache.from_json_obj(value)


def test_fetch_parses_fields_bounds_body_and_uses_github_endpoint() -> None:
    long_body = "x" * (MAX_BODY_EXCERPT_CHARS + 50)
    transport = FakeTransport(
        HttpResponse(
            status=200,
            headers={"ETag": 'W/"new"'},
            body=_api_body(body=long_body),
        )
    )

    result = fetch_pull_request(
        _key(),
        fetched_at="2026-08-05T11:00:00Z",
        timeout_seconds=7.5,
        transport=transport,
    )

    assert not result.not_modified
    assert result.metadata.title.startswith("Drain ineligible")
    assert len(result.metadata.body_excerpt) == MAX_BODY_EXCERPT_CHARS
    assert result.metadata.base_ref == "main"
    assert result.metadata.head_label == "kkdwvd:mitosis-affn-viol"
    assert result.metadata.author == "kkdwvd"
    assert result.metadata.etag == 'W/"new"'
    assert transport.calls == [
        (
            "https://api.github.com/repos/sched-ext/scx/pulls/3668",
            {
                "Accept": "application/vnd.github+json",
                "User-Agent": "wrkviz",
                "X-GitHub-Api-Version": "2022-11-28",
            },
            7.5,
        )
    ]


def test_conditional_fetch_sends_the_cached_etag() -> None:
    cached = _record()
    transport = FakeTransport(HttpResponse(status=304, headers={}, body=b""))

    result = fetch_pull_request(
        _key(),
        cached=cached,
        fetched_at="2026-08-05T12:00:00Z",
        transport=transport,
    )

    assert result.not_modified
    assert result.metadata is cached
    headers = transport.calls[0][1]
    assert headers["If-None-Match"] == 'W/"cached"'
    assert "Authorization" not in headers


def test_not_modified_without_cache_is_rejected() -> None:
    transport = FakeTransport(HttpResponse(status=304, headers={}, body=b""))

    with pytest.raises(GitHubMetadataError, match="without a cached record"):
        fetch_pull_request(_key(), transport=transport)


def test_fetch_rejects_oversized_injected_response() -> None:
    transport = FakeTransport(
        HttpResponse(
            status=200,
            headers={},
            body=b"x" * (MAX_API_RESPONSE_BYTES + 1),
        )
    )

    with pytest.raises(GitHubMetadataError, match="size limit"):
        fetch_pull_request(_key(), transport=transport)


def test_gh_transport_rejects_non_github_url_before_running_gh() -> None:
    transport = GhApiTransport(gh="/nonexistent/gh")

    with pytest.raises(ValueError, match="api.github.com"):
        transport.get(
            url="https://example.com/repos/owner/repo/pulls/1",
            headers={},
            timeout_seconds=1.0,
        )


def test_gh_transport_rejects_an_unrelated_github_api_endpoint() -> None:
    transport = GhApiTransport(gh="/nonexistent/gh")

    with pytest.raises(ValueError, match="api.github.com"):
        transport.get(
            url="https://api.github.com/user",
            headers={},
            timeout_seconds=1.0,
        )


@pytest.mark.parametrize(
    "repository,number",
    [
        ("owner", 1),
        ("owner/repository/extra", 1),
        ("https://github.com/owner/repository", 1),
        ("owner/repository", 0),
    ],
)
def test_invalid_pull_request_key_is_rejected(repository: str, number: int) -> None:
    with pytest.raises(ValueError):
        PullRequestKey(repository, number)


def _fake_gh(tmp_path: Path, output: bytes, exit_code: int = 0) -> tuple[Path, Path]:
    """A gh stand-in that records its argv and prints a canned --include response."""

    calls = tmp_path / "gh-argv.json"
    canned = tmp_path / "gh-output"
    canned.write_bytes(output)
    gh = tmp_path / "gh"
    gh.write_text(
        f"#!{sys.executable}\n"
        "import json, sys\n"
        f"open({str(calls)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
        f"sys.stdout.buffer.write(open({str(canned)!r}, 'rb').read())\n"
        f"sys.exit({exit_code})\n"
    )
    gh.chmod(0o755)
    return gh, calls


def test_default_fetch_runs_gh_api_and_parses_its_included_response(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Every request goes through `gh api` (the gh on PATH, which may be a
    # rate-limiting wrapper), never a connection of this module's own.
    body = _api_body()
    output = (
        b"HTTP/2.0 200 OK\nContent-Type: application/json\r\n"
        b'Etag: "fresh"\r\n\r\n' + body
    )
    gh, calls = _fake_gh(tmp_path, output)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ.get('PATH', '')}")
    monkeypatch.setenv("GH_TOKEN", "must-not-be-read")

    result = fetch_pull_request(_key(), cached=_record(), fetched_at="2026-08-05T12:00:00Z")

    assert not result.not_modified
    assert result.metadata.etag == '"fresh"'
    argv = json.loads(calls.read_text())
    assert argv[:4] == ["api", "--include", "--method", "GET"]
    assert argv[-1] == "repos/sched-ext/scx/pulls/3668"
    assert "If-None-Match: W/\"cached\"" in argv
    assert not any("must-not-be-read" in arg or "Authorization" in arg for arg in argv)


def test_gh_transport_returns_a_not_modified_response(tmp_path: Path) -> None:
    gh, _ = _fake_gh(tmp_path, b"HTTP/2.0 304 Not Modified\nEtag: W/\"cached\"\r\n\r\n", 1)
    cached = _record()

    result = fetch_pull_request(_key(), cached=cached, transport=GhApiTransport(gh=str(gh)))

    assert result.not_modified
    assert result.metadata is cached


def test_gh_transport_refuses_a_bearer_token_and_a_failed_gh(tmp_path: Path) -> None:
    gh, calls = _fake_gh(tmp_path, b"", 1)
    transport = GhApiTransport(gh=str(gh))
    url = _key().api_url

    with pytest.raises(GitHubMetadataError, match="refusing to forward a bearer token"):
        transport.get(url=url, headers={"Authorization": "Bearer x"}, timeout_seconds=5.0)
    assert not calls.exists()
    with pytest.raises(GitHubMetadataError, match="gh api failed"):
        transport.get(url=url, headers={}, timeout_seconds=5.0)


@pytest.mark.parametrize("timeout", [0.0, -1.0, float("inf"), float("nan")])
def test_timeout_must_be_finite_and_positive(timeout: float) -> None:
    transport = FakeTransport(HttpResponse(status=200, headers={}, body=_api_body()))

    with pytest.raises(ValueError, match="finite and positive"):
        fetch_pull_request(_key(), timeout_seconds=timeout, transport=transport)

    assert transport.calls == []
