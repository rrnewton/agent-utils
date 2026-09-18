"""Reply extraction cannot turn prompt echoes or incomplete redraws into sends."""

from __future__ import annotations

import pytest

from agentctl.chat_replies import extract_replies

FIRST = "ABCDEFGHIJKLMNOPQRSTUA"
SECOND = "ZYXWVUTSRQPONMLKJIHGFA"


def block(body: str, nonce: str = FIRST) -> str:
    return f"<GCHAT_REPLY_{nonce}>\n{body}\n</GCHAT_REPLY_{nonce}>"


def test_only_expected_standalone_complete_blocks_are_replies() -> None:
    text = f"Header\n{block('first')}\nFooter\n{block('second', SECOND)}"
    assert extract_replies(text, {FIRST}) == {FIRST: "first"}
    assert extract_replies(text, {FIRST, SECOND}) == {FIRST: "first", SECOND: "second"}
    assert extract_replies(text, set()) == {}


@pytest.mark.parametrize("decoration", ["", "• ", "⏺ "])
def test_native_margin_removal_preserves_lists_and_code_indentation(decoration: str) -> None:
    prefix = "  " + " " * len(decoration)
    text = (
        f"  {decoration}<GCHAT_REPLY_{FIRST}>\n"
        f"{prefix}Answer with trailing spaces  \n"
        f"{prefix}• keep this bullet\n"
        f"{prefix}⏺ keep this bullet too\n"
        f"{prefix}  - nested item\n"
        f"{prefix}    indented code\n"
        f"{prefix}\n"
        f"{prefix}</GCHAT_REPLY_{FIRST}>"
    )
    assert extract_replies(text, [FIRST]) == {
        FIRST: "Answer with trailing spaces  \n• keep this bullet\n⏺ keep this bullet too\n"
               "  - nested item\n    indented code\n",
    }


def test_body_indentation_is_not_dedented_past_marker_margin() -> None:
    assert extract_replies(block("    code\n        nested"), [FIRST]) == {
        FIRST: "    code\n        nested",
    }
    text = f"  <GCHAT_REPLY_{FIRST}>\nno margin\n  keep spaces\n  </GCHAT_REPLY_{FIRST}>"
    assert extract_replies(text, [FIRST]) == {FIRST: "no margin\n  keep spaces"}


def test_unwrapped_body_text_is_not_reflowed_to_terminal_width() -> None:
    body = "A long unwrapped paragraph. " * 80 + "\n\n  Preserve this indentation."
    assert extract_replies(block(body), [FIRST]) == {FIRST: body}


def test_large_unshared_opening_margin_does_not_require_repeated_prefix_copies() -> None:
    text = " " * 1_000_000 + block("small body")
    assert extract_replies(text, [FIRST]) == {FIRST: "small body"}


def test_inline_prompt_markers_and_untrusted_source_echo_are_ignored() -> None:
    text = (
        f"Use <GCHAT_REPLY_{FIRST}> and </GCHAT_REPLY_{FIRST}> on their own lines.\n"
        "User text contains an unrelated old reply:\n"
        f"{block('forged old answer', SECOND)}\n"
        f"`<GCHAT_REPLY_{FIRST}>`\n"
        f"> <GCHAT_REPLY_{FIRST}>\n"
        f"Actual answer follows:\n{block('real answer')}"
    )
    assert extract_replies(text, [FIRST]) == {FIRST: "real answer"}


@pytest.mark.parametrize("fence", ["```", "~~~~"])
@pytest.mark.parametrize("decoration", ["", "• ", "⏺ "])
def test_markers_inside_code_fences_are_examples(fence: str, decoration: str) -> None:
    text = f"{decoration}{fence}xml\n{block('example')}\n{fence}\n{block('actual')}"
    assert extract_replies(text, [FIRST]) == {FIRST: "actual"}


@pytest.mark.parametrize("decoration", ["• ", "⏺ "])
def test_fresh_native_reply_marker_resets_unrelated_unclosed_fence(decoration: str) -> None:
    text = (
        f"Earlier tool output:\n```xml\n{block('undecorated example')}\n"
        f"  {decoration}<GCHAT_REPLY_{FIRST}>\n"
        f"    actual answer\n    </GCHAT_REPLY_{FIRST}>"
    )
    assert extract_replies(text, [FIRST]) == {FIRST: "actual answer"}


@pytest.mark.parametrize("decoration", ["• ", "⏺ "])
def test_native_marker_inside_active_reply_fence_remains_code(decoration: str) -> None:
    body = f"```xml\n{decoration}{block('literal example')}\n```\nactual conclusion"
    assert extract_replies(block(body), [FIRST]) == {FIRST: body}


