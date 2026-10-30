"""The coordinator's transcript as the source of its chat replies' text (#710 chat-transcript-extraction).

A terminal shows a reply rendered: wrapped at its width, with Markdown drawn as styles and tables as
box drawing, and Claude Code collapses text written in the middle of a turn to one summary line. The
coordinator's harness also appends the conversation to a transcript of its own, where each reply is
exactly as the agent wrote it:

* Claude Code: ``<config>/projects/<project>/<session>.jsonl`` (``assistant`` entries' ``text``
  blocks); the running process names its session in ``<config>/sessions/<pid>.json``;
* Codex: ``<home>/sessions/YYYY/MM/DD/rollout-*.jsonl`` (``response_item`` assistant messages),
  held open by the running process;
* Muse: ``<data>/muse/sessions/YYYY/MM/DD/<session>/session.jsonl`` (``assistant_message_committed``
  run events), whose ``.session.lock`` the runtime holds open.

:class:`CoordinatorTranscript` finds the transcript of the harness running in the coordinator's
pane (the processes whose ``HERDR_PANE_ID`` names the pane), follows it, and gives the capture its
complete reply blocks. The capture puts a block's transcript text in place of its screen text
when the screen's block renders it (:func:`rendered_equivalent`), and stores the transcript's
complete blocks that no screen showed. Without a transcript, replies keep their screen text.
"""

from __future__ import annotations

import json
import os
import re
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

_MARKER_ID = re.compile(r"[A-Za-z0-9_-]+\Z")
_RELOCATE_EVERY = 30.0
_FIRST_READ_BYTES = 32 * 1024 * 1024
_MAX_READ_BYTES = 32 * 1024 * 1024
_MAX_BLOCKS = 512
_MAX_BLOCK_BYTES = 4 * 1024 * 1024
_SETTLE_FOR = 1.0
_SETTLE_STEP = 0.2
_MAX_WAITED = 256


@dataclass(frozen=True)
class TranscriptBlock:
    """One complete reply block read from a transcript."""

    identifier: str
    body: str
    #: When the transcript recorded it, in Unix milliseconds, when the transcript says.
    at_millis: int | None


def _marker(line: str, closing: bool) -> str | None:
    prefix = "</CHAT_REPLY_" if closing else "<CHAT_REPLY_"
    text = line.strip()
    if not (text.startswith(prefix) and text.endswith(">")):
        return None
    identifier = text[len(prefix):-1]
    return identifier if _MARKER_ID.match(identifier) else None


def _fence(line: str) -> tuple[str, int] | None:
    text = line.lstrip()
    if not text or text[0] not in "`~":
        return None
    character = text[0]
    length = len(text) - len(text.lstrip(character))
    if length < 3 or (character == "`" and "`" in text[length:]):
        return None
    return character, length


def _closes(line: str, run: tuple[str, int]) -> bool:
    text = line.strip()
    length = len(text) - len(text.lstrip(run[0]))
    return length >= run[1] and text == run[0] * len(text)


def _dedent(lines: list[str]) -> str:
    nonblank = [index for index, line in enumerate(lines) if line.strip()]
    if not nonblank:
        return ""
    kept = lines[nonblank[0]:nonblank[-1] + 1]
    indent = min(len(line) - len(line.lstrip()) for line in kept if line.strip())
    return "\n".join(line[indent:].rstrip() if len(line) >= indent else line.strip() for line in kept)


def complete_blocks(text: str) -> list[tuple[str, str]]:
    """Every complete ``(identifier, body)`` reply block in ``text``, in order.

    A block opens at a marker line outside any code fence and closes at the closing marker line of
    the same ID, with at least one nonblank line between. Inside a block, code fences belong to the
    reply. An opening marker inside an open block starts a new block in its place.
    """
    blocks: list[tuple[str, str]] = []
    outside: tuple[str, int] | None = None
    open_id: str | None = None
    lines: list[str] = []
    inner: tuple[str, int] | None = None
    for raw in text.split("\n"):
        line = raw[:-1] if raw.endswith("\r") else raw
        if open_id is not None:
            if inner is not None:
                if _closes(line, inner):
                    inner = None
                lines.append(line)
                continue
            if _marker(line, True) == open_id:
                body = _dedent(lines)
                if body.strip():
                    blocks.append((open_id, body))
                open_id = None
                continue
            following = _marker(line, False)
            if following is not None:
                open_id, lines, inner = following, [], None
                continue
            inner = _fence(line)
            lines.append(line)
            continue
        if outside is not None:
            if _closes(line, outside):
                outside = None
            continue
        identifier = _marker(line, False)
        if identifier is not None:
            open_id, lines, inner = identifier, [], None
        else:
            outside = _fence(line)
    return blocks


