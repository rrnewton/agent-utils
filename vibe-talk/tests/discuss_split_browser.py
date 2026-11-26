#!/usr/bin/env python3
"""`#2263 vibe-talk-discuss-split-pane` in a real Chromium.

Owner request 374: in Discuss the current summary holds still while the
timeline (outline) scrolls on its own; the boundary between them moves
(mouse, touch, keys) and is kept per device; the timeline collapses to its
header row, which says when a new earlier summary arrived; the newest
follow-up pair stands open in its own Latest block above the composer, and
scrolls inside itself to its end (at most 30% of the panel).

The walk boots the real server on a loopback port (`--fake-discord`, a
throwaway store, `[summaries.llm]` pointed at a loopback model stub: the
model is the only thing stubbed). Every Discuss route the walk uses
(preview, record, Summarize, follow-up) is the server's own, with the
page's exact requests. One corner per size: a desk at 1280x800 with the
panel docked (the full-height column), a phone at 412x915 with the sheet
at full height for the boundary, then at half height with the keyboard
really up (viewport shrunk by 336px). Condition waits only; every "holds
still" is a rect.top compared before and after the act (under half a px).
"""

import argparse
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from playwright.sync_api import Page, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "discuss-split-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-split-write-token-not-a-real-credential"
KEYBOARD_PX = 336
HEIGHT_KEY = "vibe-talk.voice.discuss-timeline-height"
COLLAPSED_KEY = "vibe-talk.voice.discuss-timeline-collapsed"

Json = dict[str, object]


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def call(base: str, method: str, path: str, token: str, body: Json | None = None) -> Json:
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(base + path, data=data, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    if data:
        request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=15) as response:
        parsed: Json = json.loads(response.read() or b"null")
        return parsed


# Long enough that the summary overflows its pane and a follow-up answer overflows the
# Latest block's 30% cap, so both scrollers really scroll.
ANSWER = "\n".join(
    ["## Progress"]
    + [f"- Progress point {n}: the rollout step {n} is done and reviewed." for n in range(1, 9)]
    + ["", "## Review"]
    + [f"- Review point {n}: the reviewer checked part {n} of the change." for n in range(1, 9)]
    + ["", "Split walk answer end."]
)


