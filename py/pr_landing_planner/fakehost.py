"""A deterministic, network-free :class:`~pr_landing_planner.host.VcsHost` for tests + the demo.

:class:`FakeHost` answers every host operation from an in-memory fixture (loadable from JSON or
YAML), so the whole planner — collection, conflict graph, CI classification, fusion, and all three
output formats — is exercisable with zero network and byte-stable output. The fixture can also
simulate a PR moving mid-collection (``fetched_head_sha`` differing from ``api_head_sha``) to test
the content-identity guard.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    # PyYAML is optional: only the YAML branch of load_fixture_text needs it, and it imports yaml
    # lazily there. JSON fixtures, `--help`, and `--version` never need the dependency.
    import yaml

from pr_landing_planner.model import CheckRun, RawPr, edge_key


class FixtureError(ValueError):
    """Raised when a FakeHost fixture document is malformed."""


# Surfaced (as a FixtureError, so the CLI prints it cleanly) when a YAML fixture is requested but
# PyYAML is not installed. JSON fixtures need no optional dependency.
_MISSING_YAML_MSG = (
    "loading a YAML fixture requires the optional PyYAML dependency, which is not installed. "
    "Install it with: python3 -m pip install 'pyyaml>=6'  (or run: agent-utils/setup). "
    "JSON fixtures need no extra dependency."
)


# --------------------------------------------------------------------------- narrowing helpers
def _as_obj(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise FixtureError(f"{where}: expected an object, got {type(value).__name__}")
    return {str(k): v for k, v in value.items()}


def _opt_str(m: Mapping[str, object], key: str, default: str) -> str:
    val = m.get(key, default)
    if isinstance(val, (int, float)) and not isinstance(val, bool):
        return str(val)
    return val if isinstance(val, str) else default


def _req_int(m: Mapping[str, object], key: str, where: str) -> int:
    val = m.get(key)
    if isinstance(val, bool) or not isinstance(val, int):
        raise FixtureError(f"{where}: field {key!r} must be an integer")
    return val


def _opt_int(m: Mapping[str, object], key: str, default: int) -> int:
    val = m.get(key, default)
    return val if isinstance(val, int) and not isinstance(val, bool) else default


def _opt_bool(m: Mapping[str, object], key: str, default: bool) -> bool:
    val = m.get(key, default)
    return val if isinstance(val, bool) else default


def _opt_str_list(m: Mapping[str, object], key: str) -> tuple[str, ...]:
    val = m.get(key)
    if not isinstance(val, list):
        return ()
    return tuple(str(item) for item in val)


def _checks_from(value: object, where: str) -> tuple[CheckRun, ...]:
    if value is None:
        return ()
    if not isinstance(value, list):
        raise FixtureError(f"{where}: 'checks' must be a list")
    out: list[CheckRun] = []
    for i, entry in enumerate(value):
        obj = _as_obj(entry, f"{where}.checks[{i}]")
        duration = obj.get("duration_secs")
        out.append(
            CheckRun(
                name=_opt_str(obj, "name", ""),
                status=_opt_str(obj, "status", "").upper(),
                conclusion=_opt_str(obj, "conclusion", "").upper(),
                text=_opt_str(obj, "text", ""),
                workflow=_opt_str(obj, "workflow", ""),
                duration_secs=duration if isinstance(duration, int) and not isinstance(duration, bool) else None,
            )
        )
    return tuple(out)


# --------------------------------------------------------------------------- internal PR record
@dataclass(frozen=True)
class _FakePr:
    raw: RawPr
    head_sha: str
    fetched_head_sha: str
    changed_files: frozenset[str]
    base_conflict_paths: tuple[str, ...]
    commits_behind: int


def _fake_pr_from(value: object, where: str) -> _FakePr:
    obj = _as_obj(value, where)
    number = _req_int(obj, "number", where)
    head_sha = _opt_str(obj, "head_sha", f"sha-{number}")
    api_head_sha = _opt_str(obj, "api_head_sha", head_sha)
    fetched_head_sha = _opt_str(obj, "fetched_head_sha", head_sha)
    raw = RawPr(
        number=number,
        head_ref=_opt_str(obj, "head_ref", f"feature-{number}"),
        base_ref=_opt_str(obj, "base_ref", "integration"),
        api_head_sha=api_head_sha,
        title=_opt_str(obj, "title", ""),
        author=_opt_str(obj, "author", ""),
        is_draft=_opt_bool(obj, "is_draft", False),
        mergeable=_opt_str(obj, "mergeable", ""),
        review_decision=_opt_str(obj, "review_decision", ""),
        created_at=_opt_str(obj, "created_at", ""),
        updated_at=_opt_str(obj, "updated_at", ""),
        additions=_opt_int(obj, "additions", 0),
        deletions=_opt_int(obj, "deletions", 0),
        labels=_opt_str_list(obj, "labels"),
        checks=_checks_from(obj.get("checks"), where),
    )
    return _FakePr(
        raw=raw,
        head_sha=head_sha,
        fetched_head_sha=fetched_head_sha,
        changed_files=frozenset(_opt_str_list(obj, "changed_files")),
        base_conflict_paths=_opt_str_list(obj, "base_conflict_paths"),
        commits_behind=_opt_int(obj, "commits_behind", 0),
    )


# --------------------------------------------------------------------------- the fake host
class FakeHost:
    """A :class:`~pr_landing_planner.host.VcsHost` backed by an in-memory fixture."""

    def __init__(
        self,
        prs: tuple[_FakePr, ...],
        conflicts: Mapping[tuple[int, int], tuple[str, ...]],
        ancestry: frozenset[tuple[int, int]],
        base_shas: Mapping[str, str],
    ) -> None:
        self._prs = prs
        self._conflicts = dict(conflicts)
        self._ancestry = ancestry
        self._base_shas = dict(base_shas)
        self._by_number = {p.raw.number: p for p in prs}
        self._number_by_sha: dict[str, int] = {}
        for p in prs:
            self._number_by_sha[p.head_sha] = p.raw.number
            self._number_by_sha[p.fetched_head_sha] = p.raw.number
        self._base_ref_by_sha = {sha: ref for ref, sha in self._base_shas.items()}

    # --- VcsHost protocol -------------------------------------------------
    def list_open_prs(self, repo: str, base: str | None) -> tuple[RawPr, ...]:
        return tuple(p.raw for p in self._prs)

    def fetch_ref(self, source: str, dest: str) -> str:
        if source.startswith("refs/heads/"):
            ref = source[len("refs/heads/") :]
            return self._base_shas.get(ref, f"basesha-{ref}")
        if source.startswith("refs/pull/") and source.endswith("/head"):
            middle = source[len("refs/pull/") : -len("/head")]
            number = int(middle)
            pr = self._by_number.get(number)
            if pr is None:
                raise FixtureError(f"fetch_ref: unknown PR in {source!r}")
            return pr.fetched_head_sha
        raise FixtureError(f"fetch_ref: unrecognized source ref {source!r}")

    def merge_tree(self, left: str, right: str) -> tuple[str, ...]:
        left_pr = self._number_by_sha.get(left)
        right_pr = self._number_by_sha.get(right)
        left_base = left in self._base_ref_by_sha
        right_base = right in self._base_ref_by_sha
        if left_base and right_pr is not None:
            return self._by_number[right_pr].base_conflict_paths
        if right_base and left_pr is not None:
            return self._by_number[left_pr].base_conflict_paths
        if left_pr is not None and right_pr is not None:
            return self._conflicts.get(edge_key(left_pr, right_pr), ())
        return ()

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        a = self._number_by_sha.get(ancestor)
        d = self._number_by_sha.get(descendant)
        if a is None or d is None:
            return False
        return (a, d) in self._ancestry

    def changed_files(self, base_sha: str, head_sha: str) -> frozenset[str]:
        number = self._number_by_sha.get(head_sha)
        return self._by_number[number].changed_files if number is not None else frozenset()

    def commits_behind(self, head_sha: str, base_sha: str) -> int:
        number = self._number_by_sha.get(head_sha)
        return self._by_number[number].commits_behind if number is not None else 0

    # --- construction -----------------------------------------------------
    @classmethod
    def from_fixture(cls, raw: object) -> tuple["FakeHost", str, str]:
        """Build a FakeHost from a parsed fixture. Returns ``(host, repo, base)``."""
        doc = _as_obj(raw, "<root>")
        repo = _opt_str(doc, "repo", "owner/repo")
        base = _opt_str(doc, "base", "integration")
        prs_raw = doc.get("prs")
        if not isinstance(prs_raw, list):
            raise FixtureError("<root>: 'prs' must be a list")
        prs = tuple(_fake_pr_from(entry, f"prs[{i}]") for i, entry in enumerate(prs_raw))

        conflicts: dict[tuple[int, int], tuple[str, ...]] = {}
        for i, entry in enumerate(_as_list(doc.get("conflicts"))):
            obj = _as_obj(entry, f"conflicts[{i}]")
            a = _req_int(obj, "a", f"conflicts[{i}]")
            b = _req_int(obj, "b", f"conflicts[{i}]")
            conflicts[edge_key(a, b)] = _opt_str_list(obj, "paths") or ("<conflict>",)

        ancestry: set[tuple[int, int]] = set()
        for i, entry in enumerate(_as_list(doc.get("ancestry"))):
            obj = _as_obj(entry, f"ancestry[{i}]")
            ancestry.add((_req_int(obj, "before", f"ancestry[{i}]"), _req_int(obj, "after", f"ancestry[{i}]")))

        base_refs = {p.raw.base_ref for p in prs}
        base_shas = {ref: f"basesha-{ref}" for ref in base_refs}
        return cls(prs, conflicts, frozenset(ancestry), base_shas), repo, base


def _as_list(value: object) -> list[object]:
    return list(value) if isinstance(value, list) else []


def load_fixture_text(text: str, *, as_yaml: bool) -> object:
    """Parse a fixture document from ``text`` (YAML or JSON) into a plain ``object`` (no Any leak).

    PyYAML is optional; a YAML fixture requested without it raises :class:`FixtureError` with an
    actionable install hint (JSON fixtures work without it)."""
    if as_yaml:
        try:
            import yaml
        except ModuleNotFoundError as exc:
            raise FixtureError(_MISSING_YAML_MSG) from exc
        raw: object = yaml.safe_load(text)
    else:
        try:
            raw = json.loads(text)
        except json.JSONDecodeError as exc:
            raise FixtureError(f"invalid JSON fixture: {exc}") from exc
    return raw


__all__ = ["FakeHost", "FixtureError", "load_fixture_text"]
