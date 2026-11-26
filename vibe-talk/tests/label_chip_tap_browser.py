#!/usr/bin/env python3
"""A tapped label chip keeps the tapped row where it stands, per frame (`#2285`).

`#2285 vibe-talk-labels-browser-tapped-message-jumps`, owner thread 168 and rule 370: tapping a
`#label` chip on a card opens the label's search, and the tapped message keeps its top exactly,
however few results stay. Real Chromium on the labels fixture's fake API (`labels_browser`), the
search closed before the tap as a reader meets it, at a 412x915 phone (touch, scale 2.6) with the
on-screen keyboard really open (the viewport shrunk by its height and `visualViewport` waited
for) and a 1280x800 desk:

  short (and short-closed, the phone with its keyboard down): two results, too few to scroll far. On the desk the tapped row stood lower than the list's head
    lets it stand (room above the rows holds it); on the phone it stood higher than the fitting
    list reaches (a floor under the content holds it).
  long: many results, the list scrolls; the ordinary hold.
  reclaim: many results, the first of them tapped low on the screen, so room above holds it in
    a list that scrolls; the reader then scrolls past that room, and nothing moves when it goes.

Every frame from before the tap until the answer has settled records the row's top; the check is
that none differs from the top before the tap by more than half a pixel (positions are
fractional). Waits are on conditions only, the hang backstop aside (`browser_waits`, rule 346).
`--traces DIR` keeps each scene's frames as JSON; `--screenshots DIR` keeps a PNG before and after.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import labels_browser
from keyboard_hides_bar_browser import KEYBOARD_PX
from offline_cache_browser import TOKEN, FakeApi, Json, check, message

if TYPE_CHECKING:
    from playwright.sync_api import Page

LIMIT_PX = 0.5
"""The most the tapped row's top may differ from its top before the tap, in any frame."""

SETTLE_FRAMES = 30
"""Frames running with no request in flight, after the search is counted: the settle."""

FETCH_WATCH_JS = """(() => {
  window.__inflight = 0;
  const plain = window.fetch.bind(window);
  window.fetch = (...args) => {
    window.__inflight += 1;
    return plain(...args).finally(() => { window.__inflight -= 1; });
  };
})();"""
"""Counts the page's requests in flight, so the settle waits for every answer to land."""


class TapApi(labels_browser.LabelsApi):
    """The labels fixture with a script of its own: (content, labels) rows posted to the channel."""

    def __init__(self, script: list[tuple[str, list[str]]]) -> None:
        FakeApi.__init__(self)
        self.messages = []
        for index, (words, labels) in enumerate(script):
            row = message(index, words)
            if labels:
                row["labels"] = labels
            self.messages.append(row)
        self.threads = []
        self.groups = []
        self.next_group = 0


def long_script() -> list[tuple[str, list[str]]]:
    """Ten plain rows, then forty, every other one carrying #kvm: twenty results, more than a screen."""
    rows: list[tuple[str, list[str]]] = [(f"Plain lane note {n}.", []) for n in range(10)]
    for n in range(40):
        if n % 2 == 0:
            rows.append((f"KVM result {n} on the host.\n#kvm", ["kvm"]))
        else:
            rows.append((f"Other lane result {n}.", []))
    return rows


TRACE_JS = r"""(id) => {
  window.__tapTrace = [];
  const row = () => document.querySelector(`#discord-log > li[data-id="${id}"]`);
  const area = document.getElementById('scroll-area');
  const list = document.getElementById('discord-log');
  const tick = () => {
    const r = row();
    window.__tapTrace.push({
      top: r ? r.getBoundingClientRect().top : null,
      scrollTop: area.scrollTop,
      headRoom: list.style.paddingTop || '',
      floor: (area.firstElementChild && [...area.children].find((kid) => kid.contains(list)) || list).style.minHeight || '',
      query: document.getElementById('search-field').value,
    });
    if (window.__tapTracing) requestAnimationFrame(tick);
  };
  window.__tapTracing = true;
  requestAnimationFrame(tick);
}"""


