#!/usr/bin/env python3
"""A thread's layout prices its boxes through the REAL server (`#2385`, S-t1).

`#2385 vibe-talk-thread-open-layout-first`. The thread boxes walk (thread_reply_boxes_browser.py)
proves the page's half against a fake API; this one proves the layout read end to end: the page's
`GET /api/v1/channels/{id}/threads/{thread_id}/layout` reaches the shipped server binary's own
route, which answers from the server's own copy of the thread.

The server is the release binary on a loopback port with `--fake-discord` as its only fake (a
throwaway store and message cache, serving on), one per screen size. Through the test-only
fake-seed route it seeds a thread whose root's exact reply count the fake keeps (`reply_count`,
raised by each seeded reply). The owner's case: the device sees the thread with its root and one
reply (an earlier visit, in Main by its deep link), and eight more replies of four shapes are posted
(one line, a paragraph, eight short lines, one long enough to fold). An untagged write-token read of
the thread (a read the server makes upstream) brings them into the server's copy, and the layout
route is asked until it answers all nine (a condition, not a clock).

The page then reopens in Main by its deep link and the reader opens the thread: its root says nine
replies, the device holds one. In the browser only the thread's own timeline read (the bodies) is
HELD, at the page's route, so "before the bodies" is an order of events (owner rule 346): the
layout read goes through to the real route.
At 412x915 (dark, touch) and 1280x800 (light) it asserts:

  the layout read was sent before the bodies' read, and the real route answered it 200 with the
  thread's nine entries, in thread order;
  every box stands at its entry's price (core/channels/pending.ts `layoutReplyPx`, mirrored as
  `layout_price` in thread_reply_boxes_browser.py) within 1 px, and the prices are not one height;
  released, the bodies fill the boxes in place: the first reply stands where the first box stood
  (within 0.5 px), and the rows above move at most 0.5 px on any frame (rule 370) of their own
  (the coverage line above the thread, `#2391`, is subtracted as in the boxes walk).

Negative control: `--web-root DIR` serves another page build (main before S-t1) over the same real
server; it sends no layout read, and the walk fails.

Needs a release build (`cargo build --release`). `--screenshots DIR` keeps a PNG per stage.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import re
import socket
import subprocess
import time
import urllib.parse
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING

import browser_scratch
import browser_waits
from thread_reply_boxes_browser import layout_price

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Request, Response, Route

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "thread-layout-server-read-token-not-a-real-credential"
WRITE_TOKEN = "thread-layout-server-write-token-not-a-real-credential"
THREAD_ID = "walk-layout-thread"
REPLIES = 9
# The replies the device saw on its earlier visit; the rest are posted after it.
SEEN = 1
# Generous condition timeouts for a hot host (rule 346): every wait ends on its condition.
WAIT_S = 90.0

Json = dict[str, object]
# One frame of the sampler's trace.
Frame = dict[str, object]

SAMPLER = r"""
(() => {
  const trace = []; window.__trace = trace; let last = '';
  const top = (el) => el ? Math.round(el.getBoundingClientRect().top * 100) / 100 : null;
  const row = (id) => [...document.querySelectorAll('#discord-log > li[data-ids]')]
    .find((li) => li.getAttribute('data-ids').split(' ').includes(id) && li.getClientRects().length) || null;
  const tick = () => {
    const ids = window.__ids || {};
    const boxes = [...document.querySelectorAll('#pending-replies .pending-reply')];
    const f = {rows: document.querySelectorAll('#discord-log > li[data-id]').length, boxes: boxes.length,
      root: ids.root ? top(row(ids.root)) : null, first: ids.first ? top(row(ids.first)) : null,
      box: top(boxes[0]), bh: boxes.map((b) => Math.round(b.getBoundingClientRect().height)).join('/'),
      seam: Math.round(((document.getElementById('channel-summary') || {getBoundingClientRect: () => ({height: 0})}).getBoundingClientRect().height) * 100) / 100,
      scroll: Math.round((document.getElementById('scroll-area') || {scrollTop: 0}).scrollTop)};
    const key = JSON.stringify(f);
    if (key !== last) { last = key; f.t = Math.round(performance.now()); trace.push(f); }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})();
"""


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
    with urllib.request.urlopen(request, timeout=30) as response:
        parsed: Json = json.loads(response.read() or b"null")
        return parsed


def until(page: Page, script: str, why: str) -> None:
    deadline = time.monotonic() + WAIT_S
    while time.monotonic() < deadline:
        if page.evaluate(script):
            return
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def num(frame: Frame, key: str) -> float | None:
    value = frame.get(key)
    return float(value) if isinstance(value, (int, float)) else None


def drift(trace: list[Frame], key: str, start: int) -> float:
    """The most screen top `key` moved from its value at frame `start`, over the frames after."""
    tops = [top for top in (num(f, key) for f in trace[start:]) if top is not None]
    return max((abs(t - tops[0]) for t in tops), default=0.0)


class Server:
    """One throwaway release server with `--fake-discord`, its seeded thread and its base URL."""

    def __init__(self, work: Path, binary: Path) -> None:
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.work = work
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{self.port}"\n'
            '[discord]\nbot_token = "thread-layout-server-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\nserve = true\npath = "{work}/cache.sqlite3"\n')
        self.log = (work / "server.log").open("wb")
        self.process = subprocess.Popen(
            [str(binary), "--config", str(work / "vibe-talk.toml"), "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=self.log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + WAIT_S
        while True:
            try:
                call(self.base, "GET", "/healthz", READ_TOKEN)
                break
            except Exception:
                check(self.process.poll() is None, f"the server exited: {self.log_tail()}")
                check(time.monotonic() < deadline, f"the server did not come up: {self.log_tail()}")
                time.sleep(0.25)
        self.now = int(time.time() * 1000)
        self.root = self.seed({"author": "release-lane", "text": "Overnight run of the release lane, replies below.",
                               "at_ms": self.now - 30 * 60_000, "thread_id": THREAD_ID, "thread_root": True,
                               "reply_count": 0})
        self.replies = [self.seed_reply(k) for k in range(SEEN)]

    def seed(self, body: Json) -> str:
        answer = call(self.base, "POST", f"/api/v1/channels/{CHANNEL}/fake-seed", WRITE_TOKEN, body)
        return str(answer["id"])

    def seed_reply(self, k: int) -> str:
        """Reply `k` (from 0), minutes ago so the copy keeps it, in one of four shapes."""
        shapes = ["Done.", "The step's output as the run wrote it, line by line. " * 3,
                  "\n".join(f"line {n}" for n in range(8)),
                  "The step's output, line by line, as the run wrote it. " * 7]
        return self.seed({"author": "release-lane", "text": f"Reply {k + 1}: step {k + 1} finished. {shapes[k % 4]}",
                          "at_ms": self.now - (29 - k) * 60_000, "thread_id": THREAD_ID,
                          "root_message_id": self.root})

    def post_rest(self) -> None:
        """The replies posted after the device's earlier visit."""
        self.replies += [self.seed_reply(k) for k in range(SEEN, REPLIES)]

    def fill_copy(self) -> Json:
        """Bring the thread into the server's copy by an untagged write-token read (one the server
        makes upstream, not a page's cache-first read), and wait until the real layout route answers
        all of it."""
        thread = urllib.parse.quote(THREAD_ID, safe="")
        call(self.base, "GET", f"/api/v1/channels/{CHANNEL}/timeline?view=thread&thread_id={thread}&limit=50",
             WRITE_TOKEN)
        deadline = time.monotonic() + WAIT_S
        while True:
            layout = call(self.base, "GET", f"/api/v1/channels/{CHANNEL}/threads/{thread}/layout", READ_TOKEN)
            entries = layout.get("entries")
            if isinstance(entries, list) and len(entries) == REPLIES:
                return layout
            check(time.monotonic() < deadline, f"the server's copy never held the thread's replies: {layout}")
            time.sleep(0.25)

    def log_tail(self) -> str:
        self.log.flush()
        return (self.work / "server.log").read_text(errors="replace")[-2000:]

    def close(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=20)
        self.log.close()


def open_main(page: Page, server: Server, tag: str) -> None:
    """Open the channel in Main by its deep link (`#462`; All's read would bring every reply into
    this device's store) and wait for the thread in the picker."""
    page.goto(f"{server.base}/?c={CHANNEL}&v=main", wait_until="load")
    until(page, f"() => [...document.querySelectorAll('#thread-select option')].some((o) => o.value === 'thread:{THREAD_ID}')",
          f"{tag}: the thread never reached the picker")
    settled(page)


def earlier_visit(page: Page, server: Server, tag: str) -> None:
    """The device sees the thread with its root and the replies posted so far, and goes back to Main."""
    open_main(page, server, tag)
    page.select_option("#thread-select", f"thread:{THREAD_ID}")
    until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {1 + SEEN}",
          f"{tag}: the earlier visit did not draw the root and its reply")
    settled(page)
    page.select_option("#thread-select", "main")
    until(page, "() => document.getElementById('thread-select').value === 'main'", f"{tag}: Main did not come back")
    # The device keeps its copy on settle; a beat for that settle, as the boxes walk takes (not a
    # budget: nothing is timed or asserted on it).
    page.wait_for_timeout(1500)