#: Characters a terminal draws as style or layout rather than as text.
_MARKUP = frozenset("*_`#>~|[]()-+•·●◦▪")
_TABLE_DELIMITER = re.compile(r"[|\-: \t]*\Z")


def rendered_key(text: str) -> str:
    """What survives every way a terminal renders ``text``.

    Its characters in order, without whitespace, box drawing, Markdown markup and list bullets, or
    table delimiter rows; letters, digits, other punctuation and symbols stay.
    """
    return "".join(
        character
        for line in text.splitlines()
        if not ("---" in line and _TABLE_DELIMITER.match(line.strip()))
        for character in line
        if not character.isspace() and not "\u2500" <= character <= "\u257f" and character not in _MARKUP)


_LINK_TARGET = re.compile(r"\]\([^)\n]*\)")


def label_key(text: str) -> str:
    """:func:`rendered_key` of ``text`` without its Markdown link targets."""
    return rendered_key(_LINK_TARGET.sub("]", text))


def rendered_equivalent(screen: str, source: str) -> bool:
    """Whether ``screen`` is a terminal's rendering of ``source``.

    The same :func:`rendered_key`, with or without the source's link targets, or the same characters
    in another order, as when a table's cells wrap across rows.
    """
    shown = rendered_key(screen)
    if not shown:
        return False
    candidates = (rendered_key(source), label_key(source))
    if shown in candidates:
        return True
    return any(sorted(shown) == sorted(key) for key in candidates)


def _shingles(text: str) -> set[str]:
    words = [word.lower() for word in re.split(r"[^\w]|_", text) if word]
    if len(words) < 3:
        return set(words)
    return {" ".join(words[index:index + 3]) for index in range(len(words) - 2)}


def near_duplicate(left: str, right: str) -> bool:
    """Whether two texts share at least three fifths of their word triples."""
    first, second = _shingles(left), _shingles(right)
    if not first or not second:
        return False
    return len(first & second) * 5 >= len(first | second) * 3


def _parse_utc(text: object) -> float | None:
    if not isinstance(text, str) or not text.endswith(("Z", "+00:00")):
        return None
    try:
        return datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _texts(content: object, kind: str, key: str = "type") -> list[str]:
    if not isinstance(content, list):
        return []
    return [item["text"] for item in content
            if isinstance(item, dict) and item.get(key) == kind and isinstance(item.get("text"), str)]


def _claude_texts(entry: dict[str, object]) -> list[str]:
    if entry.get("isSidechain") is True or entry.get("type") != "assistant":
        return []
    message = entry.get("message")
    return _texts(message.get("content") if isinstance(message, dict) else None, "text")


def _codex_texts(entry: dict[str, object]) -> list[str]:
    payload = entry.get("payload")
    if (entry.get("type") != "response_item" or not isinstance(payload, dict)
            or payload.get("type") != "message" or payload.get("role") != "assistant"):
        return []
    found = _texts(payload.get("content"), "output_text")
    return ["\n".join(found)] if found else []


def _muse_records(line: dict[str, object]) -> Iterator[dict[str, object]]:
    children = line.get("children")
    if isinstance(children, list):
        for child in children:
            text = child.get("record_json") if isinstance(child, dict) else None
            if isinstance(text, str):
                try:
                    record = json.loads(text)
                except ValueError:
                    continue
                if isinstance(record, dict):
                    yield record
    if "payload_type" in line:
        yield line


def decode_texts(harness: str, line: bytes) -> list[tuple[float | None, str]]:
    """The model's text that one transcript line of ``harness`` holds, with its time."""
    if harness == "muse" and b"assistant_message_committed" not in line:
        return []
    try:
        entry = json.loads(line)
    except ValueError:
        return []
    if not isinstance(entry, dict):
        return []
    if harness == "claude":
        at = _parse_utc(entry.get("timestamp"))
        return [(at, text) for text in _claude_texts(entry)]
    if harness == "codex":
        at = _parse_utc(entry.get("timestamp"))
        return [(at, text) for text in _codex_texts(entry)]
    if harness == "muse":
        out: list[tuple[float | None, str]] = []
        for record in _muse_records(entry):
            payload = record.get("payload")
            if record.get("payload_type") != "runtime.session" or not isinstance(payload, dict):
                continue
            event = payload.get("event")
            if (payload.get("kind") == "run" and isinstance(event, dict)
                    and event.get("kind") == "assistant_message_committed"
                    and isinstance(event.get("text"), str)):
                micros = record.get("recorded_at")
                at = micros / 1e6 if isinstance(micros, (int, float)) else None
                out.append((at, str(event["text"])))
        return out
    return []