class Scene:
    """One page at one size, signed in to the channel."""

    def __init__(self, page: Page, api: TapApi, label: str, touch: bool, shots: Path | None, traces: Path | None) -> None:
        self.page = page
        self.api = api
        self.label = label
        self.touch = touch
        self.shots = shots
        self.traces = traces

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def until(self, script: str, what: str) -> None:
        browser_waits.wait_until(self.page, script, f"{self.label}: {what}")

    def frames(self, count: int) -> None:
        self.page.evaluate("(n) => new Promise((done) => { const step = () => (n-- <= 0 ? done() : requestAnimationFrame(step)); requestAnimationFrame(step); })", count)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def sign_in(self, rows: int) -> None:
        page = self.page
        page.fill("#api-token", TOKEN)
        page.click("#save-token")
        self.until("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden",
                   "neither the Welcome nor the channel opened")
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
        self.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {rows}",
                   f"the channel did not open with its {rows} messages")
        self.until("() => document.querySelectorAll('#discord-log .label-chip').length > 0", "no label chips stood on the cards")
        self.frames(2)

    def keyboard_open(self, width: int, height: int) -> None:
        """The on-screen keyboard, really: the viewport shrinks by its height, as a phone's does."""
        shrunk = height - KEYBOARD_PX
        self.page.set_viewport_size({"width": width, "height": shrunk})
        self.until(f"() => Math.round(window.visualViewport.height) === {shrunk}", "the keyboard's shrink did not reach visualViewport")
        self.frames(2)

    def place_row(self, row_id: str, top: float) -> None:
        """Scroll the list so the row's top stands at `top` px in the viewport (as near as the list allows)."""
        self.page.evaluate("""([id, top]) => {
          const area = document.getElementById('scroll-area');
          const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
          area.scrollTop += row.getBoundingClientRect().top - top;
        }""", [row_id, top])
        self.frames(3)

    def tap_chip(self, row_id: str) -> list[Json]:
        """Tap the row's first label chip; answer the frames traced from before the tap to the settle."""
        page = self.page
        chip = f'#discord-log > li[data-id="{row_id}"] .label-chip >> nth=0'
        box = page.locator(chip).bounding_box()
        self.check(box is not None, f"{row_id}'s chip is not on the screen")
        assert box is not None
        page.evaluate(TRACE_JS, row_id)
        self.until("() => window.__tapTrace.length >= 3", "the trace did not start")
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            page.touchscreen.tap(x, y)
        else:
            page.mouse.click(x, y)
        self.until("() => document.getElementById('search-field').value.startsWith('label:')", "the chip did not open the label search")
        self.until("() => /\\d+ of/.test(document.getElementById('search-count').textContent)", "the label search was not counted")
        # Settled: no request of the page's in flight for SETTLE_FRAMES frames running, so a late
        # answer's landing is inside the frames traced (the in-flight count is FETCH_WATCH_JS's).
        page.evaluate("""(frames) => new Promise((done) => {
          let quiet = 0;
          const step = () => { quiet = window.__inflight === 0 ? quiet + 1 : 0;
            if (quiet >= frames) done(); else requestAnimationFrame(step); };
          requestAnimationFrame(step);
        })""", SETTLE_FRAMES)
        page.evaluate("() => { window.__tapTracing = false; }")
        trace = page.evaluate("() => window.__tapTrace")
        return [frame for frame in trace if isinstance(frame, dict)]

    def judge(self, name: str, trace: list[Json]) -> float:
        tops = tops_of(trace)
        self.check(len(tops) == len(trace) and len(tops) > SETTLE_FRAMES, f"{name}: the tapped row left the list in some frame")
        worst = max(abs(top - tops[0]) for top in tops)
        if self.traces is not None:
            (self.traces / f"{self.label}-{name}.json").write_text(json.dumps(
                {"scene": f"{self.label}-{name}", "limit_px": LIMIT_PX, "worst_px": worst, "frames": trace}, indent=1))
        self.check(worst <= LIMIT_PX, f"{name}: the tapped row moved {worst:.3f} px (top {tops[0]} before the tap; frames {tops})")
        return worst


def tops_of(trace: list[Json]) -> list[float]:
    """The tapped row's top in each frame that drew it."""
    return [float(top) for top in (frame.get("top") for frame in trace) if isinstance(top, (int, float))]