class ModelStub(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        body = json.dumps({
            "content": [{"type": "text", "text": ANSWER}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 300, "output_tokens": 200},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args: object) -> None:
        pass


class Instance:
    """One throwaway real server with the model stubbed, and its base URL."""

    def __init__(self) -> None:
        self.model = ThreadingHTTPServer(("127.0.0.1", free_port()), ModelStub)
        self.model_port = int(self.model.server_address[1])
        threading.Thread(target=self.model.serve_forever, daemon=True).start()
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.tmp = tempfile.TemporaryDirectory(prefix="discuss-split-")
        work = Path(self.tmp.name)
        (work / "model-key").write_text("discuss-split-stub-key\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{self.port}"\n'
            '[discord]\nbot_token = "discuss-split-bot-token-never-sent"\n'
            "live_poll_seconds = 5\n"
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\nserve = true\npath = "{work}/cache.sqlite3"\n'
            f'[summaries.llm]\ndialect = "anthropic"\n'
            f'base_url = "http://127.0.0.1:{self.model_port}"\n'
            f'api_key_file = "{work}/model-key"\n')
        self.server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 60
        while True:
            try:
                call(self.base, "GET", "/healthz", READ_TOKEN)
                break
            except Exception:
                check(time.time() < deadline, "the walk server did not come up")
                time.sleep(0.25)
        now = int(time.time() * 1000)
        for n in range(8):
            call(self.base, "POST", f"/api/v1/channels/{CHANNEL}/fake-seed", WRITE_TOKEN,
                 {"author": "alice", "text": f"Split walk note {n}: rollout step {n}.",
                  "at_ms": now - (90 - n) * 60_000})

    def close(self) -> None:
        self.server.terminate()
        try:
            self.server.wait(timeout=10)
        except Exception:
            self.server.kill()
        self.model.shutdown()
        self.tmp.cleanup()


def top(page: Page, selector: str) -> float:
    value = page.evaluate(
        "(s) => { const el = document.querySelector(s); return el ? el.getBoundingClientRect().top : null; }", selector)
    check(value is not None, f"no {selector} on the page")
    return float(value)


def height(page: Page, element_id: str) -> float:
    return float(page.evaluate(f"document.getElementById({json.dumps(element_id)}).getBoundingClientRect().height"))


GEOMETRY = """() => {
  const r = (el) => { if (!el) return null; const b = el.getBoundingClientRect();
    return [Math.round(b.top), Math.round(b.bottom), el.scrollHeight, el.clientHeight]; };
  const out = {};
  for (const id of ['discuss-panel', 'discuss-current-section', 'discuss-timeline-section', 'discuss-timeline',
                    'discuss-latest', 'discuss-followup', 'discuss-send']) out[id] = r(document.getElementById(id));
  return out;
}"""

# How much of the Latest answer's body text shows: WHOLE lines of its text (list items and
# paragraphs; its headings and its "Discuss" label do not count) inside the block and below the
# pair's sticky header.
LATEST_LINES = """() => {
  const box = document.getElementById('discuss-latest');
  if (!box || box.hidden) return null;
  const b = box.getBoundingClientRect();
  const head = box.querySelector('.discuss-qa > summary').getBoundingClientRect();
  const agent = box.querySelector('.discuss-agent');
  const label = agent.querySelector('.discuss-turn-label');
  const top = Math.max(head.bottom, b.top);
  const bottom = b.bottom;
  const texts = [...agent.querySelectorAll('li, p')];
  let lines = 0;
  let lh = 0;
  for (const el of texts) {
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    let h = parseFloat(cs.lineHeight);
    if (!isFinite(h)) h = 1.2 * parseFloat(cs.fontSize);
    lh = lh || h;
    const shown = Math.min(r.bottom, bottom) - Math.max(r.top, top);
    if (shown > 0) lines += Math.floor((shown + 0.5) / h);
  }
  return { lines, lineHeight: lh, textBlocks: texts.length,
           labelOff: label.getBoundingClientRect().bottom <= head.bottom + 1,
           question: box.querySelector('.discuss-qa > summary').textContent };
}"""

# Whether the timeline's top row stands whole (no first line cut off at the pane's top).
TOP_ROW_WHOLE = """() => {
  const t = document.getElementById('discuss-timeline');
  const b = t.getBoundingClientRect();
  const row = [...t.children].find((r) => r.getBoundingClientRect().bottom > b.top + 1);
  return !row || row.getBoundingClientRect().top >= b.top - 1;
}"""

PANES_FIT = """() => {
  const panel = document.getElementById('discuss-panel');
  const p = panel.getBoundingClientRect();
  const inside = (id) => { const el = document.getElementById(id);
    if (!el || el.hidden || el.getClientRects().length === 0) return true;
    const r = el.getBoundingClientRect(); return r.top >= p.top - 1 && r.bottom <= p.bottom + 1; };
  return panel.scrollHeight <= panel.clientHeight + 1
    && ['discuss-current-section', 'discuss-timeline-section', 'discuss-latest', 'discuss-followup', 'discuss-send'].every(inside);
}"""


class Walk:
    def __init__(self, page: Page, instance: Instance, phone: bool, shots: Path | None, tag: str) -> None:
        self.page = page
        self.instance = instance
        self.phone = phone
        self.shots = shots
        self.tag = tag
        # The "holds still" evidence: each act's anchor text and rect.top before and after.
        self.sidecar: dict[str, object] = {"corner": tag}
        page.add_init_script(
            f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)});"
            " localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
        page.goto(instance.base + "/", wait_until="load")
        page.wait_for_selector("#view-switch", state="visible")
        for _ in range(3):
            if not page.locator("#discord-channel").is_hidden():
                break
            page.click("#view-switch")
            try:
                page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
                break
            except Exception:
                continue

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.tag}-{name}.png"))

    def write_sidecar(self) -> None:
        if self.shots is not None:
            (self.shots / f"{self.tag}-sidecar.json").write_text(json.dumps(self.sidecar, indent=1) + "\n")

    def fits(self, what: str) -> None:
        check(bool(self.page.evaluate(PANES_FIT)),
              f"{self.tag}: {what}: the panes, Latest or the composer left the panel, or the panel scrolls: "
              f"{json.dumps(self.page.evaluate(GEOMETRY))}")

    def open_discuss(self) -> None:
        page = self.page
        if page.locator("#discuss-panel").is_visible():
            return
        page.wait_for_function(
            "() => { const b = document.getElementById('discuss-open');"
            " return b && b.getAttribute('aria-disabled') !== 'true'; }")
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            if not page.locator("#discuss-menu-open").is_visible():
                page.click("#action-menu-open")
            page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible")
        page.wait_for_function("() => document.getElementById('discuss-scope').textContent.trim().length > 0")

    def summarize(self) -> None:
        page = self.page
        # Composing on a phone the Summarize bar stands aside; a reader taps out of the box
        # (the keyboard goes down) and the bar is back.
        page.evaluate("document.activeElement && document.activeElement.blur && document.activeElement.blur()")
        page.wait_for_function("() => document.querySelector('.discuss-ask').getBoundingClientRect().height > 0")
        olds = int(page.evaluate("document.querySelectorAll('#discuss-timeline .discuss-old').length"))
        had = bool(page.evaluate("document.getElementById('discuss-answer').textContent.includes('Progress point')"))
        if int(page.evaluate("Number(document.getElementById('discuss-count').value)")) == 0:
            page.fill("#discuss-count", "8")
        page.click("#discuss-summarize")
        if had:
            page.wait_for_function(
                f"() => document.querySelectorAll('#discuss-timeline .discuss-old').length > {olds}")
        else:
            page.wait_for_function(
                "() => document.getElementById('discuss-answer').textContent.includes('Progress point')")

    def ask(self, question: str) -> None:
        self.page.fill("#discuss-followup", question)
        self.page.click("#discuss-send")
        self.page.wait_for_function(
            f"() => !document.getElementById('discuss-latest').hidden"
            f" && document.getElementById('discuss-latest').textContent.includes({json.dumps(question)})")

    def drag_head(self, dy: float) -> None:
        """Drag the timeline's header row (the boundary) by `dy` px: a finger on the phone, the mouse on the desk."""
        box = self.page.locator("#discuss-timeline-head").bounding_box()
        check(box is not None, "the timeline header has no box")
        assert box is not None
        x = box["x"] + box["width"] * 0.6
        y = box["y"] + box["height"] / 2
        steps = 10
        if self.phone:
            cdp = self.page.context.new_cdp_session(self.page)
            cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
            for i in range(1, steps + 1):
                cdp.send("Input.dispatchTouchEvent",
                         {"type": "touchMove", "touchPoints": [{"x": x, "y": y + dy * i / steps}]})
            cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
            cdp.detach()
        else:
            self.page.mouse.move(x, y)
            self.page.mouse.down()
            for i in range(1, steps + 1):
                self.page.mouse.move(x, y + dy * i / steps)
            self.page.mouse.up()

    def held_top(self, held: dict[str, object]) -> float:
        value = self.page.evaluate(
            "(h) => { const rows = [...document.querySelectorAll('#discuss-answer li, #discuss-answer h2, #discuss-answer summary')];"
            " const row = rows[h.at]; return row && row.textContent.trim().slice(0, 60) === h.text ? row.getBoundingClientRect().top : null; }",
            held)
        check(value is not None, f"{self.tag}: the summary row being read is gone: {held}")
        return float(value)

    def latest_lines(self, what: str, question: str) -> None:
        """Liaison: at least two whole text lines of the answer's body show in Latest, the question
        stays in its header, and its "Discuss" label has scrolled off above."""
        state = self.page.evaluate(LATEST_LINES)
        check(state is not None, f"{self.tag}: {what}: no Latest pair")
        self.sidecar[f"latest_lines_{what.replace(' ', '_')}"] = state
        check(int(state["lines"]) >= 2, f"{self.tag}: {what}: under two whole text lines of the answer show in Latest: {state}")
        check(question in str(state["question"]), f"{self.tag}: {what}: the Latest header lost the question: {state}")
        check(bool(state["labelOff"]), f"{self.tag}: {what}: Latest did not land on the answer's first line: {state}")

    def kept(self) -> object:
        return self.page.evaluate(f"localStorage.getItem({json.dumps(HEIGHT_KEY)})")

    def boundary(self) -> None:
        """Drag, then the summary holds still while the timeline scrolls and grows."""
        page = self.page
        self.fits("the full-height panel")
        summary_top = top(page, "#discuss-current-heading")
        start = height(page, "discuss-timeline")
        self.drag_head(-120)
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getAttribute('data-timeline-size') === 'kept'")
        grown = height(page, "discuss-timeline")
        check(grown >= start + 100, f"{self.tag}: the drag did not move the boundary: {start} -> {grown}")
        dragged_top = top(page, "#discuss-current-heading")
        self.sidecar["drag"] = {"anchor": "Current summary heading", "top_before": summary_top, "top_after": dragged_top,
                                "timeline_before": start, "timeline_after": grown, "kept": self.kept()}
        check(abs(dragged_top - summary_top) < 0.5,
              f"{self.tag}: the summary moved under the boundary drag")
        # The kept height is the dragged one; the flex layout may round the pane a pixel or two.
        check(abs(float(str(self.kept())) - grown) <= 3,
              f"{self.tag}: the kept height {self.kept()} is not the pane's {grown}")
        self.fits("after the drag")
        self.shot("dragged")
        # Open every fold so the timeline really scrolls, then scroll the summary a little:
        # the reader is mid-summary, and nothing the timeline does may move that.
        page.click("#discuss-timeline-head .discuss-toggle")
        page.wait_for_function(
            "() => { const t = document.getElementById('discuss-timeline'); return t.scrollHeight > t.clientHeight + 40; }")
        page.evaluate("document.getElementById('discuss-current-section').scrollTop = 60")
        summary_scroll = page.evaluate("document.getElementById('discuss-current-section').scrollTop")
        # The row being read, found again by its place and words: a follow-up redraws the
        # summary from the saved record, so the element itself is a new one afterwards.
        held = page.evaluate(
            "(() => { const pane = document.getElementById('discuss-current-section'); const p = pane.getBoundingClientRect();"
            " const rows = [...document.querySelectorAll('#discuss-answer li, #discuss-answer h2, #discuss-answer summary')];"
            " const at = rows.findIndex((r) => r.getBoundingClientRect().top >= p.top + 50); if (at < 0) return null;"
            " return { at, text: rows[at].textContent.trim().slice(0, 60), top: rows[at].getBoundingClientRect().top }; })()")
        check(held is not None, f"{self.tag}: no summary row in view to hold")
        reading = float(held["top"])
        scrolls: list[dict[str, object]] = []
        self.sidecar["timeline_scroll"] = {"anchor": held["text"], "top_before": reading, "after": scrolls}
        for where in ("middle", "end", "start"):
            page.evaluate(
                "(w) => { const t = document.getElementById('discuss-timeline');"
                " t.scrollTop = w === 'start' ? 0 : w === 'end' ? t.scrollHeight : (t.scrollHeight - t.clientHeight) / 2; }",
                where)
            page.wait_for_function(
                "(w) => { const t = document.getElementById('discuss-timeline'); const max = t.scrollHeight - t.clientHeight;"
                " return w === 'start' ? t.scrollTop === 0 : w === 'end' ? Math.abs(t.scrollTop - max) <= 1 : t.scrollTop > 0; }",
                arg=where)
            now_top = self.held_top(held)
            scrolls.append({"timeline_at": where, "top": now_top})
            check(abs(now_top - float(reading)) < 0.5,
                  f"{self.tag}: the summary row moved when the timeline scrolled to its {where}")
            check(page.evaluate("document.getElementById('discuss-current-section').scrollTop") == summary_scroll,
                  f"{self.tag}: the summary pane scrolled with the timeline")
        # The timeline grows (a follow-up lands; the old Latest pair joins its foot): the
        # summary row and the timeline row being read both keep their place. The question is
        # typed first, so the composer's focus (which sets the Summarize bar aside on a phone)
        # stands the same before and after: only the growth is measured.
        question = "What changed since then?"
        page.fill("#discuss-followup", question)
        page.wait_for_function("() => document.activeElement === document.getElementById('discuss-followup')")
        page.evaluate(
            "() => { const t = document.getElementById('discuss-timeline'); t.scrollTop = Math.max(1, (t.scrollHeight - t.clientHeight) / 2); }")
        page.wait_for_function("() => document.getElementById('discuss-timeline').scrollTop > 0")
        anchor = page.evaluate(
            "(() => { const t = document.getElementById('discuss-timeline'); const b = t.getBoundingClientRect();"
            " const row = [...t.children].find((r) => r.getBoundingClientRect().bottom > b.top + 1);"
            " return row ? { key: row.getAttribute('data-entry-key'), top: row.getBoundingClientRect().top } : null; })()")
        check(anchor is not None and anchor["key"], f"{self.tag}: no timeline row in view to hold")
        rows_before = int(page.evaluate("document.getElementById('discuss-timeline').children.length"))
        page.click("#discuss-send")
        page.wait_for_function(
            f"() => document.getElementById('discuss-latest').textContent.includes({json.dumps(question)})")
        page.wait_for_function(
            f"() => document.getElementById('discuss-timeline').children.length > {rows_before}")
        grew_top = self.held_top(held)
        check(abs(grew_top - float(reading)) < 0.5,
              f"{self.tag}: the summary row moved when the timeline grew")
        moved = page.evaluate(
            "(k) => { const row = document.querySelector(`#discuss-timeline > [data-entry-key=\"${k}\"]`);"
            " return row ? row.getBoundingClientRect().top : null; }", anchor["key"])
        self.sidecar["timeline_grow"] = {"summary_anchor": held["text"], "summary_top_before": reading, "summary_top_after": grew_top,
                                         "timeline_anchor": anchor["key"], "timeline_top_before": anchor["top"], "timeline_top_after": moved}
        check(moved is not None and abs(float(moved) - float(anchor["top"])) < 0.5,
              f"{self.tag}: the timeline row being read moved when the timeline grew: {anchor} -> {moved}")
        self.fits("after the timeline grew")

    def keys(self) -> None:
        page = self.page
        page.focus("#discuss-split")
        before = height(page, "discuss-timeline")
        page.keyboard.press("ArrowDown")
        page.wait_for_function(f"() => document.getElementById('discuss-timeline').getBoundingClientRect().height < {before - 20}")
        page.keyboard.press("ArrowUp")
        page.wait_for_function(
            f"() => Math.abs(document.getElementById('discuss-timeline').getBoundingClientRect().height - {before}) <= 1")
        page.keyboard.press("Enter")
        page.wait_for_function(
            "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'true'"
            " && document.getElementById('discuss-timeline').getBoundingClientRect().height === 0")
        self.fits("collapsed from the keys")
        page.keyboard.press("Enter")
        page.wait_for_function(
            f"() => Math.abs(document.getElementById('discuss-timeline').getBoundingClientRect().height - {before}) <= 1")

    def collapse_and_hint(self) -> None:
        page = self.page
        page.click("#discuss-timeline-fold")
        page.wait_for_function(
            "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'true'")
        check(page.evaluate(f"localStorage.getItem({json.dumps(COLLAPSED_KEY)})") == "1",
              f"{self.tag}: the collapse was not kept on the device")
        check(page.locator("#discuss-timeline-new").is_hidden(), f"{self.tag}: a hint stood with nothing new")
        self.summarize()
        page.wait_for_function(
            "() => !document.getElementById('discuss-timeline-new').hidden"
            " && document.getElementById('discuss-timeline-new').textContent === '1 new summary'")
        check(page.evaluate("document.getElementById('discuss-timeline-section').getAttribute('data-collapsed')") == "true",
              f"{self.tag}: a new earlier summary opened the timeline")
        check(page.locator("#discuss-latest").is_hidden(), f"{self.tag}: Latest stood after a summary")
        self.ask("Is anything blocked?")
        self.fits("collapsed with a Latest pair")
        self.shot("collapsed-hint")
        # Reload: the fold and the height are the device's; the hint is this session's.
        kept = self.kept()
        page.reload(wait_until="load")
        self.open_discuss()
        page.wait_for_function(
            "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'true'"
            " && !document.getElementById('discuss-latest').hidden")
        check(self.kept() == kept and page.evaluate(
            "document.getElementById('discuss-panel').style.getPropertyValue('--discuss-timeline-height')") == f"{kept}px",
            f"{self.tag}: the kept height did not survive the reload")
        page.click("#discuss-timeline-fold")
        page.wait_for_function(
            "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'false'"
            " && document.getElementById('discuss-timeline-new').hidden")

    def latest_scrolls_to_end(self, what: str) -> None:
        """Liaison: the Latest cap (30%) lets a long answer scroll fully inside its block, no clipping."""
        page = self.page
        page.evaluate("(() => { const l = document.getElementById('discuss-latest'); l.scrollTop = l.scrollHeight; })()")
        page.wait_for_function(
            "() => { const l = document.getElementById('discuss-latest'); return Math.abs(l.scrollTop - (l.scrollHeight - l.clientHeight)) <= 1; }")
        end = page.evaluate(
            "(() => { const l = document.getElementById('discuss-latest'); const b = l.getBoundingClientRect();"
            " const turns = l.querySelectorAll('.discuss-turn'); const last = turns[turns.length - 1];"
            " const r = last.getBoundingClientRect();"
            " return { overflow: l.scrollHeight > l.clientHeight + 1, top: b.top, bottom: b.bottom, lastBottom: r.bottom,"
            "  text: last.textContent.includes('Split walk answer end.') }; })()")
        check(end["text"], f"{self.tag}: {what}: the Latest answer is not the stub's whole answer")
        check(end["lastBottom"] <= end["bottom"] + 1 and end["lastBottom"] > end["top"],
              f"{self.tag}: {what}: the Latest answer's end cannot be scrolled into its block: {end}")
        page.evaluate("document.getElementById('discuss-latest').scrollTop = 0")

    def composing_blur(self) -> None:
        """Liaison: the Summarize bar steps aside while composing on a phone and comes back on
        blur, and nothing jumps when it does: every summary row keeps its rect.top (dy 0)."""
        page = self.page
        # The owner's own use: the timeline folded small, so the summary has the room.
        if page.evaluate("document.getElementById('discuss-timeline-section').getAttribute('data-collapsed')") != "true":
            page.click("#discuss-timeline-fold")
            page.wait_for_function(
                "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'true'")
        page.evaluate("document.getElementById('discuss-current-section').scrollTop = 0")
        page.locator("#discuss-followup").click()
        page.wait_for_function(
            "() => document.activeElement === document.getElementById('discuss-followup')"
            " && document.querySelector('.discuss-ask').getBoundingClientRect().height === 0")
        rows = page.evaluate(
            "(() => { const pane = document.getElementById('discuss-current-section').getBoundingClientRect();"
            " return [...document.querySelectorAll('#discuss-current-heading, #discuss-answer > *')]"
            "  .map((r) => r.getBoundingClientRect()).filter((b) => b.bottom > pane.top && b.top < pane.bottom)"
            "  .map((b) => b.top); })()")
        check(len(rows) >= 2, f"{self.tag}: composing: too few summary rows in view to hold: {rows}")
        self.shot("composing")
        page.evaluate("document.activeElement.blur()")
        page.wait_for_function("() => document.querySelector('.discuss-ask').getBoundingClientRect().height > 0")
        after = page.evaluate(
            "(n) => [...document.querySelectorAll('#discuss-current-heading, #discuss-answer > *')]"
            ".map((r) => r.getBoundingClientRect().top).slice(0, n)", len(rows))
        self.sidecar["blur"] = {"anchors": "Current summary heading and the summary's visible rows", "tops_composing": rows,
                                "tops_after_blur": after, "frames": ["phone-light-composing.png", "phone-light-after-blur.png"]}
        check(all(abs(a - b) < 0.5 for a, b in zip(rows, after)),
              f"{self.tag}: a summary row moved when the Summarize bar came back on blur: {rows} -> {after}")
        self.fits("after blur")
        self.shot("after-blur")

    def keyboard_open(self, question: str = "Is anything blocked?") -> None:
        page = self.page
        page.locator("#discuss-followup").click()
        page.wait_for_function("() => document.activeElement === document.getElementById('discuss-followup')")
        closed = int(page.evaluate("window.visualViewport.height"))
        page.set_viewport_size({"width": 412, "height": closed - KEYBOARD_PX})
        page.wait_for_function(f"() => window.visualViewport.height === {closed - KEYBOARD_PX}")
        page.evaluate("document.activeElement.scrollIntoView({block: 'nearest'})")
        self.fits("keyboard up")
        state = page.evaluate(
            "(() => { const vh = window.visualViewport.height; const p = document.getElementById('discuss-panel').getBoundingClientRect();"
            " const l = document.getElementById('discuss-latest').getBoundingClientRect();"
            " const f = document.getElementById('discuss-followup').getBoundingClientRect();"
            " const s = document.getElementById('discuss-send').getBoundingClientRect();"
            " return { latest: l.height, composerIn: f.bottom <= vh + 1 && s.bottom <= vh + 1 && f.top >= p.top,"
            "  askHidden: document.querySelector('.discuss-ask').getBoundingClientRect().height === 0 }; })()")
        check(state["latest"] >= 40, f"{self.tag}: keyboard up: the Latest pair has no room: {state}")
        self.latest_lines("keyboard up", question)
        check(state["composerIn"], f"{self.tag}: keyboard up: the composer is not whole in view: {state}")
        check(state["askHidden"], f"{self.tag}: keyboard up: the Summarize bar still takes the room: {state}")
        self.latest_scrolls_to_end("keyboard up")
        self.shot("keyboard-open")
        page.set_viewport_size({"width": 412, "height": closed})
        page.wait_for_function(f"() => window.visualViewport.height === {closed}")


