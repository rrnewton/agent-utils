"""Extract complete, request-scoped reply blocks from rendered terminal text."""

from __future__ import annotations

import re
from collections.abc import Collection

_NONCE = re.compile(r"[A-Za-z0-9_-]{22}\Z")
_MARKER = re.compile(r"<(?P<close>/?)GCHAT_REPLY_(?P<nonce>[A-Za-z0-9_-]{22})>")
_FENCE = re.compile(r"(?P<fence>`{3,}|~{3,})(?P<suffix>.*)")
_MAX_REPLY_BYTES = 30_000


def _undecorate(line: str) -> tuple[str, str]:
    """Return marker text and its margin, replacing a native leading bullet."""
    stripped = line.lstrip(" \t")
    margin = line[:len(line) - len(stripped)]
    if stripped.startswith(("• ", "⏺ ")):
        stripped = stripped[2:]
        margin += "  "
        extra = len(stripped) - len(stripped.lstrip(" \t"))
        margin += stripped[:extra]
        stripped = stripped[extra:]
    return stripped.rstrip(" \t"), margin


def _body(lines: list[str], opening_margin: str, closing_margin: str) -> str:
    """Remove only indentation shared by the markers and nonblank body lines."""
    margin_length = len(opening_margin)
    for line in [closing_margin, *(line for line in lines if line.strip())]:
        common = 0
        limit = min(margin_length, len(line))
        while common < limit and opening_margin[common] == line[common]:
            common += 1
        margin_length = common
        if not margin_length:
            break
    margin = opening_margin[:margin_length]
    result = "\n".join(line[len(margin):] if line.startswith(margin) else line for line in lines)
    try:
        size = len(result.encode("utf-8"))
    except UnicodeEncodeError as error:
        raise ValueError("chat reply body must be valid UTF-8") from error
    if not result.strip() or size > _MAX_REPLY_BYTES:
        raise ValueError("chat reply body must be nonempty and at most 30000 UTF-8 bytes")
    return result


def extract_replies(text: str, nonces: Collection[str]) -> dict[str, str]:
    """Return complete replies keyed by their expected 22-character nonce.

    Each reply uses standalone ``<GCHAT_REPLY_NONCE>`` and
    ``</GCHAT_REPLY_NONCE>`` lines. Inline examples and markers inside Markdown
    fences are ignored. A native leading ``• ``/``⏺ `` on an expected opening
    marker starts a fresh assistant item, discarding an unrelated outside-block
    fence. Native decorations and shared terminal indentation are removed from
    markers without stripping body bullets or meaningful code indentation.
    Incomplete blocks produce no reply; identical complete blocks from terminal
    redraws produce one reply.

    ``text`` must be rendered terminal text, not a raw byte-stream transcript.
    CRLF is accepted, but ANSI escapes, lone carriage returns, other control
    characters, invalid nonces, conflicting/nested blocks, mismatched closing
    markers, and empty or oversized completed replies raise ``ValueError``.
    Bodies are limited to 30000 UTF-8 bytes. Callers retain request ownership,
    unique nonces, and durable delivery deduplication across snapshots.
    """
    expected = set(nonces)
    if any(_NONCE.fullmatch(nonce) is None for nonce in expected):
        raise ValueError("chat reply nonce must contain 22 base64url characters")
    text = text.replace("\r\n", "\n")
    if any((ord(char) < 32 and char not in "\t\n") or 127 <= ord(char) <= 159 for char in text):
        raise ValueError("chat reply capture must contain rendered text without ANSI or control sequences")

    replies: dict[str, str] = {}
    active: str | None = None
    opening_margin = ""
    body: list[str] = []
    fence = ""
    for line in text.split("\n"):
        normalized, margin = _undecorate(line)
        marker = _MARKER.fullmatch(normalized)
        if (active is None and marker is not None and not marker["close"]
                and marker["nonce"] in expected
                and line.lstrip(" \t").startswith(("• ", "⏺ "))):
            fence = ""
        fence_match = _FENCE.fullmatch(normalized)
        if fence:
            if (fence_match is not None
                    and fence_match["fence"][0] == fence[0]
                    and len(fence_match["fence"]) >= len(fence)
                    and not fence_match["suffix"].strip()):
                fence = ""
            if active is not None:
                body.append(line)
            continue
        if fence_match is not None and (
            fence_match["fence"][0] != "`" or "`" not in fence_match["suffix"]
        ):
            fence = fence_match["fence"]
            if active is not None:
                body.append(line)
            continue

        if marker is None:
            if active is not None:
                body.append(line)
            continue
        nonce = marker["nonce"]
        if active is None:
            if nonce in expected and not marker["close"]:
                active, opening_margin, body = nonce, margin, []
            continue
        if not marker["close"]:
            raise ValueError("nested chat reply markers are ambiguous")
        if nonce != active:
            raise ValueError("chat reply closing marker does not match its opening marker")
        answer = _body(body, opening_margin, margin)
        if active in replies and replies[active] != answer:
            raise ValueError("conflicting chat reply bodies for the same nonce")
        replies[active] = answer
        active, body = None, []
    return replies