def run_scene(playwright: object, args: argparse.Namespace, size: str, name: str) -> float:
    phone = size == "phone"
    width, height = (412, 915) if phone else (1280, 800)
    short = name.startswith("short")
    script = [(words, labels) for words, labels, _thread, _root in labels_browser.SCRIPT] if short else long_script()
    api = TapApi(script)
    server = ThreadingHTTPServer(("127.0.0.1", 0), labels_browser.labels_handler(api))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory() as profile:
            context = playwright.chromium.launch_persistent_context(  # type: ignore[attr-defined]
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.6 if phone else 1,
                has_touch=phone, is_mobile=phone, color_scheme="light")
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            page.add_init_script(FETCH_WATCH_JS)
            page.goto(f"http://127.0.0.1:{server.server_port}")
            scene = Scene(page, api, f"{size}", phone, args.screenshots, args.traces)
            scene.sign_in(len(script))
            if phone and name != "short-closed":
                scene.keyboard_open(width, height)
            if name.startswith("short") and phone:
                # The labels walk's row 201 ("#kvm #benchmarking"), high on the screen, just under
                # where the open search bar ends: the two results (201, 205) put it lower, so a
                # floor under them holds it.
                tapped = "201"
                scene.place_row(tapped, 110)
            elif name == "short":
                # 205, the other #kvm result, low on the screen: under 201 alone it stands higher,
                # so room above the rows holds it.
                tapped = "205"
                scene.place_row(tapped, height * 0.6)
            elif name == "long":
                # A result in the middle of the list, mid-screen: the list scrolls either way.
                tapped = "230"
                scene.place_row(tapped, (height if not phone else height - KEYBOARD_PX) / 2)
            else:
                # The FIRST result, low on the screen: it stands lower than the results' head
                # allows, in a list that scrolls.
                tapped = "210"
                scene.place_row(tapped, (height if not phone else height - KEYBOARD_PX) - 220)
            scene.shot(f"{name}-before")
            trace = scene.tap_chip(tapped)
            worst = scene.judge(name, trace)
            held = trace[-1]
            # Which room held it, so a scene cannot pass by not needing one.
            # With the keyboard up the phone's two results and composer overflow its 579 px, so
            # the ordinary hold keeps the row there; the floor's case is the keyboard down.
            if name == "short-closed" or (short and not phone):
                needed = "floor" if phone else "headRoom"
                scene.check(bool(held.get(needed)), f"short: no {needed} held the row ({held})")
            scene.shot(f"{name}-after")
            if args.screenshots is not None:
                # The before and after shots are the frames either side of the tap; this sidecar names
                # the anchor by its words and gives its top in both, as the shot review asks.
                tops = tops_of(trace)
                words = page.evaluate("(id) => (document.querySelector(`#discord-log > li[data-id=\"${id}\"] .body`) || {}).textContent || ''", tapped)
                (args.screenshots / f"{size}-{name}-anchor.json").write_text(json.dumps(
                    {"scene": f"{size}-{name}", "anchor_id": tapped, "anchor_text": str(words)[:120],
                     "top_before": tops[0], "top_after": tops[-1], "worst_px": worst,
                     "room_above": held.get("headRoom") or "", "floor": held.get("floor") or ""}, indent=1))
            if name == "reclaim":
                worst = max(worst, reclaim(scene, tapped))
            context.close()
            print(f"{size}-{name}: tapped row held within {worst:.3f} px (room above {held.get('headRoom') or 'none'}, floor {held.get('floor') or 'none'})")
            return worst
    finally:
        server.shutdown()


def reclaim(scene: Scene, tapped: str) -> float:
    """The reader scrolls down past the room above the rows: it goes, and no frame shows a row move for it."""
    page = scene.page
    room = float(page.evaluate("() => parseFloat(document.getElementById('discord-log').style.paddingTop) || 0"))
    scene.check(room > 0, "no room stood above the rows to hold the first result low on the screen")
    distance = room + 120
    page.evaluate(TRACE_JS, tapped)
    scene.until("() => window.__tapTrace.length >= 3", "the reclaim trace did not start")
    box = page.locator("#scroll-area").bounding_box()
    assert box is not None
    page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    page.mouse.wheel(0, distance)
    scene.until("() => !document.getElementById('discord-log').style.paddingTop", "the room above was not reclaimed once scrolled past")
    # Settled: the place has not changed for ten frames running.
    page.evaluate("""() => new Promise((done) => {
      const area = document.getElementById('scroll-area');
      let last = area.scrollTop; let still = 0;
      const step = () => { if (area.scrollTop === last) still += 1; else { still = 0; last = area.scrollTop; }
        if (still >= 10) done(); else requestAnimationFrame(step); };
      requestAnimationFrame(step);
    })""")
    page.evaluate("() => { window.__tapTracing = false; }")
    trace = [frame for frame in page.evaluate("() => window.__tapTrace") if isinstance(frame, dict)]
    tops = tops_of(trace)
    scene.check(len(tops) == len(trace), "the tapped row left the list while the reader scrolled")
    # Scrolling down moves every row up: no frame may bring the row down (the reclaim's jump would),
    # and in all it moves up by the reader's own scroll.
    steps = [after - before for before, after in zip(tops, tops[1:])]
    down = max(steps, default=0.0)
    moved = tops[0] - tops[-1]
    if scene.traces is not None:
        (scene.traces / f"{scene.label}-reclaim-scroll.json").write_text(json.dumps(
            {"scene": f"{scene.label}-reclaim-scroll", "room_px": room, "wheel_px": distance, "moved_px": moved,
             "worst_down_step_px": down, "frames": trace}, indent=1))
    scene.check(down <= LIMIT_PX, f"a frame moved the row down {down:.3f} px while the reader scrolled down (room {room} px reclaimed)")
    scene.check(abs(moved - distance) <= LIMIT_PX, f"the row moved {moved:.3f} px for the reader's {distance:.3f} px scroll (room {room} px)")
    return abs(moved - distance)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--traces", type=Path)
    args = parser.parse_args()
    for folder in (args.screenshots, args.traces):
        if folder is not None:
            folder.mkdir(parents=True, exist_ok=True)
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        raise SystemExit("playwright is not installed: python3 -m playwright install chromium")
    with sync_playwright() as playwright:
        for size in ("phone", "desk"):
            for name in ("short", "long", "reclaim") + (("short-closed",) if size == "phone" else ()):
                run_scene(playwright, args, size, name)
    print("label chip tap: every tapped row held")


if __name__ == "__main__":
    main()
