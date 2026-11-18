#!/usr/bin/env python3
"""Discuss (`catchup-side-agent`, slice 1) in a real Chromium.

INTEGRATOR: run this after the server's catchup route and the page land
together (`make -C vibe-talk discuss-browser` once that target is added, or
`python3 tests/discuss_browser.py` against a built release binary). It is
written to the agreed contract, not run in the panel slice: the catchup
route is answered at the network edge with a contract-shaped body (the model
is nobody's to run in a walk), and dismiss/restore are recorded and answered
there too; everything else — the real server, sign-in, the channel, the bar —
is the shipped page at a 393x852 phone (touch) and a 1280x800 desk. The
walk server has no [summaries.llm], so client-config is answered with
`discuss_available` added — the capability a real Discuss deployment
reports; the disabled-button state without it is covered by unit tests.

The walk: open the channel, tap Discuss (nothing is asked), press Summarize,
see "Reading…" give way to the summary with its covered-messages footer, Mark
read sending exactly the markable ids, a footer link jumping with the panel
still open (`#1395 discuss-scope-binding`, `#1345` keep-open), the
summary's opened section surviving close/reopen and a reload but not a
new Summarize (`#1497 discuss-fold-state-persist`), the summary reading
aloud through the shared owner — the bottom toolbar's Read (no top-bar
Read; `#1531 discuss-top-bar-slim-fixed`), tap-to-read from a block, the lit
block, and stop on close (`#1501 discuss-read-aloud-parity`, the
documents-read harness's fake device voice) — and
the panel overlaying the list without moving #scroll-area on a desk (on a
phone the half-height sheet is docked under the list, `#1526`). Condition waits
only — no fixed sleeps in any assertion.
"""

import io
import json
import math
import socket
import struct
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import wave
from pathlib import Path

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "discuss-check-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-check-write-token-not-a-real-credential"
MARKABLE = ["100000000000000101", "100000000000000102"]

CATCHUP_ANSWER = {
    "thread_id": None,
    "view": "flat",
    "window": {"count": 12, "unread_count": 2, "read_count": 10,
               "from_id": MARKABLE[0], "through_id": MARKABLE[1],
               "truncated": False, "noise_hidden": 0},
    "unread_ids": MARKABLE,
    "markable_ids": MARKABLE,
    "footer_links": [
        {"id": MARKABLE[0], "label": "09:51 · Ada", "thread_id": None},
        {"id": MARKABLE[1], "label": "09:52 · Ben", "thread_id": None},
        {"id": "100000000000000103", "label": "09:53 · Cy", "thread_id": None},
        {"id": "100000000000000104", "label": "09:54 · Di", "thread_id": None},
        {"id": "100000000000000105", "label": "09:55 · Ed", "thread_id": None},
        {"id": "100000000000000106", "label": "09:56 · Fi", "thread_id": None},
        {"id": "100000000000000107", "label": "09:57 · Gil", "thread_id": None},
        {"id": "100000000000000108", "label": "09:58 · Hal", "thread_id": None},
        {"id": "100000000000000109", "label": "09:59 · Ivy", "thread_id": None},
        {"id": "100000000000000110", "label": "10:00 · Jo", "thread_id": None},
    ],
    "text": "## Progress\nAda shipped the fix.\n\n## Review\nBen is reviewing it.",
    "text_html": "<h2>Progress</h2><ul><li>Ada shipped the fix "
                 "<a href=\"?c=100000000000000001&v=all&m=discord:100000000000000001:100000000000000101\">(09:51 ↩)</a></li></ul>"
                 "<h2>Review</h2><p>Ben is reviewing it.</p>",
    "fresh_ids": [],
    "backend": "fake",
    "model": "fake-model",
    "stamp": {"context_ms": 120, "model_ms": 900, "first_token_ms": None,
              "total_ms": 1050, "input_tokens": None, "output_tokens": None,
              "input_chars": 4200, "output_chars": 48},
    "turns": [],
}

# `#1501`: the documents-read harness's fake device voice (slow, so no block
# ends by itself while the walk steps it), recording every utterance.
DEVICE_VOICE = """(() => {
  const spoken = [];
  window.deviceSpeech = spoken;
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  const engine = {
    paused: false, speaking: false, pending: false,
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) {
      const entry = {text: String(utterance.text), started: null, ended: null, cancelled: false};
      spoken.push(entry);
      setTimeout(() => {
        if (entry.cancelled) return;
        entry.started = performance.now();
        if (utterance.onstart) utterance.onstart({});
        setTimeout(() => {
          if (entry.cancelled) return;
          entry.ended = performance.now();
          if (utterance.onend) utterance.onend({});
        }, 400 * entry.text.length);
      }, 30);
    },
    cancel() { for (const entry of spoken) if (entry.ended === null) entry.cancelled = true; },
    pause() { this.paused = true; },
    resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
})();"""


