"""Text rules shared by both editions of herdr-run, written out rather than borrowed.

Everything in this module exists because the obvious language built-in gives an answer that
depends on the language, or on the version of the language's Unicode tables: ``repr()`` and
``str.isprintable`` follow the interpreter's Unicode database, ``str.strip`` treats the
information separators U+001C..U+001F as whitespace, ``int()`` accepts underscores and every
script's decimal digits, and ``json.loads`` accepts ``NaN``. Each function here states its rule
explicitly so the same input produces the same bytes on every interpreter, and the native edition
implements the identical rule. Shared golden cases, loaded by both editions' unit tests, hold the
two to it.
"""

from __future__ import annotations

import json
import math
import re
from decimal import Decimal

__all__ = [
    "ASCII_WHITESPACE",
    "BLANK_CHARACTERS",
    "DEFAULT_IGNORABLE_RANGES",
    "FORMAT_CHARACTER_RANGES",
    "JSON_MAX_DEPTH",
    "UNICODE_WHITESPACE",
    "display",
    "format_round3",
    "is_blank",
    "is_default_ignorable",
    "is_format_character",
    "is_invisible",
    "name_violation",
    "number_text",
    "os_error_text",
    "parse_exit_marker",
    "quote",
    "quote_optional",
    "rstrip_space",
    "strict_json_loads",
    "strip_ascii",
    "strip_space",
]

#: The six ASCII whitespace characters POSIX ``isspace`` names, trimmed from an exit-code marker.
#: Information separators and every non-ASCII space are kept, so they make a marker invalid rather
#: than being silently removed.
ASCII_WHITESPACE = " \t\n\r\x0b\x0c"

#: The Unicode ``White_Space`` property, written out. Used where text that came from a terminal is
#: judged blank or trimmed. The list has been stable since Unicode 6.3; writing it out keeps it from
#: following whichever Unicode version an interpreter or compiler happens to ship.
UNICODE_WHITESPACE = frozenset(
    "\t\n\x0b\x0c\r \x85\xa0\u1680"
    "\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a"
    "\u2028\u2029\u202f\u205f\u3000"
)

_UNICODE_WHITESPACE_TEXT = "".join(sorted(UNICODE_WHITESPACE))

#: Unicode general category ``Cf`` (format characters), written out as of Unicode 15.0: the soft
#: hyphen, zero-width spaces and joiners, bidirectional marks, embeddings, overrides and isolates,
#: the byte-order mark, interlinear annotation, tag characters, and the rest. None of them draws
#: anything, so a name that contains one looks exactly like a name that does not.
FORMAT_CHARACTER_RANGES: tuple[tuple[int, int], ...] = (
    (0x00AD, 0x00AD),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
)


#: Unicode ``Default_Ignorable_Code_Point``, written out as of Unicode 16.0: characters a renderer
#: shows as nothing at all unless it supports them specially. Besides most format characters it
#: holds the combining grapheme joiner (U+034F), the Khmer inherent vowels (U+17B4, U+17B5), the
#: Mongolian free variation selectors (U+180B..U+180D, U+180F), the variation selectors
#: (U+FE00..U+FE0F, U+E0100..U+E01EF), the Hangul fillers, and code points reserved for more of
#: the same (U+2065, U+FFF0..U+FFF8, and the unassigned rest of U+E0000..U+E0FFF), so a
#: character a later Unicode version assigns there is refused too.
DEFAULT_IGNORABLE_RANGES: tuple[tuple[int, int], ...] = (
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
)

#: Assigned, ordinary characters that nevertheless draw as empty space: the Hangul fillers
#: (U+115F, U+1160, U+3164, U+FFA0, which are also default-ignorable) and U+2800 BRAILLE PATTERN
#: BLANK, which is neither default-ignorable nor white space and so needs naming on its own.
BLANK_CHARACTERS: frozenset[str] = frozenset("\u115f\u1160\u2800\u3164\uffa0")


def is_format_character(character: str) -> bool:
    """True for a character in :data:`FORMAT_CHARACTER_RANGES`."""
    code = ord(character)
    return any(low <= code <= high for low, high in FORMAT_CHARACTER_RANGES)


def is_default_ignorable(character: str) -> bool:
    """True for a character in :data:`DEFAULT_IGNORABLE_RANGES`."""
    code = ord(character)
    return any(low <= code <= high for low, high in DEFAULT_IGNORABLE_RANGES)


