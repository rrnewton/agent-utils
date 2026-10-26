#!/usr/bin/env python3
"""`#532 shift-o-latency` in real Chromium at 393x852 (touch) and 1280x800.

The owner, 2026-10-26: "shift-o is slow - why? It's not terribly slow but it does take a couple seconds.
If I am folding up 4 messages in a row in a thread I thought I could spam the key four times and see them
responsively fold up." And of e: "the 'e' key and marking archived is one of those things that I most want
to be instantly responsive locally."

Against a fake API of twenty threads, one with twelve long replies and the rest with four, every thread
unpacked in the Threads view (Unpack all):

* Shift+O: four presses 60 ms apart on the last reply fold the four last replies, each folded in the frame
  after its key; each press's handler (the keydown from the window's capture to the end of its dispatch)
  under `--budget-ms` at the median and twice that at most, and from the key to the next frame drawn
  under `--frame-ms` at the median.
* e: the same on another thread's replies, each marked Done in the frame after its key, held to the same.
* style: hiding one row restyles a few elements, not the page: under `--restyle-max` elements in the
  browser's own trace (on a touch screen a rule of #429 keyboard-hides-bar restyled every element of the
  page, about 5,000, at every show or hide).

The numbers are printed for each size. Pass --report-only to print them without failing, --screenshots DIR
to keep a PNG after the folds, and --web-root DIR to run against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import statistics
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import Json, check, handler_for, said
from two_column_browser import ColumnsApi, Desk, thread_record

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

SIZES = (("phone-393", 393, 852, True), ("desk", 1280, 800, False))

TARGET = "spaces/A/threads/target"
OTHER = "spaces/A/threads/t18"
PARA = ("The nightly run finished; the integration shard was retried twice and passed on the third attempt, "
        "and the artifact upload waited on the cache for most of an hour before it went through. ")


def long_reply(n: int) -> tuple[str, str]:
    """A reply as an agent writes one, and as src/render.rs renders it."""
    text = "\n\n".join([f"Reply {n}. " + PARA * 2, PARA, "```\ncargo test --release\nok 123 passed\n```", PARA])
    html = (f"<p>Reply {n}. {PARA * 2}</p>\n<p>{PARA}</p>\n<pre><code>cargo test --release\nok 123 passed\n"
            f"</code></pre>\n<p>{PARA}</p>\n")
    return text, html


class LatencyApi(ColumnsApi):
    """Twenty threads of long replies, Done and its Undo served."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        self.threads = []
        self.done: set[str] = set()
        index = 0
        for t in range(20):
            thread_id = TARGET if t == 19 else f"spaces/A/threads/t{t}"
            replies = 12 if thread_id == TARGET else 4
            root_at = index
            root = said(index, f"Thread {t} root. {PARA}", "coder")
            root["thread"] = thread_record(thread_id, index, replies)
            self.messages.append(root)
            index += 1
            last = root
            for r in range(replies):
                text, html = long_reply(r)
                row = said(index, text, "coder")
                row["content_html"] = html
                row["thread"] = thread_record(thread_id, root_at, replies, is_root=False)
                self.messages.append(row)
                last = row
                index += 1
            self.threads.append({"id": thread_id, "root": root, "title": "Thread", "reply_count": replies,
                                 "reply_count_exact": True, "updated_at": last["timestamp"]})

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        ids = body.get("messages")
        named = [str(one) for one in ids] if isinstance(ids, list) else []
        if path.endswith("/dismiss"):
            with self.lock:
                self.done.update(named)
            return 200, {"dismissed": named}
        if path.endswith("/restore"):
            with self.lock:
                self.done.difference_update(named)
            return 200, {"messages": named}
        return super().post(path, body)

    def timeline(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            done = sorted(self.done)
        return {**super().timeline(query), "dismissed": done}


# Each key's handler, from the window's capture to the end of its dispatch (a window listener in the bubble
# phase, after the page's own on the document), and from the key to the frame after it; and the first frame
# each watched row was drawn folded or Done.
WATCH_JS = """(ids) => {
  window.__keys = [];
  window.__seen = {};
  window.__ids = ids;
  if (window.__watching) return;
  window.__watching = true;
  addEventListener('keydown', (event) => {
    if (event.key !== 'O' && event.key !== 'e') return;
    const press = {start: performance.now(), stamp: event.timeStamp, handler: null, frame: null};
    window.__keys.push(press);
    requestAnimationFrame(() => setTimeout(() => { press.frame = performance.now() - press.stamp; }, 0));
  }, true);
  addEventListener('keydown', (event) => {
    const press = window.__keys[window.__keys.length - 1];
    if (press && press.handler === null) press.handler = performance.now() - press.start;
  });
  const look = (now) => {
    for (const id of window.__ids) {
      if (window.__seen[id] !== undefined) continue;
      const li = document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`);
      if (li && (li.getAttribute('data-read-run') || li.getAttribute('data-archived') === 'true')) window.__seen[id] = now;
    }
    requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}"""

BLOCK_IDS_JS = """(id) => [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}'][data-id]`)]
  .map((li) => li.getAttribute('data-id'))"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per size into (default: none)")
    parser.add_argument("--size", choices=[size[0] for size in SIZES], action="append",
                        help="walk only at these sizes, repeatable (default: all of them)")
    parser.add_argument("--budget-ms", type=float, default=50.0,
                        help="most a press's handler may take at the median, in ms; twice it at most (default: 50)")
    parser.add_argument("--frame-ms", type=float, default=120.0,
                        help="most from a key to the next frame drawn at the median, in ms (default: 120)")
    parser.add_argument("--restyle-max", type=int, default=500,
                        help="most elements hiding one row may restyle (default: 500)")
    parser.add_argument("--report-only", action="store_true",
                        help="print the numbers without failing on them (default: fail)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def unpack_all(desk: Desk) -> None:
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == "true":
            break
        desk.page.click("#side-unpack")
        desk.settled()
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{TARGET}']\").length >= 12",
                  "the long thread did not unpack under its card", seconds=20)


def select(desk: Desk, row_id: str) -> None:
    desk.page.evaluate("(id) => { const li = document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`);"
                       " li.scrollIntoView({block: 'center'}); selectRow(sidePane, li); }", row_id)
    desk.settled()


def presses(desk: Desk, key: str, ids: list[str]) -> tuple[list[float], list[float], dict[str, float]]:
    """Four presses of `key` 60 ms apart, watching `ids` for the frame each is drawn folded or Done."""
    page = desk.page
    page.evaluate(WATCH_JS, ids)
    for _ in range(4):
        page.keyboard.press(key)
        page.wait_for_timeout(60)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not page.evaluate(
            "() => window.__keys.length === 4 && window.__keys.every((press) => press.frame !== null)"):
        page.wait_for_timeout(50)
    keys = page.evaluate("() => window.__keys")
    seen = page.evaluate("() => window.__seen")
    assert isinstance(keys, list) and isinstance(seen, dict)
    first = float(keys[0]["start"]) if keys else 0.0
    return ([float(press["handler"] or 0) for press in keys], [float(press["frame"] or 0) for press in keys],
            {str(id): float(at) - first for id, at in seen.items()})


def judge(desk: Desk, args: argparse.Namespace, what: str, handler: list[float], frame: list[float],
          seen: dict[str, float], ids: list[str]) -> str:
    said = (f"{what}: handlers {', '.join(f'{ms:.0f}' for ms in handler)} ms, key to frame "
            f"{', '.join(f'{ms:.0f}' for ms in frame)} ms, rows drawn at {', '.join(f'{seen.get(id, -1):.0f}' for id in ids)} ms")
    if not args.report_only:
        desk.check(len(handler) == 4, f"{what}: not four presses heard: {said}")
        desk.check(all(id in seen for id in ids), f"{what}: not every press drew its row: {said}")
        desk.check(statistics.median(handler) <= args.budget_ms and max(handler) <= 2 * args.budget_ms,
                   f"{what}: over the {args.budget_ms:.0f} ms budget: {said}")
        desk.check(statistics.median(frame) <= args.frame_ms, f"{what}: the frames came late: {said}")
    return said


RESTYLE_JS = """() => { const rows = [...document.querySelectorAll('#side-cards > li[data-unpack-thread][data-id]')];
  window.__hide = rows[2]; document.body.offsetHeight; }"""


def restyled(page: Page) -> int:
    """How many elements the browser restyles when one unpacked row is hidden, from its own trace."""
    page.evaluate(RESTYLE_JS)
    cdp = page.context.new_cdp_session(page)
    events: list[Json] = []
    finished: list[bool] = []
    cdp.on("Tracing.dataCollected", lambda event: events.extend(event["value"]))
    cdp.on("Tracing.tracingComplete", lambda _event: finished.append(True))
    cdp.send("Tracing.start", {"transferMode": "ReportEvents", "traceConfig": {
        "includedCategories": ["devtools.timeline", "disabled-by-default-devtools.timeline"]}})
    page.evaluate("() => { window.__hide.hidden = true; document.body.offsetHeight; window.__hide.hidden = false; }")
    cdp.send("Tracing.end")
    deadline = time.monotonic() + 10
    while not finished and time.monotonic() < deadline:
        page.wait_for_timeout(50)
    counts = []
    for event in events:
        data = event.get("args")
        begin = data.get("beginData") if isinstance(data, dict) else None
        if event.get("name") == "UpdateLayoutTree" and isinstance(data, dict):
            counts.append(int(str(data.get("elementCount") or (begin or {}).get("elementCount") or 0)))
    return max(counts, default=0)


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = LatencyApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-fold-latency-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"fold-latency-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", fixture.TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 5",
                          "the channel did not open", seconds=20)
            page.select_option("#thread-select", "threads")
            desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length >= 20",
                          "the Threads view has not every card", seconds=20)
            unpack_all(desk)
            page.wait_for_timeout(1000)
            rows = int(page.evaluate("() => document.querySelectorAll('#side-cards > li').length"))
            notes = [f"{rows} rows in the Threads view"]
            # A first key, away from what is measured: the page's first key in a session says the keys are
            # in use on the root, which styles the page once.
            page.keyboard.press("ArrowUp")
            desk.settled()

            target = page.evaluate(BLOCK_IDS_JS, TARGET)
            assert isinstance(target, list)
            select(desk, str(target[-1]))
            folded = [str(one) for one in target[-4:]][::-1]
            handler, frame, seen = presses(desk, "Shift+O", folded)
            notes.append(judge(desk, args, "Shift+O", handler, frame, seen, folded))
            desk.check(page.evaluate("(id) => document.querySelector(`#side-cards > li[data-unpack-thread][data-id='${id}']`)"
                                     ".getAttribute('data-read-run')", folded[-1]) == "head",
                       "the four folds are not one line")
            desk.shot("1-folded")

            other = page.evaluate(BLOCK_IDS_JS, OTHER)
            assert isinstance(other, list)
            select(desk, str(other[0]))
            marked = [str(one) for one in other[:4]]
            handler, frame, seen = presses(desk, "e", marked)
            notes.append(judge(desk, args, "e", handler, frame, seen, marked))
            desk.shot("2-done")

            count = restyled(page)
            notes.append(f"hiding one row restyled {count} elements")
            if not args.report_only:
                desk.check(count <= args.restyle_max, f"hiding one row restyled {count} elements, over {args.restyle_max}")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
