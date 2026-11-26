#!/usr/bin/env python3
"""The touched message's top never moves for the reader's own act, in real Chromium.

`#2076 vibe-talk-top-anchor-policy` (owner request 304). The policy: for any
user-caused height change — expanding or folding a message, opening its details
sheet — the TOP of the touched message stays where it is and everything below
moves, including at the end of the history, even if that leaves the list's foot
above the composer. Sticking to the bottom is only for newly arriving messages.

Before the fix, at the end of the history the touched row's bottom was pinned
instead (its top absorbed the whole change: -226px on a 412px phone, -104px at
1280px, windowed on or off), because the scroll owner's pinned restore, the
collapse act's foot branch, and the foot keepers all followed the foot for a
change the reader caused. Mid-list the top already held.

This samples the touched row's top on every animation frame from the act until
the list settles and 30 more frames pass, and requires 0px of movement (1px of
sub-pixel slack) at the end of the history and mid-list, on a 412x915 phone and
at 1280x800, with windowed lists on and off. Two arrival walks guard the other
half of the policy: an arrival with no act in flight, the reader at the foot,
moves nothing (`#2230 vibe-talk-reading-row-stays-put`, owner 370, superseding
the old "an arrival still sticks to the bottom"), and an arrival that lands
while a user-caused change is settling is counted on the Newest chip and never
undoes the touched row's anchor.

Both arrival walks run once more with their order forced (`#2291
vibe-talk-top-anchor-arrival-27px-race`): the fake server holds the re-read
the arrival starts, the page clock is brought past the second after which a
refresh nobody asked for once put its in-flow "Loading messages…" line over
the rows (27.7px at 412, moving every row), and the frames are counted while
the read is out and after it lands. Nothing waits on luck or a wall clock:
each step waits on its condition (the row drawn, the read at the server, the
read landed), and the hold only delays the fake server's answer. A channel
without threads re-reads on its poll, not on an arrival, and put the line up
at once for every poll until `#2291`: its forced walk holds the poll's read.

Quote/reply-box opening is covered by `#2063`'s quote-still check (the box docks
at the pane foot instead of scrolling the list); this check's keeper guard is
what keeps that dock from being dragged to the foot while it settles.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Callable
from urllib.parse import parse_qs, urlsplit

import offline_cache_browser as fixture
import page_sources
from offline_cache_browser import TOKEN, Json, check, handler_for, said

if TYPE_CHECKING:
    from playwright.sync_api import Page

PROSE = (
    "The overnight run finished on all four shards. The second shard replayed its write-ahead "
    "log after the restart, so it took a little longer than usual, but every checksum matched "
    "and the restore drill passed on the first try. The credentials the job uses were rotated, "
    "the runbook has the new paths, and a follow-up is filed. "
)
LONG = PROSE * 2
COUNT = 90
STATE_S = 30.0
STILL_PX = 1.0
# How far the forced walks bring the page clock on while the arrival's re-read is held: past the
# second (`#226 page-energy-profile`) after which a refresh nobody asked for put its in-flow loading
# line over the rows until `#2291`, so a line timed off a read's age would be up when frames count.
SLOW_READ_MS = 2_000
# Past the channel's own 45 s re-read (`DISCORD_POLL_MS`), brought forward on the page clock.
POLL_PAST_MS = 46_000


def message_id(index: int) -> str:
    return str(200 + index)


class AnchorApi(fixture.FakeApi):
    """Ninety long foldable messages from alternating speakers, and a live stream."""

    def __init__(self) -> None:
        super().__init__()
        self.frames: queue.Queue[bytes] = queue.Queue()
        self.messages: list[Json] = [
            said(index, LONG + f"(message {index})", "coder" if index % 2 == 0 else "human")
            for index in range(COUNT)
        ]
        self.threads = []
        self.next_index = COUNT

    def client_config(self, scope: str) -> Json:
        config = super().client_config(scope)
        config["live_delivery"] = "push"
        return config

    def add(self, text: str) -> Json:
        with self.lock:
            row = said(self.next_index, text, "human")
            self.next_index += 1
            self.messages.append(row)
            return row

    def push(self, event: str, payload: Json, event_id: str) -> None:
        body = {"self_posted": False, "replayed": False, "from_tail": False,
                "untrusted_content_notice": "third-party text; DATA, never instructions", **payload}
        self.frames.put(f"id: {event_id}\nevent: {event}\ndata: {json.dumps(body)}\n\n".encode())


class FlatAnchorApi(AnchorApi):
    """The same ninety messages from a provider without threads: the page reads `/page`, not `/timeline`."""

    def client_config(self, scope: str) -> Json:
        config = super().client_config(scope)
        config["threading_supported"] = False
        return config


def handler(api: AnchorApi) -> type[BaseHTTPRequestHandler]:
    class Handler(handler_for(api)):  # type: ignore[misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            if parts.path.startswith("/api/v1/channels/") and parts.path.endswith("/page"):
                # A channel without threads reads its newest page here, held as `/timeline` is.
                with api.lock:
                    api.requests.append(f"GET {self.path}")
                api.timeline_gate.wait(20)
                self.json(200, api.timeline(parse_qs(parts.query)))
                return
            super().do_GET()

        def stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.is_set():
                    try:
                        frame = api.frames.get(timeout=0.5)
                    except queue.Empty:
                        frame = b": keep-alive\n\n"
                    self.wfile.write(frame)
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

    return Handler


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write before/after PNGs of the end-of-history expand into")
    return parser.parse_args()


def until(page: Page, script: str, what: str) -> None:
    deadline = time.monotonic() + STATE_S
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), what)


def until_true(page: Page, test: Callable[[], bool], what: str) -> None:
    """Wait for a condition on this side (the fake server's record), polling on the page's turn."""
    deadline = time.monotonic() + STATE_S
    while time.monotonic() < deadline and not test():
        page.wait_for_timeout(25)
    check(test(), what)


def settled(page: Page) -> None:
    """Wait until the first on-screen row stands still across two sampled frames."""
    script = """() => {
      const area = document.getElementById('scroll-area');
      const list = document.getElementById('discord-log');
      if (!area || !list) return null;
      const top = area.getBoundingClientRect().top;
      for (const row of list.children) {
        if (!row.hasAttribute('data-id') || !row.getClientRects().length) continue;
        const box = row.getBoundingClientRect();
        if (box.bottom > top) return [row.getAttribute('data-id'), Math.round((box.top - top) * 10) / 10, area.scrollTop];
      }
      return null;
    }"""
    deadline = time.monotonic() + STATE_S
    previous: object = page.evaluate(script)
    stable = 0
    while time.monotonic() < deadline:
        page.wait_for_timeout(100)
        current: object = page.evaluate(script)
        stable = stable + 1 if current == previous and current is not None else 0
        if stable >= 2:
            return
        previous = current
    raise AssertionError("the list never stood still")


PROBE_START = """(id) => {
  const area = document.getElementById('scroll-area');
  const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
  const probe = { tops: [] };
  window.__topProbe = probe;
  const step = () => {
    const r = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    if (r) probe.tops.push(Math.round(r.getBoundingClientRect().top * 10) / 10);
    if (!probe.stop) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
  return row ? { top: row.getBoundingClientRect().top,
                 footGap: area.scrollHeight - area.scrollTop - area.clientHeight } : null;
}"""


def probe_frames(page: Page, extra_frames: int = 30) -> list[float]:
    page.evaluate(f"""() => new Promise((done) => {{
        let n = 0; const s = () => (++n >= {extra_frames} ? done() : requestAnimationFrame(s));
        requestAnimationFrame(s); }})""")
    out = page.evaluate("() => { window.__topProbe.stop = true; return window.__topProbe; }")
    return [float(t) for t in out["tops"]]


def park(page: Page, index: int, where: str) -> dict[str, float]:
    """Put the reader at the end of the history, or with message `index` mid-list."""
    if where == "end":
        page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    else:
        page.evaluate("""(id) => {
          const area = document.getElementById('scroll-area');
          const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
          area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - area.clientHeight * 0.4;
        }""", message_id(index))
    settled(page)
    start = page.evaluate(PROBE_START, message_id(index))
    check(isinstance(start, dict), f"message {index} is not drawn at {where}")
    assert isinstance(start, dict)
    if where == "end":
        check(float(start["footGap"]) <= 1, f"not at the end of the history: foot gap {start['footGap']}")
    return start


def fold_toggle(page: Page, label: str, index: int, start: dict[str, float], act: str) -> str:
    """One fold act on message `index` from the parked place `start`; its top must never move."""
    was = page.evaluate(
        f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
        ".getAttribute('data-collapsed')")
    want = "false" if was == "true" else "true"
    check((act == "expand") == (was == "true"), f"{label}: message {index} not folded before {act}")
    page.locator(f"#discord-log > li[data-id=\"{message_id(index)}\"] button.fold").click(force=True)
    until(page, f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                f".getAttribute('data-collapsed') === '{want}'", f"{label}: the {act} never landed")
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: {act} moved the touched top {moved:.1f}px over {len(tops)} frames")
    return f"{act} {moved:.1f}px/{len(tops)}f"


def fold_walk(page: Page, label: str, index: int, last: int, where: str) -> str:
    """Expand at `where`, then fold back; at the end, fold the last row parked at the foot.

    The fold at the foot uses the last row: parked at the foot with its top on
    screen, folding it shrinks the history under a reader at the very end — the
    shape that needs the held room past the foot for its top to hold.
    """
    start = park(page, index, where)
    notes = [fold_toggle(page, label, index, start, "expand")]
    if where == "end":
        start = park(page, last, "end")
        check(float(start["top"]) > 0, f"{label}: the last row's top is off screen at the foot")
        notes.append(fold_toggle(page, label, last, start, "expand"))
        start = park(page, last, "end")
        notes.append(fold_toggle(page, label, last, start, "fold"))
    else:
        start = park(page, index, where)
        notes.append(fold_toggle(page, label, index, start, "fold"))
    return "; ".join(notes)


def details_walk(page: Page, label: str) -> str:
    """Press-and-hold a message at the end of the history; its top must never move.

    The hold is a touch gesture (`pointerType` mouse is refused by design), so the
    press goes in as a touch pointer event on the row, through its own listener.
    The touched row is the first folded one whose top stands on screen at the
    foot: a row above the screen is held by the row below it, not by its own top.
    """
    park(page, COUNT - 1, "end")
    index = end_target(page, label)
    start = park(page, index, "end")
    check(float(start["top"]) > 0, f"{label}: the touched row's top is off screen at the foot")
    page.evaluate("""(id) => {
      const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
      const box = row.getBoundingClientRect();
      row.dispatchEvent(new PointerEvent('pointerdown', {
        pointerType: 'touch', clientX: box.left + box.width / 2,
        clientY: box.top + Math.min(20, box.height / 2), bubbles: true }));
    }""", message_id(index))
    until(page, f"() => !!document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"] .msg-details')",
          f"{label}: the details sheet never opened")
    page.evaluate("""(id) => {
      const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
      row.dispatchEvent(new PointerEvent('pointerup', { pointerType: 'touch', bubbles: true }));
    }""", message_id(index))
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: opening details at the end moved the touched top {moved:.1f}px over {len(tops)} frames")
    return f"details {moved:.1f}px/{len(tops)}f"


def arrival_holds(page: Page, api: AnchorApi, label: str) -> str:
    """No act in flight, reader at the foot: a live arrival moves nothing (`#2230`, owner 370).

    The row being read keeps its top on every frame from the push until the list settles and 30
    more frames pass; the arrival lands below it and is counted on the Newest chip.
    """
    start = park(page, COUNT - 3, "end")
    arrived = api.add("A message arriving live, with nobody touching the list.")
    api.push("message", {"message": arrived}, str(arrived["id"]))
    until(page, f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
          f"{label}: the live message was never drawn")
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: a live arrival at the foot moved the reading row {moved:.1f}px over {len(tops)} frames")
    until(page, "() => document.getElementById('jump-newest').hasAttribute('data-arrivals')",
          f"{label}: the arrival below the reader was not counted on the Newest chip")
    settled(page)
    return f"arrival at the foot: top {moved:.1f}px/{len(tops)}f, chipped"


def end_target(page: Page, label: str) -> int:
    """The first folded row whose top stands on screen at the foot, as an index."""
    found = page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      const top = area.getBoundingClientRect().top;
      for (const row of document.querySelectorAll('#discord-log > li[data-id]')) {
        const box = row.getBoundingClientRect();
        if (box.top >= top + 20 && row.getAttribute('data-collapsed') === 'true'
            && row.querySelector('button.fold')) return row.getAttribute('data-id');
      }
      return null;
    }""")
    check(isinstance(found, str), f"{label}: no folded row stands on screen at the foot")
    assert isinstance(found, str)
    return int(found) - 200


def arrival_during_act(page: Page, api: AnchorApi, label: str, index: int) -> str:
    """An arrival landing while an expand settles at the end never moves the touched top."""
    start = park(page, index, "end")
    check(float(start["top"]) > 0, f"{label}: the touched row's top is off screen at the foot")
    page.locator(f"#discord-log > li[data-id=\"{message_id(index)}\"] button.fold").click(force=True)
    arrived = api.add("A message arriving live while the expand settles.")
    api.push("message", {"message": arrived}, str(arrived["id"]))
    until(page, f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                ".getAttribute('data-collapsed') === 'false'", f"{label}: the expand never landed")
    until(page, f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
          f"{label}: the live message was never drawn")
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: an arrival during the act moved the touched top {moved:.1f}px over {len(tops)} frames")
    until(page, "() => document.getElementById('jump-newest').hasAttribute('data-arrivals')",
          f"{label}: the arrival during the act was not counted on the Newest chip")
    return f"arrival during act: top {moved:.1f}px/{len(tops)}f, chipped"


def timeline_reads(api: AnchorApi) -> int:
    """How many channel reads the fake server has been asked for, held ones included."""
    with api.lock:
        return sum(1 for line in api.requests
                   if line.startswith("GET ") and line.split(" ", 1)[1].split("?", 1)[0].endswith(("/timeline", "/page")))


REREAD_STATE = """() => ({
  line: !document.getElementById('channel-loading').hidden,
  pill: document.getElementById('channel-freshness').textContent || '',
})"""


def frames(page: Page, count: int) -> None:
    """Let `count` animation frames pass, with the probe still sampling."""
    page.evaluate(f"""() => new Promise((done) => {{
        let n = 0; const s = () => (++n >= {count} ? done() : requestAnimationFrame(s));
        requestAnimationFrame(s); }})""")


def held_reread(page: Page, api: AnchorApi, label: str, begin: Callable[[], None]) -> str:
    """Hold the re-read `begin` starts at the server, past a slow read's second on the page clock.

    The probe started at the park samples every frame from `begin` through the held read, its
    landing and 30 more frames. Returns what the page showed while the read was out.
    """
    reads = timeline_reads(api)
    api.timeline_gate.clear()
    try:
        begin()
        until_true(page, lambda: timeline_reads(api) > reads,
                   f"{label}: the channel's re-read never reached the server")
        page.clock.fast_forward(SLOW_READ_MS)
        frames(page, 30)
        held = page.evaluate(REREAD_STATE)
    finally:
        api.timeline_gate.set()
    check("refreshing" in str(held["pill"]),
          f"{label}: the pill did not say the held re-read was out: {held['pill']!r}")
    until(page, "() => !/refreshing/.test(document.getElementById('channel-freshness').textContent || '')",
          f"{label}: the held re-read never landed")
    return "line up" if held["line"] else "pill only"


def forced_arrival_at_foot(page: Page, api: AnchorApi, label: str) -> str:
    """`arrival_holds` with its order forced: the arrival's re-read is out, slow, and lands."""
    start = park(page, COUNT - 3, "end")

    def arrive() -> None:
        arrived = api.add("A message arriving live while the channel's re-read is slow.")
        api.push("message", {"message": arrived}, str(arrived["id"]))
        until(page, f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
              f"{label}: the live message was never drawn")

    shown = held_reread(page, api, label, arrive)
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: a live arrival at the foot, its re-read slow ({shown}), moved the reading row "
          f"{moved:.1f}px over {len(tops)} frames")
    until(page, "() => document.getElementById('jump-newest').hasAttribute('data-arrivals')",
          f"{label}: the arrival below the reader was not counted on the Newest chip")
    settled(page)
    return f"forced arrival at the foot: top {moved:.1f}px/{len(tops)}f, {shown}, chipped"


