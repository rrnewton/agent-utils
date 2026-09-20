"""Extract complete, request-scoped reply blocks from rendered terminal text."""

from __future__ import annotations

import re
from collections.abc import Collection

_NONCE = re.compile(r"[A-Za-z0-9_-]{22}\Z")
_MARKER = re.compile(r"<(?P<close>/?)(?P<protocol>G?CHAT)_REPLY_(?P<nonce>[^<>\s]*)>")
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
    size = len(result.encode("utf-8"))
    if not result.strip() or size > _MAX_REPLY_BYTES:
        raise ValueError("chat reply body must be nonempty and at most 30000 UTF-8 bytes")
    return result


def _scan(text: str, nonces: Collection[str], *, extract: bool) -> tuple[dict[str, list[str]], list[str]]:
    expected = set(nonces)
    if any(_NONCE.fullmatch(nonce) is None for nonce in expected):
        raise ValueError("chat reply nonce must contain 22 base64url characters")
    text = text.replace("\r\n", "\n")
    if any((ord(char) < 32 and char not in "\t\n") or 127 <= ord(char) <= 159 for char in text):
        raise ValueError("chat reply capture must contain rendered text without ANSI or control sequences")
    try:
        text.encode("utf-8")
    except UnicodeEncodeError as error:
        raise ValueError("chat reply capture must be valid UTF-8") from error

    replies: dict[str, list[str]] = {}
    unknown: dict[str, None] = {}
    active: tuple[str, str] | None = None
    opening_margin = ""
    body: list[str] = []
    fence = ""
    prompt_margin: int | None = None
    for line in text.split("\n"):
        normalized, margin = _undecorate(line)
        stripped = line.lstrip(" \t")
        decorated = stripped.startswith(("• ", "⏺ "))
        if active is None:
            # Native TUIs echo wrapped user prompts beneath ›/❯. Their hanging
            # indentation does not turn markers in user text into assistant output.
            if stripped.startswith(("› ", "❯ ")):
                prompt_margin = len(line) - len(stripped) + 2
                continue
            if prompt_margin is not None:
                # A user can type native-looking bullets in the echoed prompt.
                # Only returning to its base margin ends that continuation.
                if not stripped or len(line) - len(stripped) >= prompt_margin:
                    continue
                prompt_margin = None
        marker = _MARKER.fullmatch(normalized)
        if (active is None and marker is not None and not marker["close"]
                and marker["nonce"] in expected and decorated):
            fence = ""
        fence_match = _FENCE.fullmatch(normalized)
        if fence:
            if (fence_match is not None
                    and fence_match["fence"][0] == fence[0]
                    and len(fence_match["fence"]) >= len(fence)
                    and not fence_match["suffix"].strip()):
                fence = ""
            if active is not None and extract:
                body.append(line)
            continue
        if fence_match is not None and (
            fence_match["fence"][0] != "`" or "`" not in fence_match["suffix"]
        ):
            fence = fence_match["fence"]
            if active is not None and extract:
                body.append(line)
            continue

        if marker is None:
            if active is not None and extract:
                body.append(line)
            continue
        nonce = marker["nonce"]
        if nonce not in expected:
            unknown[nonce] = None
        identity = marker["protocol"], nonce
        if active is None:
            if nonce in expected and not marker["close"]:
                active, opening_margin, body = identity, margin, []
            continue
        if not marker["close"]:
            if extract:
                raise ValueError("nested chat reply markers are ambiguous")
            continue
        if identity != active:
            if extract:
                raise ValueError("chat reply closing marker does not match its opening marker")
            continue
        if extract:
            replies.setdefault(nonce, []).append(_body(body, opening_margin, margin))
        active, body = None, []
    return replies, list(unknown)


def extract_replies(text: str, nonces: Collection[str]) -> dict[str, list[str]]:
    """Return ordered complete reply bodies for each expected request nonce.

    Each reply uses standalone ``<CHAT_REPLY_NONCE>`` and
    ``</CHAT_REPLY_NONCE>`` lines. ``GCHAT_REPLY`` blocks are also accepted.
    Opening and closing tag spellings must match.
    Multiple blocks for one nonce remain separate, including identical bodies;
    callers provide durable occurrence-based delivery deduplication across captures.

    Inline examples, quoted markers, native terminal prompt echoes, and markers
    inside Markdown fences are ignored. A native leading ``• ``/``⏺ `` on an
    expected opening marker starts a fresh assistant item, discarding an unrelated
    outside-block fence. Native decorations and shared terminal indentation are
    removed without stripping body bullets or meaningful code indentation.

    ``text`` must be rendered terminal text. CRLF is accepted; ANSI escapes,
    other control characters, invalid nonces/UTF-8, nested blocks, mismatched
    closing markers, and empty or oversized completed replies raise ``ValueError``.
    Incomplete blocks produce no reply. Bodies are limited to 30000 UTF-8 bytes.
    """
    return _scan(text, nonces, extract=True)[0]


def unknown_reply_ids(text: str, nonces: Collection[str]) -> list[str]:
    """Return first-seen unavailable IDs from standalone reply markers.

    Apply the same rendered-text validation and example/prompt filtering as
    :func:`extract_replies`, accepting both ``CHAT_REPLY`` and ``GCHAT_REPLY``.
    Both opening and closing markers are checked, including empty, malformed,
    or incorrectly sized IDs. IDs may contain punctuation; callers must escape
    and bound them when constructing feedback. Duplicate IDs appear once.

    Incomplete, nested, or mismatched blocks do not prevent diagnostics for an
    unavailable ID. Expected IDs still must contain 22 base64url characters.
    """
    return _scan(text, nonces, extract=False)[1]