def walk(page: Page, phone: bool, shots: Path | None, tag: str, whole: bool = True) -> None:
    instance = Instance()
    try:
        drive = Walk(page, instance, phone, shots, tag)
        drive.open_discuss()
        drive.summarize()
        drive.ask("What is the next step?")
        drive.summarize()
        drive.ask("Who reviews the fix?")
        drive.ask("When does it ship?")
        drive.fits("the default panel")
        if phone:
            # The phone's half sheet: a Latest pair stands, so the timeline is collapsed by
            # default (no choice kept) and the answer has the room.
            check(page.evaluate("document.getElementById('discuss-timeline-section').getAttribute('data-collapsed')") == "true"
                  and page.evaluate(f"localStorage.getItem({json.dumps(COLLAPSED_KEY)})") is None,
                  f"{tag}: the half sheet did not collapse the timeline by default beside a Latest pair")
            drive.latest_lines("half sheet default", "When does it ship?")
            if not whole:
                # The other theme's phone corner: the two-lines bar at the half-sheet default
                # (above) and with the keyboard up, nothing else again.
                drive.keyboard_open("When does it ship?")
                drive.write_sidecar()
                return
        check(int(page.evaluate("document.querySelectorAll('#discuss-timeline > details').length")) == 3,
              f"{tag}: the timeline does not hold the pair, the earlier summary and the pair before Latest")
        check(page.evaluate("document.querySelector('#discuss-latest > details').open"), f"{tag}: Latest is not open")
        drive.latest_scrolls_to_end("default")
        drive.shot("default")
        # The roomy layout for the boundary: the desk's docked column, the phone's full sheet,
        # where the timeline stands open by default with a whole row at its top.
        page.click("#discuss-dock-toggle" if not phone else "#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').getAttribute('data-discuss') === 'docked'" if not phone else
            "() => document.getElementById('discuss-panel').getAttribute('data-sheet') === 'full'")
        page.wait_for_function(
            "() => document.getElementById('discuss-timeline-section').getAttribute('data-collapsed') === 'false'")
        check(bool(page.evaluate(TOP_ROW_WHOLE)), f"{tag}: the timeline's top row stands with its first line cut off")
        drive.boundary()
        drive.keys()
        drive.collapse_and_hint()
        if phone:
            page.click("#discuss-full-toggle")
            page.wait_for_function("() => document.getElementById('discuss-panel').getAttribute('data-sheet') === 'half'")
            # The reader opened the timeline above: that choice wins on the half sheet too.
            check(page.evaluate("document.getElementById('discuss-timeline-section').getAttribute('data-collapsed')") == "false"
                  and page.evaluate(f"localStorage.getItem({json.dumps(COLLAPSED_KEY)})") == "0",
                  f"{tag}: the reader's open timeline did not stay open on the half sheet")
            drive.fits("the half sheet")
            drive.composing_blur()
            drive.keyboard_open()
        drive.write_sidecar()
    finally:
        instance.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None, help="directory to keep a PNG of each state in")
    args = parser.parse_args()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    corners = [("desktop-dark", 1280, 800, 1.0, False, "dark", True),
               ("phone-light", 412, 915, 2.6, True, "light", True),
               ("phone-dark", 412, 915, 2.6, True, "dark", False)]
    with sync_playwright() as pw:
        browser = pw.chromium.launch()
        for tag, width, height_px, scale, touch, scheme, whole in corners:
            context = browser.new_context(
                viewport={"width": width, "height": height_px}, device_scale_factor=scale,
                has_touch=touch, is_mobile=touch, color_scheme="dark" if scheme == "dark" else "light")
            browser_waits.hot_host_timeouts(context)
            walk(context.new_page(), touch, args.screenshots, tag, whole)
            context.close()
        browser.close()
    print("discuss split browser: held summary, drag, keys, collapse hint, reload, Latest to its end, blur holds, keyboard up passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
