"""Text rules shared by both editions of herdr-run, written out rather than borrowed.

Everything in this module exists because the obvious language built-in gives an answer that
depends on the language, or on the version of the language's Unicode tables: ``repr()`` and
``str.isprintable`` follow the interpreter's Unicode database, ``str.strip`` treats the
information separators U+001C..U+001F as whitespace, ``int()`` accepts underscores and every
script's decimal digits, and ``json.loads`` accepts ``NaN``. Each function here states its rule
explicitly so the same input produces the same bytes on every interpreter, and the native edition
implements the identical rule in ``textfmt.rs``. The golden cases in
``rs/herdr-run/testdata/text_cases.json`` hold the two to it.
"""

from __future__ import annotations

import json
import math
import re
from decimal import Decimal

__all__ = [
    "ASCII_WHITESPACE",
    "UNICODE_WHITESPACE",
    "format_round3",
    "is_blank",
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

#: The whitespace an environment value is trimmed of: exactly the six ASCII characters POSIX
#: ``isspace`` names. Information separators and every non-ASCII space are kept, so a control
#: character is refused later rather than silently removed.
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
    as_float = float(value)
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


def strict_json_loads(text: str) -> object:
    """Parse RFC 8259 JSON the way a strict 64-bit reader does, raising ``ValueError`` otherwise.

    ``NaN``/``Infinity`` are refused, a float that overflows is refused, an integer that does not
    fit 64 bits becomes a float, and a lone surrogate escape is refused.
    """
    document: object = json.loads(
        text,
        parse_constant=_reject_constant,
        parse_float=_parse_float,
        parse_int=_parse_int,
    )
    _reject_surrogates(document)
    return document