def bodies_read(url: str) -> bool:
    """Whether `url` is the thread's own timeline read: the read that brings its bodies."""
    parts = urllib.parse.urlsplit(url)
    return parts.path.endswith("/timeline") and urllib.parse.parse_qs(parts.query).get("view") == ["thread"]


def serve_page_from(page: Page, web_root: Path) -> None:
    """Serve every non-API path from the page build in `web_root` (the negative control's page)."""
    root = web_root.resolve()

    def from_dir(route: Route) -> None:
        path = urllib.parse.urlsplit(route.request.url).path
        if path.startswith("/api/"):
            route.fallback()
            return
        relative = "voice.html" if path in ("/", "") else path.lstrip("/")
        target = (root / relative).resolve()
        if root not in target.parents or not target.is_file():
            route.fulfill(status=404, body="")
            return
        route.fulfill(status=200, body=target.read_bytes(), headers={
            "Content-Type": mimetypes.guess_type(target.name)[0] or "application/octet-stream",
            "Cache-Control": "no-store"})

    page.route("**/*", from_dir)


class Walk:
    """One browser corner: the bodies' read held at the page, every request in the order it went."""

    def __init__(self, page: Page, server: Server, tag: str, shots: Path | None) -> None:
        self.page, self.server, self.tag, self.shots = page, server, tag, shots
        self.sent: list[str] = []
        self.held: list[Route] = []
        self.releasing = False
        self.layouts: list[tuple[int, Json]] = []
        page.on("request", self.on_request)
        page.on("response", self.on_response)
        page.route(re.compile(r"/api/v1/channels/[^/]+/timeline\?"), self.hold_bodies)

    def on_request(self, request: Request) -> None:
        if "/api/" in request.url:
            self.sent.append(request.url)

    def on_response(self, response: Response) -> None:
        if "/threads/" in response.url and response.url.split("?")[0].endswith("/layout"):
            try:
                body = response.json()
            except Exception:
                body = None
            self.layouts.append((response.status, body if isinstance(body, dict) else {}))

    def hold_bodies(self, route: Route) -> None:
        if bodies_read(route.request.url) and not self.releasing:
            self.held.append(route)
        else:
            route.fallback()

    def release(self) -> None:
        self.releasing = True
        for route in self.held:
            route.fallback()
        self.held.clear()

    def shot(self, stage: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.tag}-{stage}.png"))

    def frames(self) -> list[Frame]:
        return list(self.page.evaluate("() => window.__trace"))

    def run(self) -> str:
        page = self.page
        new = REPLIES - SEEN
        first_new = self.server.replies[SEEN]
        page.add_init_script(SAMPLER)
        page.add_init_script(f"window.__ids = {json.dumps({'root': self.server.root, 'first': first_new})};")
        open_main(page, self.server, self.tag)
        page.select_option("#thread-select", f"thread:{THREAD_ID}")
        until(page, f"() => document.querySelectorAll('#pending-replies .pending-reply').length === {new}",
              f"{self.tag}: entering the thread drew no {new} boxes (the root's count did not say it is short)")
        deadline = time.monotonic() + WAIT_S
        while not self.held and time.monotonic() < deadline:
            page.wait_for_timeout(25)
        check(bool(self.held), f"{self.tag}: the bodies' read was never sent")
        # Sent before the bodies: the page's own order of requests, not arrival at the server.
        bodies_at = next(i for i, url in enumerate(self.sent) if bodies_read(url))
        layout_at = next((i for i, url in enumerate(self.sent) if url.split("?")[0].endswith("/layout")), None)
        check(layout_at is not None and layout_at < bodies_at,
              f"{self.tag}: no layout read went out before the bodies' read (requests: {self.sent[-6:]})")
        until(page, "() => String(document.getElementById('pending-replies').getAttribute('data-priced') || '').startsWith('layout:')",
              f"{self.tag}: the real layout's answer never priced the boxes")
        settled(page)
        settled(page)
        check(bool(self.layouts), f"{self.tag}: no layout answer was seen")
        status, layout = self.layouts[-1]
        entries = layout.get("entries")
        check(status == 200 and isinstance(entries, list) and len(entries) == REPLIES,
              f"{self.tag}: the real route answered {status} with {entries!r}")
        assert isinstance(entries, list)
        check([str(e.get("id")) for e in entries if isinstance(e, dict)] == self.server.replies,
              f"{self.tag}: the layout's entries are not the thread's replies in order")
        drawn = page.evaluate("() => { const box = document.getElementById('pending-replies');"
                              " return {base: Number(box.getAttribute('data-px')),"
                              " heights: [...box.querySelectorAll('.pending-reply')].map((li) => li.getBoundingClientRect().height)}; }")
        base = float(drawn["base"])
        heights = [float(h) for h in drawn["heights"]]
        expected = [layout_price(entry, base) for entry in entries[SEEN:] if isinstance(entry, dict)]
        check(len(heights) == len(expected) and all(abs(h - e) <= 1.0 for h, e in zip(heights, expected)),
              f"{self.tag}: the boxes stand {heights}, not their entries' prices {expected} (base {base})")
        check(len(set(expected)) > 1, f"{self.tag}: the replies priced one height {expected}")
        box_top = float(page.evaluate("() => document.querySelector('#pending-replies .pending-reply').getBoundingClientRect().top"))
        mark = len(self.frames()) - 1
        self.shot("boxes")
        self.release()
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {1 + REPLIES} "
                    "&& document.querySelectorAll('#pending-replies .pending-reply').length === 0",
              f"{self.tag}: the bodies did not take the boxes' places")
        settled(page)
        settled(page)
        self.shot("filled")
        trace = self.frames()
        first_top = page.evaluate(f"() => {{ const li = [...document.querySelectorAll('#discord-log > li[data-ids]')]"
                                  f".find((r) => r.getAttribute('data-ids').split(' ').includes({json.dumps(first_new)}));"
                                  " return li ? li.getBoundingClientRect().top : null; }")
        check(first_top is not None and abs(float(first_top) - box_top) <= 0.5,
              f"{self.tag}: the first reply stands at {first_top}, where its box stood at {box_top:.2f}")
        moved = drift(trace, "root", mark)
        # KNOWN, NOT THIS CHANGE'S (`#2391`): the coverage line above the thread changes when the
        # whole thread lands, and with the list at its top nothing can hold the rows through that.
        seam = abs((num(trace[-1], "seam") or 0.0) - (num(trace[mark], "seam") or 0.0))
        own = abs(moved - seam) if num(trace[mark], "scroll") == 0 else moved
        check(own <= 0.5, f"{self.tag}: the fill moved the rows above {moved:.2f} px, {own:.2f} px of it its own "
              f"(coverage line {seam:.2f} px) (rule 370)")
        return (f"{self.tag}: layout sent before the bodies, real route 200 with {len(entries)} entries; boxes "
                f"{[round(h) for h in heights]} = prices (base {base:.0f}); first reply where its box stood "
                f"({abs(float(first_top) - box_top):.2f} px); rows above moved {own:.2f} px of their own "
                f"(coverage line {seam:.2f} px, #2391)")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/vibe-talk",
                        help="the server binary to run (default: this checkout's release build)")
    parser.add_argument("--web-root", type=Path,
                        help="serve this page build instead of the binary's own (the negative control: main's page)")
    parser.add_argument("--browser-executable", help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    binary: Path = args.binary
    if not binary.exists():
        raise SystemExit(f"no server binary at {binary}: cargo build --release first")
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    from playwright.sync_api import sync_playwright
    report: list[str] = []
    with browser_scratch.run_scratch("thread-layout-server-") as work, sync_playwright() as pw:
        for label, (width, height), scheme in (("dark-phone", (412, 915), "dark"), ("light-desk", (1280, 800), "light")):
            mobile = width < 600
            corner = work / label
            corner.mkdir()
            # The message cache refuses a directory other accounts can read or enter.
            corner.chmod(0o700)
            server = Server(corner, binary)
            try:
                def context() -> tuple[BrowserContext, Page]:
                    opened = pw.chromium.launch_persistent_context(
                        str(corner / "profile"), headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                        is_mobile=mobile, has_touch=mobile, color_scheme=scheme,  # type: ignore[arg-type]
                        timezone_id="America/New_York")
                    browser_waits.hot_host_timeouts(opened)
                    page = opened.pages[0] if opened.pages else opened.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)});"
                        " localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
                    if args.web_root is not None:
                        serve_page_from(page, args.web_root)
                    return opened, page

                before, page = context()
                try:
                    earlier_visit(page, server, label)
                finally:
                    before.close()
                server.post_rest()
                server.fill_copy()
                after, page = context()
                try:
                    report.append(Walk(page, server, label, args.screenshots).run())
                finally:
                    after.close()
            except AssertionError:
                print(f"server log tail:\n{server.log_tail()}")
                raise
            finally:
                server.close()
    for line in report:
        print(line)
    print("thread layout server browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