def forced_arrival_during_act(page: Page, api: AnchorApi, label: str) -> str:
    """`arrival_during_act` with its order forced: the arrival's re-read is out, slow, and lands."""
    park(page, COUNT - 1, "end")
    index = end_target(page, label)
    start = park(page, index, "end")
    check(float(start["top"]) > 0, f"{label}: the touched row's top is off screen at the foot")

    def arrive() -> None:
        page.locator(f"#discord-log > li[data-id=\"{message_id(index)}\"] button.fold").click(force=True)
        arrived = api.add("A message arriving live while the expand settles and the re-read is slow.")
        api.push("message", {"message": arrived}, str(arrived["id"]))
        until(page, f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                    ".getAttribute('data-collapsed') === 'false'", f"{label}: the expand never landed")
        until(page, f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
              f"{label}: the live message was never drawn")

    shown = held_reread(page, api, label, arrive)
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: an arrival during the act, its re-read slow ({shown}), moved the touched top "
          f"{moved:.1f}px over {len(tops)} frames")
    until(page, "() => document.getElementById('jump-newest').hasAttribute('data-arrivals')",
          f"{label}: the arrival during the act was not counted on the Newest chip")
    return f"forced arrival during act: top {moved:.1f}px/{len(tops)}f, {shown}, chipped"