def is_invisible(character: str) -> bool:
    """True for a character a reader cannot see.

    That is a format character, a default-ignorable code point, a blank character
    (:data:`BLANK_CHARACTERS`), or a non-ASCII ``White_Space`` character: a no-break space looks
    like a space, a line separator like a line break or nothing, a zero-width space, a byte-order
    mark, a variation selector, or a Hangul filler like nothing at all, and a bidirectional
    override reorders what follows it.
    """
    return (
        is_format_character(character)
        or is_default_ignorable(character)
        or character in BLANK_CHARACTERS
        or (character in UNICODE_WHITESPACE and ord(character) > 0x7F)
    )


def _character_class(ranges: list[tuple[int, int]]) -> str:
    return "".join(
        re.escape(chr(low)) if low == high else f"{re.escape(chr(low))}-{re.escape(chr(high))}"
        for low, high in ranges
    )


def name_violation(value: str) -> str | None:
    """Describe the first character a name may not contain, or return ``None``.

    A name -- an agent, a workspace, an allowed program, a denied option -- is compared with what
    somebody typed and shown to somebody reading, so it may contain no terminal control (C0, DEL,
    C1), no surrogate, and no invisible character (:func:`is_invisible`).
    """
    for character in value:
        code = ord(character)
        if code < 0x20 or 0x7F <= code <= 0x9F:
            return f"control character U+{code:04X}"
        if 0xD800 <= code <= 0xDFFF:
            return f"surrogate U+{code:04X}"
        if is_invisible(character):
            return f"invisible character U+{code:04X}"
    return None


def display(value: str) -> str:
    """Show ``value`` in a message as it is, or :func:`quote` it when it holds an unsafe character.

    A path or a name that is made of ordinary characters reads best unquoted; one that holds a
    terminal control, a surrogate, or an invisible character is quoted, so nothing can reach a
    terminal raw from a herdr-run message.
    """
    return value if name_violation(value) is None else quote(value)


def quote(value: str) -> str:
    """Quote ``value`` for a diagnostic, escaping everything that is not printable ASCII.

    Printable ASCII (U+0020..U+007E) passes through except ``"`` and ``\\``, which are escaped;
    newline, carriage return, and tab use their short escapes; every other code point is written
    as ``\\u{hex}``. No Unicode table is consulted, so a bidirectional override, a line separator,
    or an invisible character can never reach a terminal raw from a herdr-run message.
    """
    out = ['"']
    for character in value:
        code = ord(character)
        if character == '"':
            out.append('\\"')
        elif character == "\\":
            out.append("\\\\")
        elif character == "\n":
            out.append("\\n")
        elif character == "\r":
            out.append("\\r")
        elif character == "\t":
            out.append("\\t")
        elif 0x20 <= code <= 0x7E:
            out.append(character)
        else:
            out.append(f"\\u{{{code:x}}}")
    out.append('"')
    return "".join(out)


def quote_optional(value: str | None) -> str:
    """:func:`quote` a value that may be absent; an absent value reads ``None``."""
    return "None" if value is None else quote(value)


def os_error_text(exc: BaseException) -> str:
    """Describe an operating-system error as ``<strerror> (os error <errno>)``.

    That is the form the native edition's standard library prints, and it is easy to reproduce
    exactly here; reproducing Python's ``[Errno N] text: 'path'`` there would mean imitating
    ``repr`` of a path. An error without an errno falls back to its message.
    """
    errno = getattr(exc, "errno", None)
    strerror = getattr(exc, "strerror", None)
    if isinstance(errno, int) and isinstance(strerror, str) and strerror:
        return f"{strerror} (os error {errno})"
    return str(exc)


def strip_ascii(value: str) -> str:
    """Trim the six ASCII whitespace characters from both ends, and nothing else."""
    return value.strip(ASCII_WHITESPACE)


def strip_space(value: str) -> str:
    """Trim Unicode ``White_Space`` (the explicit list above) from both ends."""
    return value.strip(_UNICODE_WHITESPACE_TEXT)


def rstrip_space(value: str) -> str:
    """Trim Unicode ``White_Space`` (the explicit list above) from the end."""
    return value.rstrip(_UNICODE_WHITESPACE_TEXT)


def is_blank(value: str) -> bool:
    """True when ``value`` is empty or holds only Unicode ``White_Space``."""
    return all(character in UNICODE_WHITESPACE for character in value)