def tone_wav(seconds: float = 2.0, rate: int = 16000) -> bytes:
    """Unmistakable tone, as a cloud ticket's audio answers in the walk."""
    frames = b"".join(
        struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * i / rate)))
        for i in range(int(seconds * rate))
    )
    out = io.BytesIO()
    with wave.open(out, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(frames)
    return out.getvalue()


TONE = tone_wav()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def call(base: str, method: str, path: str, token: str,
         body: dict[str, object] | None = None) -> dict[str, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as res:
        parsed: object = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def walk(page: Page, base: str, width: int, height: int, scheme: str = "dark") -> None:
    """One Discuss walk at one size: open, read, mark, jump, close."""
    posts: list[dict[str, object]] = []
    writes: list[tuple[str, dict[str, object]]] = []
    # `#1497 discuss-fold-state-persist`: the saved record the reopen reads,
    # its generation advancing with each Summarize, as a new summary's would.
    generation: int | None = None
    updated = False

    def catchup(route: Route) -> None:
        nonlocal generation
        posts.append(route.request.post_data_json or {})
        generation = (generation or 1760000099999) + 1
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(CATCHUP_ANSWER))

    def catchup_update(route: Route) -> None:
        nonlocal updated
        updated = True
        body = dict(CATCHUP_ANSWER)
        body["text"] = str(CATCHUP_ANSWER["text"]) + "\n\n## Later\nBen approved it."
        body["text_html"] = str(CATCHUP_ANSWER["text_html"]) + (
            "<h2>Later</h2><ul><li>Ben approved it "
            "<a href=\"?c=100000000000000001&v=all&m=discord:100000000000000001:100000000000000102\">(09:52 ↩)</a></li></ul>")
        body["fresh_ids"] = [MARKABLE[1]]
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    def saved_record(route: Route) -> None:
        if generation is None:
            route.fulfill(status=200, content_type="application/json", body=json.dumps({
                "exists": False, "thread_id": None, "current_summary": None,
                "timeline": [], "staleness": None,
            }))
            return
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": True, "thread_id": None,
            "current_summary": {
                "backend": "fake", "model": "fake-model", "view": "flat",
                "since_ms": None, "created_ms": generation + (1 if updated else 0),
                "window": CATCHUP_ANSWER["window"],
                "covered_ids": MARKABLE, "markable_ids": MARKABLE,
                "cited_ids": [MARKABLE[1]] if updated else [],
                "fresh_ids": [MARKABLE[1]] if updated else [],
                "footer_links": CATCHUP_ANSWER["footer_links"],
                "text": str(CATCHUP_ANSWER["text"]) + ("\n\n## Later\nBen approved it." if updated else ""),
                "text_html": str(CATCHUP_ANSWER["text_html"]) + ((
                    "<h2>Later</h2><ul><li>Ben approved it "
                    "<a href=\"?c=100000000000000001&v=all&m=discord:100000000000000001:100000000000000102\">(09:52 ↩)</a></li></ul>") if updated else ""),
                "unknown_terms": [], "stamp": CATCHUP_ANSWER["stamp"],
            },
            "timeline": [],
            "staleness": {"new_unread": 1, "marked_read": 0} if not updated else None,
        }))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": 12, "unread_count": 2, "read_count": 10,
                       "from_id": MARKABLE[0], "through_id": MARKABLE[1],
                       "truncated": False, "noise_hidden": 0},
            "unread": [
                {"id": MARKABLE[0], "at_ms": 1760000000000, "label": "09:51 · Ada"},
                {"id": MARKABLE[1], "at_ms": 1760000060000, "label": "09:52 · Ben"},
            ],
        }))

    def dismiss(route: Route) -> None:
        writes.append((route.request.url, route.request.post_data_json or {}))
        route.fulfill(status=200, content_type="application/json", body="{}")

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    def open_discuss() -> None:
        # Already open (restored across a reload, `#1821`): nothing to open.
        if page.locator("#discuss-panel").is_visible():
            return
        # Reopen by whichever entry this build shows (`#1470` promotion):
        # the bar icon when promoted, the menu item when not.
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            page.click("#action-menu-open")
            page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
            page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", saved_record)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/update", catchup_update)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup", catchup)
    page.route(f"**/api/v1/channels/{CHANNEL}/dismiss", dismiss)
    page.route(f"**/api/v1/channels/{CHANNEL}/restore", dismiss)
    page.add_init_script(DEVICE_VOICE)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.goto(base + "/", wait_until="load")
    page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
    # To the channel view, which carries Discuss: the switch rotates through
    # the views, so press it until the channel picker (channel view only) is up.
    for _ in range(3):
        if not page.locator("#discord-channel").is_hidden():
            break
        page.click("#view-switch")
        try:
            page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
            break
        except Exception:
            continue
    page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)
    # The way in, whichever this build carries: the fixed bar icon (#1355)
    # where it stands visible, else the action menu's item.
    via_menu = page.locator("#discuss-open").is_hidden()
    if via_menu:
        page.wait_for_selector("#action-menu-open", state="visible", timeout=20_000)
    else:
        page.wait_for_selector("#discuss-open", state="visible", timeout=20_000)

    scroller_before = page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      const rect = area.getBoundingClientRect();
      return {top: rect.top, height: rect.height};
    }""")

    if via_menu:
        # Reopen by whichever entry this build shows (`#1470` promotion):
        # the bar icon when promoted, the menu item when not.
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            page.click("#action-menu-open")
            page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
            page.click("#discuss-menu-open")
    else:
        page.click("#discuss-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    # `#1531 discuss-top-bar-slim-fixed`: no Read in the top bar; its slim
    # fixed bar carries the scope title beside the layout/close buttons.
    check(page.locator("#discuss-read").count() == 0, "the top bar still carries a Read button")
    title_text = page.locator(".discuss-title").text_content() or ""
    check(title_text == "Discuss: main channel", f"the top bar title says {title_text!r}")
    close_box0 = page.locator("#discuss-close").bounding_box()
    title_box0 = page.locator(".discuss-title").bounding_box()
    check(close_box0 is not None and title_box0 is not None
          and title_box0["height"] <= close_box0["height"] + 1,
          f"the title is taller than its buttons: title={title_box0} close={close_box0}")
    if width < 900:
        check(close_box0 is not None and close_box0["height"] >= 44 and close_box0["width"] >= 44,
              f"the phone top-bar buttons are under 44px: {close_box0}")
    # `#1724 vibe-talk-discuss-panel-ignores-light-theme`: the whole panel —
    # its ground, its sticky head, its controls and its ink — takes the page's
    # scheme tokens, so in the light scheme it stands light with dark ink and
    # in the dark scheme dark with light ink, at 4.5:1 or better either way.
    # The tokens are resolved through a probe element, never restated here.
    theme = page.evaluate("""() => {
      const probe = document.createElement('div');
      document.body.appendChild(probe);
      const token = (name) => {
        probe.style.background = '';
        probe.style.background = `var(${name})`;
        return getComputedStyle(probe).backgroundColor;
      };
      const panel = document.getElementById('discuss-panel');
      const head = document.querySelector('.discuss-head');
      const btn = document.getElementById('discuss-summarize');
      const out = {
        panelBg: getComputedStyle(panel).backgroundColor,
        panelInk: getComputedStyle(panel).color,
        headBg: getComputedStyle(head).backgroundColor,
        btnBg: getComputedStyle(btn).backgroundColor,
        tokenPanel: token('--panel'), tokenInk: token('--ink'),
        tokenRowHover: token('--row-hover'),
      };
      probe.remove();
      return out;
    }""")

    def rgb(value: str) -> tuple[float, float, float]:
        parts = value.removeprefix("rgb(").removeprefix("rgba(").rstrip(")").split(",")
        return float(parts[0]), float(parts[1]), float(parts[2])

    def luminance(value: str) -> float:
        def lin(channel: float) -> float:
            c = channel / 255
            return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
        r, g, b = rgb(value)
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)

    def contrast(a: str, b: str) -> float:
        la, lb = luminance(a), luminance(b)
        hi, lo = max(la, lb), min(la, lb)
        return (hi + 0.05) / (lo + 0.05)

    check(theme["panelBg"] == theme["tokenPanel"],
          f"the panel ground is not the scheme's --panel: {theme}")
    check(theme["headBg"] == theme["tokenPanel"],
          f"the panel head is not the scheme's --panel: {theme}")
    check(theme["panelInk"] == theme["tokenInk"],
          f"the panel ink is not the scheme's --ink: {theme}")
    check(theme["btnBg"] == theme["tokenRowHover"],
          f"the Summarize control is not the scheme's --row-hover: {theme}")
    check(contrast(theme["panelInk"], theme["panelBg"]) >= 4.5,
          f"the panel ink does not read on its ground: {theme}")
    if scheme == "light":
        check(luminance(theme["panelBg"]) > 0.5 and luminance(theme["panelInk"]) < 0.05,
              f"the light-scheme panel did not stand light with dark ink: {theme}")
    else:
        check(luminance(theme["panelBg"]) < 0.05 and luminance(theme["panelInk"]) > 0.5,
              f"the dark-scheme panel did not stand dark with light ink: {theme}")
    # Opening asks nothing: the bound is offered first, and only its press posts.
    page.wait_for_function(
        "() => document.getElementById('discuss-summarize').textContent === 'Summarize'"
        " && document.getElementById('discuss-count').value === '2'"
        " && document.getElementById('discuss-since').value !== ''",
        timeout=20_000)
    check(posts == [], f"opening posted {posts}, not nothing")
    # `#1524 discuss-bound-hours-control`: the hours end stands beside N, the
    # bound is one row, and "since HH:MM" is dropped whole only when the head
    # cannot hold it (phone); editing K moves N and the time. Condition waits.
    check(page.locator("#discuss-hours").is_visible(), "the hours control is missing")
    check(page.locator("#discuss-hours").input_value() != "", "the hours control is empty")
    bound_row = page.evaluate("""() => {
      const c = (id) => { const r = document.getElementById(id).getBoundingClientRect(); return r.top + r.height / 2; };
      const sincePart = document.querySelector('.discuss-since-part');
      const sinceStyle = sincePart ? getComputedStyle(sincePart) : null;
      const hoursRect = document.getElementById('discuss-hours').getBoundingClientRect();
      return {
        countMid: c('discuss-count'), hoursMid: c('discuss-hours'),
        sinceMid: c('discuss-since'), buttonMid: c('discuss-summarize'),
        sinceHidden: !sincePart || sinceStyle.display === 'none',
        hoursWidth: hoursRect.width,
      };
    }""")
    check(abs(bound_row["countMid"] - bound_row["hoursMid"]) <= 3
          and abs(bound_row["countMid"] - bound_row["buttonMid"]) <= 3,
          f"the bound is not one row: {bound_row}")
    check(bound_row["hoursWidth"] > 0, f"the hours control was dropped: {bound_row}")
    if width < 900:
        check(bound_row["sinceHidden"], f"since HH:MM did not drop on the phone: {bound_row}")
    else:
        check(not bound_row["sinceHidden"] and abs(bound_row["sinceMid"] - bound_row["countMid"]) <= 3,
              f"since HH:MM did not stand on the desk row: {bound_row}")
    # `#1817 vibe-talk-summarize-inputs-free-typing`: typing "1" in hours
    # stays "1" (the old feedback loop rewrote it), the count follows as
    # the (here empty) range's 0, and typing past the old 99 cap is kept.
    page.fill("#discuss-hours", "1")
    page.wait_for_function(
        "() => document.getElementById('discuss-hours').value === '1'"
        " && document.getElementById('discuss-count').value === '0'",
        timeout=20_000)
    page.fill("#discuss-hours", "150")
    page.wait_for_function(
        "() => document.getElementById('discuss-hours').value === '150'",
        timeout=20_000)
    page.fill("#discuss-count", "2")
    page.wait_for_function(
        "() => document.getElementById('discuss-count').value === '2'"
        " && document.getElementById('discuss-hours').value !== ''"
        " && document.getElementById('discuss-hours').value.includes('.')",
        timeout=20_000)
    # `#1527 discuss-summarize-bottom-bar-mobile`: on a phone the ask row is a
    # bar at the pane's bottom with Summarize rightmost; on a desk it stays
    # at the top in its one row. Condition waits only.
    boxes = page.evaluate("""() => {
      const r = (s) => { const b = document.querySelector(s).getBoundingClientRect();
        return {top: b.top, bottom: b.bottom, left: b.left, right: b.right, height: b.height}; };
      return {panel: r('#discuss-panel'), ask: r('.discuss-ask'), head: r('.discuss-head'),
              bound: r('.discuss-bound'), btn: r('#discuss-summarize')};
    }""")
    if width < 900:
        check(abs(boxes["ask"]["bottom"] - boxes["panel"]["bottom"]) <= 2,
              f"phone ask bar is not at the pane bottom: {boxes}")
        check(boxes["btn"]["right"] > boxes["bound"]["right"]
              and boxes["btn"]["right"] <= boxes["panel"]["right"] + 1
              and boxes["btn"]["left"] >= boxes["panel"]["left"],
              f"phone Summarize is not the rightmost visible control: {boxes}")
        check(abs(boxes["btn"]["top"] - boxes["bound"]["top"]) <= 4
              and boxes["ask"]["height"] <= 64,
              f"phone ask bar is not one row: {boxes}")
    else:
        check(boxes["ask"]["top"] - boxes["panel"]["top"] <= 48,
              f"desktop ask row left the pane top: {boxes}")
    # (the rendered answer is asserted after Summarize, below)
    page.click("#discuss-summarize")
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    check(len(posts) == 1 and posts[0].get("thread_id") is None
          and posts[0].get("view") in ("flat", "main")
          and posts[0].get("since_ms") == 1760000000000,
          f"the panel posted {posts}, not one bounded channel catch-up")
    heading = page.locator("#discuss-answer h2").first.text_content() or ""
    check(heading == "Progress", f"the answer did not render its heading: {heading!r}")
    # `#1525 fold-marker-on-heading-line`: the chevron shares the heading's
    # line at the start of a 44px row, the heading element stays inside the
    # summary, a long heading wraps beside the chevron, and tap plus Enter
    # still fold. Judged on drawn geometry only, at phone and desk alike.
    check(page.locator("#discuss-answer details > summary > h2").count() == 2,
          "a fold heading no longer stands inside its summary")
    fold_geo = page.evaluate("""() => {
      const details = document.querySelector('#discuss-answer details');
      const summary = details.querySelector('summary');
      const head = summary.querySelector('h2');
      const s = summary.getBoundingClientRect();
      const h = head.getBoundingClientRect();
      return {summaryTop: s.top, headTop: h.top, headLeft: h.left,
              summaryLeft: s.left, summaryHeight: s.height,
              chevron: getComputedStyle(summary, '::before').content};
    }""")
    check(abs(fold_geo["summaryTop"] - fold_geo["headTop"]) <= 12,
          f"the marker and heading stack on separate lines: {fold_geo}")
    check(fold_geo["headLeft"] > fold_geo["summaryLeft"] + 8,
          f"the heading does not start beside a marker column: {fold_geo}")
    check(fold_geo["summaryHeight"] >= 44,
          f"the fold row is under 44px tall: {fold_geo}")
    check("▸" in fold_geo["chevron"], f"the closed fold shows no chevron: {fold_geo}")
    wrap_geo = page.evaluate("""() => {
      const head = document.querySelectorAll('#discuss-answer details > summary > h2')[1];
      const before = head.textContent;
      head.textContent = 'Review of the overnight migration and everything that '
        + 'still needs a second careful look before any of it ships to anyone';
      const summary = head.closest('summary');
      const range = document.createRange();
      range.selectNodeContents(head);
      const lines = [...range.getClientRects()];
      const out = {lines: lines.length, firstLeft: lines[0]?.left ?? 0,
                   secondLeft: lines[1]?.left ?? 0, summaryLeft: summary.getBoundingClientRect().left};
      head.textContent = before;
      return out;
    }""")
    check(wrap_geo["lines"] >= 2, f"the long heading did not wrap: {wrap_geo}")
    check(abs(wrap_geo["secondLeft"] - wrap_geo["firstLeft"]) <= 2
          and wrap_geo["secondLeft"] > wrap_geo["summaryLeft"] + 8,
          f"the wrapped heading slides under the marker: {wrap_geo}")
    first_fold = page.locator("#discuss-answer details").first
    page.locator("#discuss-answer details > summary > h2").first.click()
    page.wait_for_function(
        "() => document.querySelector('#discuss-answer details').open", timeout=20_000)
    page.locator("#discuss-answer details > summary > h2").first.click()
    page.wait_for_function(
        "() => !document.querySelector('#discuss-answer details').open", timeout=20_000)
    page.evaluate("() => document.querySelector('#discuss-answer details > summary').focus()")
    page.keyboard.press("Enter")
    page.wait_for_function(
        "() => document.querySelector('#discuss-answer details').open", timeout=20_000)
    page.keyboard.press("Enter")
    page.wait_for_function(
        "() => !document.querySelector('#discuss-answer details').open", timeout=20_000)
    check(not first_fold.evaluate("(d) => d.open"), "the fold did not end closed")
    raw = page.locator("#discuss-answer").text_content() or ""
    check("##" not in raw, f"raw Markdown showed in the answer: {raw!r}")
    status = page.locator("#discuss-status").text_content() or ""
    check(status == "Read 2 unread of 12", f"the status says {status!r}")
    covers = page.locator("#discuss-covers").text_content() or ""
    check(covers == "Other messages processed:", f"the footer says {covers!r}")
    check(page.locator("#discuss-links .discuss-link").count() == 10, "the footer links are missing")
    check(page.locator("#discuss-links .discuss-link").first.text_content() == "09:51", "the footer links are not dense HH:MM")
    # `#1540 discuss-footer-inline-links`: the times read as one dense,
    # comma-separated line that wraps like text — measured layout, not
    # just text: many times share lines, the whole footer stays within
    # a few lines, and no link stretches full-width. Judged on drawn
    # geometry at phone and desk alike.
    footer_geo = page.evaluate("""() => {
      const links = [...document.querySelectorAll('#discuss-links .discuss-link')];
      const rects = links.map((el) => el.getBoundingClientRect());
      const list = document.getElementById('discuss-links').getBoundingClientRect();
      const covers = document.getElementById('discuss-covers').getBoundingClientRect();
      const first = rects[0];
      return {
        lines: new Set(rects.map((r) => Math.round(r.top))).size,
        count: rects.length,
        widest: Math.max(...rects.map((r) => r.width)),
        listWidth: list.width,
        shortest: Math.min(...rects.map((r) => r.height)),
        coversWithFirst: Math.abs(covers.top - first.top) <= 4
          || Math.abs(covers.bottom - first.bottom) <= 24,
        text: document.getElementById('discuss-footer').textContent,
      };
    }""")
    check(footer_geo["count"] == 10 and footer_geo["lines"] <= 3,
          f"the footer times do not wrap as dense text: {footer_geo}")
    check(footer_geo["lines"] < footer_geo["count"],
          f"the footer times do not share lines: {footer_geo}")
    check(footer_geo["widest"] < footer_geo["listWidth"] * 0.6,
          f"a footer time still stretches full-width: {footer_geo}")
    check(footer_geo["shortest"] >= 32,
          f"a footer time tap target is too short: {footer_geo}")
    check("09:51, 09:52" in footer_geo["text"],
          f"the footer times are not comma-separated: {footer_geo['text']!r}")
    check(footer_geo["coversWithFirst"],
          f"the footer label does not share the times' line: {footer_geo}")
    check(page.locator("#discuss-answer .discuss-struck").count() == 0,
          "a bullet stood struck before anything was read")
    stamp = page.locator("#discuss-stamp").text_content() or ""
    check("in 4200 chars" in stamp and "out 48 chars" in stamp, f"the stamp line says {stamp!r}")

    # `#1501 discuss-read-aloud-parity`: the summary reads through the shared
    # owner on the device voice. Read counts its four blocks on the strip,
    # Next steps it, closing stops it; an inactive tap on answer text then
    # starts nothing (`#1543`), and after Read a tap on the last block jumps
    # there. The block being read is lit. Condition waits only.
    def wait_read_count(want: str) -> None:
        page.wait_for_function(
            """(want) => {
              const strip = document.getElementById('read-steer');
              const count = document.getElementById('read-steer-count');
              return !!strip && !strip.hidden && !!count && count.textContent.trim() === want;
            }""", arg=want, timeout=20_000)

    def wait_read_stopped() -> None:
        page.wait_for_function(
            """() => {
              const owner = readAloudOwner();
              const strip = document.getElementById('read-steer');
              return owner.nowPlaying === null && owner.pendingRead === null && !!strip && strip.hidden
                && document.querySelectorAll('#discuss-answer [data-reading-block]').length === 0;
            }""", timeout=20_000)

    toggle = page.locator("#audio-source")
    check(toggle.is_visible(), "the audio source toggle is not offered with Discuss open")
    if toggle.get_attribute("aria-checked") == "true":
        page.click("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)
    # `#1531`: the bottom toolbar's Read starts the summary read now.
    check(page.locator("#read-aloud").is_visible(), "the toolbar Read is not offered with Discuss open")
    page.click("#read-aloud")
    wait_read_count("1 of 4")
    page.wait_for_function("() => window.deviceSpeech.length >= 1", timeout=20_000)
    said = page.evaluate("() => window.deviceSpeech.map((u) => u.text)")
    check(said == ["Progress"], f"Read said {said}, not the first block alone")
    page.wait_for_function(
        "() => document.querySelector('#discuss-answer [data-reading-block=\"current\"]')?.textContent.trim() === 'Progress'",
        timeout=20_000)
    owner_id = page.evaluate("() => readAloudOwner().nowPlaying?.id || null")
    check(isinstance(owner_id, str) and owner_id.startswith("doc:discuss:"),
          f"the playing read is not the Discuss read: {owner_id!r}")
    page.click("#read-next")
    wait_read_count("2 of 4")
    page.wait_for_function("() => window.deviceSpeech.length >= 2", timeout=20_000)
    said = page.evaluate("() => window.deviceSpeech.map((u) => u.text)")
    check(said[-1] == "Ada shipped the fix (09:51 ↩)", f"Next said {said}, not the second block")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    wait_read_stopped()
    open_discuss()
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    page.locator("#discuss-answer .discuss-collapse-bar button").first.click()
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discuss-answer details')].every((d) => d.open)",
        timeout=20_000)
    # `#1543`: inactive — a tap on the answer text starts no read.
    said_before = page.evaluate("() => window.deviceSpeech.length")
    page.locator("#discuss-answer p", has_text="Ben is reviewing it.").click()
    check(page.evaluate("() => readAloudOwner().nowPlaying === null && readAloudOwner().pendingRead === null"),
          "an inactive tap on the Discuss answer started a read")
    check(page.evaluate("() => window.deviceSpeech.length") == said_before,
          "an inactive tap on the Discuss answer spoke")
    # Active: Read starts it, then a tap on the last block jumps there.
    page.click("#read-aloud")
    wait_read_count("1 of 4")
    said_before = page.evaluate("() => window.deviceSpeech.length")
    page.locator("#discuss-answer p", has_text="Ben is reviewing it.").click()
    wait_read_count("4 of 4")
    page.wait_for_function(
        f"() => window.deviceSpeech.length > {said_before}", timeout=20_000)
    said = page.evaluate("() => window.deviceSpeech.map((u) => u.text)")
    check(said[-1] == "Ben is reviewing it.", f"the tap said {said}, not its own block")
    page.wait_for_function(
        "() => document.querySelector('#discuss-answer [data-reading-block=\"current\"]')?.textContent.trim() === 'Ben is reviewing it.'",
        timeout=20_000)
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    wait_read_stopped()
    open_discuss()
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    page.locator("#discuss-answer .discuss-collapse-bar button").nth(1).click()
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discuss-answer details')].every((d) => !d.open)",
        timeout=20_000)

    # `#1530 discuss-summary-cloud-read-404`: the cloud voice reads the
    # summary too. The prepare is answered at the network edge in this
    # walk's style (the real route is covered through the server in the
    # integration tests) with a ticket whose audio is a tone; what the
    # owner sees is the point — no `unknown_document` banner, the strip
    # counting the summary's blocks, and the same Discuss key prepared
    # again after close/reopen. Condition waits only.
    cloud_prepares: list[str] = []

    def speech_prepare(route: Route) -> None:
        cloud_prepares.append(route.request.url)
        blocks = [
            {"kind": kind, "spoken": True, "chars": 12, "element": at, "piece": 0, "pieces": 1}
            for at, kind in enumerate(["heading", "item", "heading", "paragraph"])
        ]
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "prepared": [{"message_id": "discuss", "url": "/api/v1/speech/check-ticket", "blocks": blocks}],
            "expires_in_seconds": 600,
        }))

    def speech_audio(route: Route) -> None:
        route.fulfill(status=200, content_type="audio/wav", body=TONE)

    page.route("**/api/v1/documents/*/speech/prepare", speech_prepare)
    page.route("**/api/v1/speech/check-ticket*", speech_audio)
    if page.locator("#audio-source").get_attribute("aria-checked") != "true":
        page.click("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'true'",
            timeout=20_000)
    page.click("#read-aloud")  # `#1531`: the toolbar Read, not a top-bar button
    wait_read_count("1 of 4")
    check(len(cloud_prepares) > 0 and "discuss%3A" in cloud_prepares[-1] and "%7C" in cloud_prepares[-1],
          f"the cloud read prepared {cloud_prepares}, not the Discuss key")
    banner = page.locator("#error").text_content() or ""
    check("unknown_document" not in banner and "not archived" not in banner,
          f"the cloud Discuss read showed the old banner: {banner!r}")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    wait_read_stopped()
    open_discuss()
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    page.click("#read-aloud")  # `#1531`: the toolbar Read, not a top-bar button
    wait_read_count("1 of 4")
    check(len(cloud_prepares) == 2 and urllib.parse.unquote(cloud_prepares[0].split("/documents/")[1].split("/speech")[0])
          == urllib.parse.unquote(cloud_prepares[1].split("/documents/")[1].split("/speech")[0]),
          f"close/reopen prepared a different key: {cloud_prepares}")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    wait_read_stopped()
    open_discuss()
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    page.unroute("**/api/v1/documents/*/speech/prepare", speech_prepare)
    page.unroute("**/api/v1/speech/check-ticket*", speech_audio)
    if page.locator("#audio-source").get_attribute("aria-checked") == "true":
        page.click("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)

    # `#1497 discuss-fold-state-persist`: the summary's sections start
    # collapsed; an opened one survives close/reopen and a reload, kept on
    # this device for the saved record only; a new Summarize starts
    # collapsed again. Condition waits only — no fixed sleeps.
    folds_open = "[...document.querySelectorAll('#discuss-answer details')].map((d) => d.open)"
    page.wait_for_function(f"() => JSON.stringify({folds_open}) === '[false,false]'", timeout=20_000)
    page.locator("#discuss-answer details").nth(1).locator("summary").click()
    page.wait_for_function(f"() => JSON.stringify({folds_open}) === '[false,true]'", timeout=20_000)
    # The toggle event (and so the keep) lands a task after the open flips.
    page.wait_for_function(
        "() => (localStorage.getItem('vibe-talk.voice.discuss-folds') || '').includes('\"Review\"')",
        timeout=20_000)
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    open_discuss()
    page.wait_for_function(f"() => JSON.stringify({folds_open}) === '[false,true]'", timeout=20_000)
    # `#1821 vibe-talk-discuss-state-persists`: open stands across a reload
    # with the same scope, kept on this device; no reopen tap is needed.
    scope_before = page.locator("#discuss-scope").text_content() or ""
    page.wait_for_function(
        "() => { try { return JSON.parse(localStorage.getItem('vibe-talk.voice.discuss-panel') || 'null')?.open === true; } catch (e) { return false; } }",
        timeout=20_000)
    page.reload(wait_until="load")
    page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
    for _ in range(3):
        if not page.locator("#discord-channel").is_hidden():
            break
        page.click("#view-switch")
        try:
            page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
            break
        except Exception:
            continue
    page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    check((page.locator("#discuss-scope").text_content() or "") == scope_before,
          f"the restored scope differs: {scope_before!r} -> {page.locator('#discuss-scope').text_content()!r}")
    page.wait_for_function(f"() => JSON.stringify({folds_open}) === '[false,true]'", timeout=20_000)
    page.wait_for_function(
        "() => !document.getElementById('discuss-summarize').disabled", timeout=20_000)
    page.click("#discuss-summarize")
    page.wait_for_function(f"() => JSON.stringify({folds_open}) === '[false,false]'", timeout=20_000)

    panel_box = page.locator("#discuss-panel").bounding_box()
    check(panel_box is not None and 0 <= panel_box["x"] and panel_box["x"] + panel_box["width"] <= width,
          f"the panel spills off the {width}x{height} viewport: {panel_box}")
    scroller_after = page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      const rect = area.getBoundingClientRect();
      return {top: rect.top, height: rect.height};
    }""")
    if width >= 900:
        check(scroller_after == scroller_before,
              f"opening Discuss moved #scroll-area: {scroller_before} -> {scroller_after}")
    else:
        # `#1526 discuss-docked-pane-mobile`: on a phone the half-height
        # sheet is docked, so the list ends at the sheet's top edge instead
        # of running underneath it; the sheet's top is the scroller's bottom.
        panel_top = page.evaluate(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().top")
        check(scroller_after["height"] < scroller_before["height"]
              and abs(scroller_after["top"] + scroller_after["height"] - panel_top) <= 2,
              f"the phone sheet is not docked under the list: "
              f"scroller {scroller_before} -> {scroller_after}, panel top {panel_top}")

    # `#1344 discuss-panel-resize`: the kept layout, at this size. A desk gets the
    # wider default, drag-resizable, kept per device; a phone gets a full-width
    # sheet at half height with messages above, Maximize to full, sticking.
    if width >= 900:
        check(panel_box is not None and abs(panel_box["width"] - 560) <= 2,
              f"the desk overlay is not at the wider default: {panel_box}")
        handle = page.locator("#discuss-resize").bounding_box()
        if handle is None:
            raise AssertionError("the desk panel has no resize edge")
        cx = handle["x"] + handle["width"] / 2
        cy = handle["y"] + handle["height"] / 2
        page.mouse.move(cx, cy)
        page.mouse.down()
        page.mouse.move(cx - 120, cy, steps=5)
        page.mouse.up()
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().width >= 660",
            timeout=20_000)
        kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-width')")
        check(kept is not None and int(kept) >= 660, f"the dragged width was not kept: {kept!r}")
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        page.click("#discuss-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
            timeout=20_000)
        reopened = page.locator("#discuss-panel").bounding_box()
        check(reopened is not None and reopened["width"] >= 660,
              f"the kept width did not survive close/reopen: {reopened}")
        # `#1354 discuss-layout-modes`: docked beside the list, maximized over
        # the main area, the choice kept per device.
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        docked = page.locator("#discuss-panel").bounding_box()
        list_box = page.locator("#scroll-area").bounding_box()
        check(docked is not None and list_box is not None
              and abs(docked["x"] - (list_box["x"] + list_box["width"])) <= 4
              and docked["width"] >= 640,
              f"docked is not a column beside the list: panel={docked} list={list_box}")
        mode_kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-mode')")
        check(mode_kept == "docked", f"the docked mode was not kept: {mode_kept!r}")
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        # Docked then closed reserves no column (#1397): the screen loses
        # the mark and the list takes the full width again; reopening docks.
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
        freed = page.locator("#scroll-area").bounding_box()
        check(freed is not None and list_box is not None and freed["width"] > list_box["width"],
              f"closing docked did not free the column: list={list_box} after={freed}")
        page.click("#discuss-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'maximized'",
            timeout=20_000)
        full = page.locator("#discuss-panel").bounding_box()
        check(full is not None and full["width"] >= width - 100,
              f"maximized does not fill the main area: {full} in {width}x{height}")
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
    else:
        check(page.locator("#discuss-dock-toggle").is_hidden(),
              "the phone sheet shows the desk dock toggle")
        check(panel_box is not None and panel_box["width"] >= width - 2,
              f"the phone sheet is not full width: {panel_box} in {width}x{height}")
        check(panel_box is not None and height * 0.4 <= panel_box["height"] <= height * 0.6,
              f"the phone sheet is not at half height: {panel_box} in {width}x{height}")
        check(panel_box is not None and panel_box["y"] >= height * 0.2,
              f"no messages show above the half sheet: {panel_box} in {width}x{height}")
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height >= innerHeight * 0.75",
            timeout=20_000)
        close_box = page.locator("#discuss-close").bounding_box()
        check(close_box is not None and close_box["y"] >= 0,
              f"the full sheet's header is off screen: {close_box}")
        kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-sheet-full')")
        check(kept == "1", f"the full height was not kept: {kept!r}")
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        # Reopen by whichever entry this build shows (`#1470` promotion):
        # the bar icon when promoted, the menu item when not.
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            page.click("#action-menu-open")
            page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
            page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height >= innerHeight * 0.75",
            timeout=20_000)
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height <= innerHeight * 0.6",
            timeout=20_000)

    # `#1386 discuss-mod8-toggle`: Mod+8 closes and reopens over this view.
    page.keyboard.press("Control+8")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    page.keyboard.press("Control+8")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)

    # `#1388` landing 2: the banner's Update appends its new section green.
    page.wait_for_selector("#discuss-stale", state="visible", timeout=20_000)
    page.click("#discuss-update")
    page.wait_for_function(
        "() => document.querySelectorAll('#discuss-answer details.discuss-fresh').length === 1"
        " && document.querySelectorAll('#discuss-answer li.discuss-fresh').length === 1",
        timeout=20_000)

    page.click("#discuss-mark-read")
    page.wait_for_function(
        "() => !document.getElementById('discuss-mark-read').disabled", timeout=20_000)
    check(bool(writes) and writes[-1][0].endswith("/dismiss") and writes[-1][1] == {"messages": MARKABLE},
          f"Mark read sent {writes}, not exactly the markable ids")

    # `#1388` landing 1, phone viewport included: with every cited message
    # read, its bullet and section stand struck through, the green gone.
    page.wait_for_function(
        "() => { const li = document.querySelector('#discuss-answer li');"
        " return li && li.classList.contains('discuss-struck')"
        " && getComputedStyle(li).textDecorationLine.includes('line-through'); }",
        timeout=20_000)
    page.wait_for_function(
        "() => document.querySelectorAll('#discuss-answer li.discuss-struck').length === 2"
        " && document.querySelectorAll('#discuss-answer details.discuss-struck-section').length === 2"
        " && document.querySelectorAll('#discuss-answer .discuss-fresh').length === 0",
        timeout=20_000)

    # A footer jump stays in its scope: the panel stays open (`#1395`).
    page.click("#discuss-links .discuss-link >> nth=0")
    # The jump keeps the panel open (it overlays the list it jumped).
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)

    # `#1531`: the top bar stays fixed while the pane's content scrolls.
    page.evaluate("""() => {
      const panel = document.getElementById('discuss-panel');
      const spacer = document.createElement('div');
      spacer.id = 'discuss-test-spacer';
      spacer.style.height = '1600px';
      panel.appendChild(spacer);
      panel.scrollTop = 800;
    }""")
    page.wait_for_function(
        """() => {
          const panel = document.getElementById('discuss-panel');
          const head = document.querySelector('.discuss-head');
          const close = document.getElementById('discuss-close');
          if (!panel || !head || !close) return false;
          const panelRect = panel.getBoundingClientRect();
          const headRect = head.getBoundingClientRect();
          const closeRect = close.getBoundingClientRect();
          return panel.scrollTop >= 100
            && Math.abs(headRect.top - panelRect.top) <= 2
            && closeRect.top >= panelRect.top && closeRect.bottom <= headRect.bottom + 1;
        }""", timeout=20_000)
    page.evaluate("() => document.getElementById('discuss-test-spacer')?.remove()")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)

    # `#1821`: closing persists as closed across a reload, then opening
    # persists as open with the same scope. Condition waits only.
    page.wait_for_function(
        "() => { try { return JSON.parse(localStorage.getItem('vibe-talk.voice.discuss-panel') || 'null')?.open === false; } catch (e) { return false; } }",
        timeout=20_000)

    def to_channel() -> None:
        page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
        for _ in range(3):
            if not page.locator("#discord-channel").is_hidden():
                break
            page.click("#view-switch")
            try:
                page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
                break
            except Exception:
                continue
        page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)

    page.reload(wait_until="load")
    to_channel()
    check(page.locator("#discuss-panel").is_hidden(), "a kept-closed panel restored open")
    open_discuss()
    page.wait_for_function(
        "() => { try { return JSON.parse(localStorage.getItem('vibe-talk.voice.discuss-panel') || 'null')?.open === true; } catch (e) { return false; } }",
        timeout=20_000)
    scope_open = page.locator("#discuss-scope").text_content() or ""
    page.reload(wait_until="load")
    to_channel()
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    check((page.locator("#discuss-scope").text_content() or "") == scope_open,
          "the reopened scope differs after the second reload")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="discuss-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "discuss-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
        )
        server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 30
            while True:
                try:
                    call(base, "GET", "/healthz", READ_TOKEN)
                    break
                except Exception:
                    check(time.time() < deadline, "the server did not come up")
                    time.sleep(0.25)
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ada shipped the fix."})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ben is reviewing it."})

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                # `#1724`: phone and desk each walk in both colour schemes;
                # the walk asserts the panel follows the scheme every time.
                phone = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True,
                    color_scheme="dark")
                walk(phone.new_page(), base, 393, 852, "dark")
                phone_small = browser.new_context(
                    viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True,
                    color_scheme="light")
                walk(phone_small.new_page(), base, 390, 844, "light")
                desk = browser.new_context(viewport={"width": 1280, "height": 800},
                                            color_scheme="dark")
                walk(desk.new_page(), base, 1280, 800, "dark")
                desk_light = browser.new_context(viewport={"width": 1280, "height": 800},
                                                 color_scheme="light")
                walk(desk_light.new_page(), base, 1280, 800, "light")
                browser.close()
            print("discuss browser: phone (393x852 dark, 390x844 light) and desk "
                  "(1280x800 dark and light) walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