def _pane_processes(proc_root: Path, pane_id: str) -> list[int]:
    """The processes whose environment says they run in Herdr pane ``pane_id``, oldest first."""
    wanted = f"HERDR_PANE_ID={pane_id}".encode()
    found: list[tuple[int, int]] = []
    try:
        entries = list(proc_root.iterdir())
    except OSError:
        return []
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            environ = (entry / "environ").read_bytes()
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        if wanted not in environ.split(b"\0"):
            continue
        fields = stat[stat.rfind(")") + 1:].split()
        start = int(fields[19]) if len(fields) > 19 and fields[19].isdigit() else 0
        found.append((start, int(entry.name)))
    return [pid for _, pid in sorted(found)]


def _environment(proc_root: Path, pid: int, key: str) -> str | None:
    try:
        environ = (proc_root / str(pid) / "environ").read_bytes()
    except OSError:
        return None
    for item in environ.split(b"\0"):
        name, _, value = item.partition(b"=")
        if name.decode(errors="replace") == key and value:
            return value.decode(errors="replace")
    return None


def _open_files(proc_root: Path, pids: list[int]) -> list[Path]:
    files: list[Path] = []
    for pid in pids:
        try:
            descriptors = list((proc_root / str(pid) / "fd").iterdir())
        except OSError:
            continue
        for descriptor in descriptors:
            try:
                target = os.readlink(descriptor)
            except OSError:
                continue
            if target.startswith("/"):
                path = Path(target.removesuffix(" (deleted)"))
                if path not in files:
                    files.append(path)
    return files


def _newest(paths: list[Path]) -> Path | None:
    stamped: list[tuple[float, Path]] = []
    for path in paths:
        try:
            stamped.append((path.stat().st_mtime, path))
        except OSError:
            continue
    return max(stamped)[1] if stamped else None


def project_name(cwd: str) -> str:
    """The directory name Claude Code gives the project at ``cwd``."""
    return "".join(character if character.isascii() and character.isalnum() else "-" for character in cwd)


def locate(harness: str, pids: list[int], cwd: str | None, proc_root: Path = Path("/proc"),
           home: Path | None = None) -> tuple[Path, str] | None:
    """The transcript of ``harness`` run by ``pids`` (and its working directory), and why."""
    if not pids:
        return None
    first = pids[0]
    base = home or Path(_environment(proc_root, first, "HOME") or os.path.expanduser("~"))
    if harness == "claude":
        config = Path(_environment(proc_root, first, "CLAUDE_CONFIG_DIR") or base / ".claude")
        for pid in pids:
            try:
                record = json.loads((config / "sessions" / f"{pid}.json").read_text())
            except (OSError, ValueError):
                continue
            session = record.get("sessionId") if isinstance(record, dict) else None
            if not isinstance(session, str) or not session or "/" in session:
                continue
            directory = record.get("cwd") if isinstance(record.get("cwd"), str) else cwd
            candidates = [config / "projects" / project_name(str(directory)) / f"{session}.jsonl"] if directory else []
            try:
                candidates += [project / f"{session}.jsonl" for project in (config / "projects").iterdir()]
            except OSError:
                pass
            for path in candidates:
                if path.is_file():
                    return path, f"process {pid} names it in sessions/{pid}.json"
        return None
    files = _open_files(proc_root, pids)
    if harness == "codex":
        rollouts = [path for path in files if path.name.startswith("rollout-")
                    and path.name.endswith(".jsonl") and "/sessions/" in str(path)]
        newest = _newest(rollouts)
        return (newest, "the process holds it open") if newest else None
    if harness == "muse":
        journals = [path.parent / "session.jsonl" for path in files
                    if path.name == ".session.lock" and "/muse/sessions/" in str(path)
                    and "/subagent/" not in str(path)]
        newest = _newest([path for path in journals if path.is_file()])
        return (newest, "the process holds its session lock") if newest else None
    return None