def number_text(value: float) -> str:
    """Render seconds for a message: integral values with no decimals, otherwise the shortest
    round-trip decimal, never in exponent notation (``0.00001``, not ``1e-05``)."""
    if math.isfinite(value) and value == math.floor(value):
        return f"{value:.0f}"
    if not math.isfinite(value):
        return repr(value)
    return format(Decimal(repr(value)), "f")


def format_round3(value: float) -> float:
    """Round to three decimal places by correctly rounded decimal formatting."""
    return float(f"{value:.3f}")


_EXIT_MARKER = re.compile(r"[+-]?[0-9]+")

#: Longest completion marker accepted. A real one is a few bytes; anything longer is not one.
MAX_EXIT_MARKER_BYTES = 128


def parse_exit_marker(payload: bytes) -> int | None:
    """Parse a run's ``exit_code`` completion marker, or return ``None`` when it is not one.

    ASCII bytes only, at most 128 of them, ASCII whitespace trimmed, then ``[+-]?[0-9]+`` within
    the signed 32-bit range. Anything else -- underscores, other scripts' digits, a byte that is
    not ASCII, a value out of range -- means "not a completed marker", never a crash and never a
    deletion the other edition would not make.
    """
    if len(payload) > MAX_EXIT_MARKER_BYTES or any(byte >= 0x80 for byte in payload):
        return None
    text = strip_ascii(payload.decode("ascii"))
    if not _EXIT_MARKER.fullmatch(text):
        return None
    value = int(text, 10)
    if not -(2**31) <= value <= 2**31 - 1:
        return None
    return value


def _reject_constant(token: str) -> object:
    raise ValueError(f"{token} is not valid JSON")


def _parse_float(token: str) -> float:
    value = float(token)
    if not math.isfinite(value):
        raise ValueError("number out of range")
    return value


def _parse_int(token: str) -> object:
    # Mirror a strict JSON reader with 64-bit integers: an integer literal outside the u64/i64
    # range, and ``-0``, are numbers but not integers.
    if token == "-0":
        return -0.0
    value = int(token)
    if -(2**63) <= value <= 2**64 - 1:
        return value
    try:
        as_float = float(value)
    except OverflowError:
        raise ValueError("number out of range") from None
    if not math.isfinite(as_float):
        raise ValueError("number out of range")
    return as_float


def _reject_surrogates(value: object) -> None:
    stack = [value]
    while stack:
        current = stack.pop()
        if isinstance(current, str):
            if any(0xD800 <= ord(character) <= 0xDFFF for character in current):
                raise ValueError("lone surrogate in JSON string")
        elif isinstance(current, list):
            stack.extend(current)
        elif isinstance(current, dict):
            stack.extend(current.keys())
            stack.extend(current.values())


#: Deepest JSON nesting refused: a document whose arrays and objects nest this many levels deep is
#: not read, so 127 levels is the most accepted. The native edition's reader has exactly this limit.
JSON_MAX_DEPTH = 128

#: A JSON string (closed or not) or one bracket, so nesting can be measured without parsing.
_JSON_DEPTH_TOKENS = re.compile(r'"[^"\\]*(?:\\.[^"\\]*)*"?|[\[\]{}]', re.DOTALL)


def _json_too_deep(text: str) -> bool:
    depth = 0
    for match in _JSON_DEPTH_TOKENS.finditer(text):
        token = match.group()
        if token in ("[", "{"):
            depth += 1
            if depth >= JSON_MAX_DEPTH:
                return True
        elif token in ("]", "}"):
            depth -= 1
    return False


def strict_json_loads(text: str) -> object:
    """Parse RFC 8259 JSON the way a strict 64-bit reader does, raising ``ValueError`` otherwise.

    ``NaN``/``Infinity`` are refused, a float that overflows is refused, an integer that does not
    fit 64 bits becomes the nearest float (and is refused when even that overflows), a lone
    surrogate escape is refused, and so is nesting :data:`JSON_MAX_DEPTH` levels deep. Every
    failure is a ``ValueError``; nothing else escapes.
    """
    if _json_too_deep(text):
        raise ValueError(f"JSON nested {JSON_MAX_DEPTH} or more levels deep")
    try:
        document: object = json.loads(
            text,
            parse_constant=_reject_constant,
            parse_float=_parse_float,
            parse_int=_parse_int,
        )
    except (OverflowError, RecursionError) as exc:
        raise ValueError(str(exc)) from exc
    _reject_surrogates(document)
    return document