def forced_poll_at_foot(page: Page, api: AnchorApi, label: str) -> str:
    """A channel without threads: its poll's read held slow at the server moves no row (`#2291`)."""
    start = park(page, COUNT - 3, "end")
    shown = held_reread(page, api, label, lambda: page.clock.fast_forward(POLL_PAST_MS))
    tops = probe_frames(page)
    moved = max((abs(t - float(start["top"])) for t in tops), default=0.0)
    check(moved <= STILL_PX,
          f"{label}: the poll's slow read ({shown}) moved the reading row {moved:.1f}px over {len(tops)} frames")
    return f"forced poll at the foot, no threads: top {moved:.1f}px/{len(tops)}f, {shown}"


def open_main(page: Page, touch: bool, windowed: bool, threads: bool = True) -> None:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0", "the channel did not open")
    if not threads:
        pass  # one list: no Main to pick, no thread column beside it
    elif touch:
        page.select_option("#thread-select", "main")
    else:
        page.click("#column-toggle")
        until(page, "() => document.getElementById('side-column').getClientRects().length > 0",
              "the desk layout did not draw two columns")
    if windowed:
        until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 20",
              "windowed Main did not draw rows")
    else:
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {COUNT}",
              "Main did not draw every message")
    settled(page)


def run(chromium: object, args: argparse.Namespace, label: str, width: int, height: int,
        touch: bool, scheme: str, windowed: bool, shots_only: bool, forced: bool = False,
        flat: bool = False) -> list[str]:
    api = FlatAnchorApi() if flat else AnchorApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report: list[str] = []
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-top-anchor-") as profile:
            context = chromium.launch_persistent_context(  # type: ignore[attr-defined]
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme=scheme,
                device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
            if windowed:
                context.add_init_script(
                    "try { localStorage.setItem('vibe-talk.voice.windowed-lists', '1'); } catch (e) {}")
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            if forced:
                # Time flows as normal; the forced walks step it past a slow read's second.
                page.clock.install()
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(page, touch, windowed, threads=not flat)
            name = f"{label}-{'windowed' if windowed else 'full'}"
            if forced and flat:
                report.append(f"{name}: {forced_poll_at_foot(page, api, name)}")
                check(not errors, f"{name}: the page threw: {errors}")
                context.close()
                return report
            if forced:
                report.append(f"{name}: {forced_arrival_at_foot(page, api, name)}")
                report.append(f"{name}: {forced_arrival_during_act(page, api, name)}")
                check(not errors, f"{name}: the page threw: {errors}")
                context.close()
                return report
            if args.screenshots:
                park(page, COUNT - 3, "end")
                page.screenshot(path=str(args.screenshots / f"{name}-before.png"))
            report.append(f"{name} end: {fold_walk(page, name, COUNT - 3, COUNT - 1, 'end')}")
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{name}-after.png"))
            if not shots_only:
                report.append(f"{name} mid: {fold_walk(page, name, COUNT // 2, COUNT - 1, 'mid')}")
                report.append(f"{name}: {details_walk(page, name)}")
                report.append(f"{name}: {arrival_holds(page, api, name)}")
                report.append(f"{name}: {arrival_during_act(page, api, name, end_target(page, name))}")
            check(not errors, f"{name}: the page threw: {errors}")
            context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()
    return report


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False)):
            for windowed in (False, True):
                report += run(playwright.chromium, args, label, width, height, touch, "dark", windowed, False)
                report += run(playwright.chromium, args, f"{label}-forced", width, height, touch, "dark",
                              windowed, False, forced=True)
        report += run(playwright.chromium, args, "phone-412-forced-nothreads", 412, 915, True, "dark", False, False,
                      forced=True, flat=True)
        if args.screenshots:
            for label, width, height, touch in (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False)):
                for scheme in ("light", "dark"):
                    report += run(playwright.chromium, args, f"{label}-{scheme}", width, height, touch, scheme, False, True)
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