class CoordinatorTranscript:
    """The transcript of the harness in one Herdr pane, followed for its reply blocks."""

    def __init__(self, resolve: Callable[[], tuple[str, str, str | None] | None],
                 log: Callable[[str], None], proc_root: Path = Path("/proc"),
                 home: Path | None = None) -> None:
        #: ``resolve`` gives ``(harness, pane_id, cwd)`` for the coordinator now, or ``None``.
        self._resolve = resolve
        self._log = log
        self._proc_root = proc_root
        self._home = home
        self._path: Path | None = None
        self._harness = ""
        self._identity: tuple[int, int] | None = None
        self._offset = 0
        self._partial = b""
        self._located_at: float | None = None
        self._reported: str | None = None
        self._blocks: list[TranscriptBlock] = []
        self._waited: list[str] = []

    def _report(self, line: str) -> None:
        if line != self._reported:
            self._log(line)
            self._reported = line

    def _relocate(self) -> None:
        now = time.monotonic()
        # Looking reads the process table, so it is done at most once per period, found or not.
        if self._located_at is not None and now - self._located_at < _RELOCATE_EVERY:
            return
        self._located_at = now
        target = self._resolve()
        found = None
        if target is not None:
            harness, pane_id, cwd = target
            found = locate(harness, _pane_processes(self._proc_root, pane_id), cwd, self._proc_root, self._home)
        if found is None:
            self._path = None
            self._report("agentctl: no transcript of the coordinator was found; replies keep the text "
                         "read from its screen")
            return
        path, evidence = found
        if path == self._path:
            return
        assert target is not None
        self._path, self._harness = path, target[0]
        self._identity, self._partial = None, b""
        self._report(f"agentctl: reply text comes from the coordinator's {self._harness} transcript "
                     f"{path} ({evidence})")

    def _poll(self) -> None:
        self._relocate()
        if self._path is None:
            return
        try:
            with self._path.open("rb") as handle:
                status = os.fstat(handle.fileno())
                identity = (status.st_dev, status.st_ino)
                aligned = True
                if identity != self._identity or status.st_size < self._offset:
                    self._identity, self._partial = identity, b""
                    self._offset = max(0, status.st_size - _FIRST_READ_BYTES)
                    aligned = self._offset == 0
                if status.st_size - self._offset > _MAX_READ_BYTES:
                    self._offset, self._partial, aligned = status.st_size - _MAX_READ_BYTES, b"", False
                start = self._offset - 1 if not aligned and self._offset > 0 else self._offset
                handle.seek(start)
                data = handle.read(status.st_size - start)
        except OSError as error:
            path, self._path = self._path, None
            self._report(f"agentctl: the coordinator's transcript {path} cannot be read ({error}); "
                         "replies keep the text read from its screen")
            return
        self._offset = start + len(data)
        if not aligned:
            newline = data.find(b"\n")
            data = data[newline + 1:] if newline >= 0 else b""
        *lines, self._partial = (self._partial + data).split(b"\n")
        for line in lines:
            for at, text in decode_texts(self._harness, line) if line else []:
                for identifier, body in complete_blocks(text):
                    millis = int(at * 1000) if at is not None and at >= 0 else None
                    self._blocks.append(TranscriptBlock(identifier, body, millis))
        while self._blocks and (len(self._blocks) > _MAX_BLOCKS
                                or sum(len(block.body) for block in self._blocks) > _MAX_BLOCK_BYTES):
            self._blocks.pop(0)

    def blocks(self) -> list[TranscriptBlock]:
        """The complete reply blocks read up to now, oldest first."""
        self._poll()
        return list(self._blocks)

    def settle(self, identifiers: list[str]) -> None:
        """Wait up to a second for the transcript to hold each new one of ``identifiers``."""
        pending = [identifier for identifier in identifiers if identifier not in self._waited]
        if not pending:
            return
        deadline = time.monotonic() + _SETTLE_FOR
        while True:
            self._poll()
            held = {block.identifier for block in self._blocks}
            if self._path is None or all(identifier in held for identifier in pending) or time.monotonic() >= deadline:
                break
            time.sleep(_SETTLE_STEP)
        self._waited = (self._waited + pending)[-_MAX_WAITED:]


def closing_identifiers(rendered: str) -> list[str]:
    """The IDs of the closing markers alone on their lines in ``rendered``, first appearance first."""
    found: list[str] = []
    for line in rendered.splitlines():
        match = re.fullmatch(r"\s*(?:[⏺●•◆]\s*)?</CHAT_REPLY_([A-Za-z0-9_-]+)>\s*", line)
        if match is not None and match.group(1) not in found:
            found.append(match.group(1))
    return found