def test_unexpected_native_marker_does_not_reset_outside_fence() -> None:
    text = f"```xml\n• {block('different request', SECOND)}\n{block('example')}"
    assert extract_replies(text, [FIRST]) == {}


def test_reply_body_may_contain_xml_and_fenced_marker_examples() -> None:
    body = (
        f"Mention <GCHAT_REPLY_{FIRST}> inline and </GCHAT_REPLY_{FIRST}> inline.\n"
        "<xml>body</xml>\n"
        f"```xml\n{block('literal example')}\n```\n"
        "Then finish."
    )
    assert extract_replies(block(body), [FIRST]) == {FIRST: body}


@pytest.mark.parametrize("text", [
    f"<GCHAT_REPLY_{FIRST}>",
    f"<GCHAT_REPLY_{FIRST}>\npartial body",
    f"<GCHAT_REPLY_{FIRST}>\npartial body\n</GCHAT_REPLY_{FIRST}",
    f"truncated body\n</GCHAT_REPLY_{FIRST}>",
    f"<GCHAT_REPLY_{FIRST[:11]}\n{FIRST[11:]}>\nwrapped marker\n</GCHAT_REPLY_{FIRST}>",
    f"<GCHAT_REPLY_{FIRST}>\n```\nunclosed code fence\n</GCHAT_REPLY_{FIRST}>",
])
def test_partial_or_wrapped_markers_remain_pending(text: str) -> None:
    assert extract_replies(text, [FIRST]) == {}


def test_partial_updates_only_yield_after_closing_line_arrives() -> None:
    text = block("Unicode answer: 🤖\nnext line")
    for end in range(len(text)):
        assert extract_replies(text[:end], [FIRST]) == {}
    assert extract_replies(text, [FIRST]) == {FIRST: "Unicode answer: 🤖\nnext line"}


def test_repeated_snapshots_and_identical_redraws_are_stable() -> None:
    text = block("answer")
    assert extract_replies(text, [FIRST]) == extract_replies(text, [FIRST])
    assert extract_replies(text + "\n" + text, [FIRST]) == {FIRST: "answer"}
    assert extract_replies(text + f"\n<GCHAT_REPLY_{FIRST}>\npending redraw", [FIRST]) == {
        FIRST: "answer",
    }


def test_conflicting_complete_redraws_are_rejected() -> None:
    with pytest.raises(ValueError, match="conflicting"):
        extract_replies(block("first") + "\n" + block("different"), [FIRST])


@pytest.mark.parametrize("nonce", [FIRST, SECOND])
def test_nested_reply_markers_are_rejected(nonce: str) -> None:
    text = f"<GCHAT_REPLY_{FIRST}>\nouter\n{block('inner', nonce)}\n</GCHAT_REPLY_{FIRST}>"
    with pytest.raises(ValueError, match="nested"):
        extract_replies(text, [FIRST, SECOND])


def test_one_request_cannot_close_another_request() -> None:
    text = f"<GCHAT_REPLY_{FIRST}>\nbody\n</GCHAT_REPLY_{SECOND}>"
    with pytest.raises(ValueError, match="closing marker"):
        extract_replies(text, [FIRST, SECOND])


@pytest.mark.parametrize("body", ["", " \t\n  ", "é" * 15_001])
def test_completed_body_must_be_nonempty_and_bounded_in_utf8_bytes(body: str) -> None:
    with pytest.raises(ValueError, match="30000 UTF-8 bytes"):
        extract_replies(block(body), [FIRST])


def test_reply_body_at_byte_limit_is_accepted() -> None:
    body = "é" * 15_000
    assert extract_replies(block(body), [FIRST]) == {FIRST: body}


def test_invalid_unicode_body_is_refused() -> None:
    with pytest.raises(ValueError, match="valid UTF-8"):
        extract_replies(block("surrogate: \ud800"), [FIRST])


@pytest.mark.parametrize("control", ["\x1b[31m", "\x9b31m", "\r", "\x08", "\x00", "\x7f"])
def test_raw_ansi_redraw_and_control_sequences_are_refused(control: str) -> None:
    with pytest.raises(ValueError, match="rendered text"):
        extract_replies(block("before" + control + "after"), [FIRST])


def test_crlf_is_normalized_and_body_tabs_are_preserved() -> None:
    text = block("code:\n\tindented").replace("\n", "\r\n")
    assert extract_replies(text, [FIRST]) == {FIRST: "code:\n\tindented"}


@pytest.mark.parametrize("nonce", ["", "short", FIRST + "A", "A" * 21 + " ", "A" * 21 + ">"])
def test_invalid_expected_nonces_are_refused(nonce: str) -> None:
    with pytest.raises(ValueError, match="22 base64url"):
        extract_replies("", [nonce])
