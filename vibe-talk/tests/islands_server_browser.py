#!/usr/bin/env python3
"""Islands fill through the REAL server's `around` route (`#2426 vibe-talk-sparse-loading-program`, R2).

islands_browser.py walks the page's islands against a fake API; this walk proves the reads that fill
a seam reach the shipped server binary's own timeline route, which answers them from the server's
own copy of the channel.

The server is the release binary on a loopback port with `--fake-discord` as its only fake (a
throwaway store and message cache, serving on, `backfill = "full"`), one for both screen sizes.
Through the test-only fake-seed route it seeds 1,200 top-level messages half a minute apart. The
server's own full-history walk, asked to read the channel next, brings them into its copy (a
caller's step back is rows only; a walk proves its stretch page by page); the walk waits until the
server says it holds the whole history and the real route answers around reads of every stretch
whole (conditions, not clocks).

The view is Main: the server's copy keeps Main's order, so an around read there answers a stretch
(Discord's All keeps none, and answers the target alone). At 412x915 (dark, touch) and 1280x800
(light) it asserts, by counting:

  stepping back with the "Older messages" control breaks the reader's island off from the newest
  window: exactly one seam stands, saying "N messages not loaded", and the rows held never pass the
  newest window + the island + two pages + 30 (150/150 on a phone, 200/200 on a desk);
  Newest lands on the newest message with ZERO timeline requests (counted at the page's route);
  scrolling up from the newest to the seam fills it through the real route: every one of the page's
  `around` reads (below=50, above=0) is answered 200 with `around.held` true, and the seam narrows
  until it closes with no hole; the row being read moves at most 1.0 px on every frame of every fill.

Positioning: the scroll owner re-asserts its held row for a while after each landing and puts back
a programmatic scroll write made inside that window, so the walk waits until nothing has written the
list's scroll position for 12 frames before it scrolls (as islands_browser.py does).

Negative control: `--web-root DIR` serves another page build (main before R2) over the same real
server; no seam ever stands, and the walk fails.

Needs a release build (`cargo build --release`); `--binary PATH` runs another. `--screenshots DIR`
keeps a PNG per stage.
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
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, NamedTuple

import browser_scratch
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Request, Response, Route

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "islands-server-read-token-not-a-real-credential"
WRITE_TOKEN = "islands-server-write-token-not-a-real-credential"
THREAD_ID = "walk-islands-thread"
TOTAL = 1200
PAGE = 50
FILL = ("a channel message long enough to wrap over a couple of lines on a phone, as the owner's lists "
        "look when several agents report at once. ")
# Steps back taken past the first break-off, so later break-offs join the one seam.
STEPS_PAST_BREAK = 3
MAX_STEPS = 18
# Generous condition timeouts for a hot host (rule 346): every wait ends on its condition.
WAIT_S = 90.0
# The server's walk of the whole history, at 50 reads a minute: the hang backstop for it.
HISTORY_WAIT_S = 600.0
TIMELINE = re.compile(r"/api/v1/channels/[^/]+/timeline\?")

Json = dict[str, object]

CENSUS_JS = """() => {
  const list = document.getElementById('discord-log');
  const ids = [...list.querySelectorAll(':scope > li[data-id]')].flatMap((li) =>
    (li.getAttribute('data-ids') || li.getAttribute('data-id') || '').split(' ').filter(Boolean));
  const seams = [...list.querySelectorAll(':scope > li.island-gap')].map((li) => li.textContent.trim());
  return { ids, seams };
}"""

# The row being read: the first message row whose top is at or below the band's top.
READING_JS = """() => {
  const area = document.getElementById('scroll-area');
  const top = area.getBoundingClientRect().top;
  for (const li of document.querySelectorAll('#discord-log > li[data-id]')) {
    const r = li.getBoundingClientRect();
    if (r.top >= top - 0.5 && r.height > 0) return { id: li.getAttribute('data-id'), top: r.top };
  }
  return null;
}"""

# Every frame's top of one row while armed, to judge a fill by its worst frame.
ARM_JS = """(id) => { window.__islandProbe = { id, tops: [], on: true };
  const sample = () => { const p = window.__islandProbe; if (!p.on) return;
    const li = document.querySelector(`#discord-log > li[data-id="${p.id}"]`);
    p.tops.push(li ? li.getBoundingClientRect().top : null); requestAnimationFrame(sample); };
  requestAnimationFrame(sample); }"""
DISARM_JS = "() => { const p = window.__islandProbe; p.on = false; return p.tops; }"

# Counts every write to the list's scroll position, for `quiet`.
WRITES_JS = r"""() => { if (window.__scrollWrites !== undefined) return; window.__scrollWrites = 0;
  const area = document.getElementById('scroll-area');
  const desc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
  Object.defineProperty(area, 'scrollTop', { configurable: true,
    get() { return desc.get.call(this); },
    set(v) { window.__scrollWrites += 1; desc.set.call(this, v); } }); }"""

SEAM_TOP_JS = ("() => { const s = document.querySelector('#discord-log > li.island-gap'); "
               "return s ? s.getBoundingClientRect().top : null; }")
AT_FOOT_JS = ("() => { const a = document.getElementById('scroll-area'); "
              "return a.scrollHeight - a.scrollTop - a.clientHeight < 4; }")
SCROLL_BY_JS = ("(dy) => { const a = document.getElementById('scroll-area'); a.scrollTop += dy; "
                "a.dispatchEvent(new Event('scroll')); }")
NOT_LOADED = re.compile(r"^(\d+) messages? not loaded$")


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


def frames(page: Page, n: int = 2) -> None:
    for _ in range(n):
        page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")


def pump(page: Page, condition: Callable[[], bool], why: str) -> None:
    """Frames until `condition()` holds, with the hang backstop as the deadline."""
    deadline = time.monotonic() + WAIT_S
    while not condition():
        check(time.monotonic() < deadline, why)
        frames(page, 1)


def settle(page: Page) -> None:
    """Until the list's ids and seams stop changing for a few frames (a step's draw and its break-off)."""
    last = None
    still = 0
    for _ in range(400):
        now = page.evaluate(CENSUS_JS)
        key = (len(now["ids"]), tuple(now["seams"]))
        if key == last:
            still += 1
            if still >= 4:
                return
        else:
            still = 0
        last = key
        frames(page, 1)
    raise AssertionError("the list never settled")


def quiet(page: Page) -> None:
    """Until nothing has written the list's scroll position for 12 frames: after a landing the scroll
    owner re-asserts the row it held for a while and puts back a programmatic write made inside that
    window. The walk positions after it."""
    page.evaluate(WRITES_JS)
    last = -1
    still = 0
    for _ in range(600):
        count = int(page.evaluate("() => window.__scrollWrites"))
        if count == last:
            still += 1
            if still >= 12:
                return
        else:
            still = 0
        last = count
        frames(page, 1)


def worst_move(tops: list[float | None], baseline: float) -> float:
    present = [top for top in tops if top is not None]
    check(len(present) == len(tops), "the row being read left the list mid-fill")
    return max((abs(top - baseline) for top in present), default=0.0)


def query_of(url: str) -> dict[str, list[str]]:
    return urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)


def is_around(url: str) -> bool:
    return "around" in query_of(url)


class Census(NamedTuple):
    """The message ids the list holds, oldest first, and the words of each seam standing in it."""

    ids: list[str]
    seams: list[str]


class Server:
    """One throwaway release server with `--fake-discord`, its seeded channel and its base URL."""

    def __init__(self, work: Path, binary: Path) -> None:
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.work = work
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{self.port}"\n'
            '[discord]\nbot_token = "islands-server-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            # The whole history, walked at the fastest pace the setting allows.
            f'[message_cache]\nenabled = true\nserve = true\nbackfill = "full"\nfull_reads_per_minute = 50\n'
            f'path = "{work}/cache.sqlite3"\n')
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
        now = int(time.time() * 1000)
        # Oldest first, half a minute apart, the newest half a minute ago. The oldest heads a thread:
        # the fake says its channel has threads once one is seeded, and the page keeps its islands
        # (and reads the timeline route) only for a channel that has them. Main lists the root.
        self.ids = [self.seed({"author": "release-lane", "text": f"Message {k}. " + FILL * (1 + k % 3),
                               "at_ms": now - (TOTAL - k) * 30_000,
                               **({"thread_id": THREAD_ID, "thread_root": True} if k == 0 else {})})
                    for k in range(TOTAL)]

    def seed(self, body: Json) -> str:
        answer = call(self.base, "POST", f"/api/v1/channels/{CHANNEL}/fake-seed", WRITE_TOKEN, body)
        return str(answer["id"])

    def timeline(self, query: str, token: str) -> Json:
        return call(self.base, "GET", f"/api/v1/channels/{CHANNEL}/timeline?{query}", token)

    def fill_copy(self) -> str:
        """Bring the channel into the server's copy by its own full-history walk (asked to read this
        channel next), wait until the server says it holds the whole history, and then until the real
        route answers around reads of every stretch whole: every 100th message with 50 each way, in
        order, up to the newest window (which the page reads as its newest pages)."""
        call(self.base, "POST", f"/api/v1/channels/{CHANNEL}/history/prioritize", WRITE_TOKEN)
        deadline = time.monotonic() + HISTORY_WAIT_S
        while True:
            history = call(self.base, "GET", f"/api/v1/channels/{CHANNEL}/history", READ_TOKEN)
            if history.get("complete") is True:
                break
            check(time.monotonic() < deadline, f"the server never held the channel's whole history: {history}")
            time.sleep(0.25)
        targets = range(PAGE, TOTAL - 2 * PAGE, 2 * PAGE)
        for at in targets:
            want = self.ids[at - PAGE:at + PAGE + 1]
            deadline = time.monotonic() + WAIT_S
            while True:
                answer = self.timeline(f"view=main&around={self.ids[at]}&below={PAGE}&above={PAGE}", READ_TOKEN)
                messages = answer.get("messages")
                got = [str(m.get("id")) for m in messages if isinstance(m, dict)] if isinstance(messages, list) else []
                around = answer.get("around")
                if isinstance(around, dict) and around.get("held") is True and got == want:
                    break
                check(time.monotonic() < deadline,
                      f"the server's copy never answered the stretch around message {at} whole: {around}, {len(got)} messages")
                time.sleep(0.25)
        return (f"the server holds the channel's whole history; around reads answer its oldest "
                f"{targets[-1] + PAGE + 1} messages whole, {len(targets)} stretches")

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


def serve_page_from(page: Page, web_root: Path) -> None:
    """Serve every non-API path from the page build in `web_root` (another page over the same server)."""
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
    """One browser corner: every timeline request the page sends, in order, and its around answers;
    an around read can be held at the page while the walk notes the row being read."""

    def __init__(self, page: Page, server: Server, tag: str, shots: Path | None) -> None:
        self.page, self.server, self.tag, self.shots = page, server, tag, shots
        self.sent: list[str] = []
        self.finished = 0
        self.answers: list[tuple[str, int, object]] = []
        self.holding = False
        self.held: list[Route] = []
        # Whether the per-frame probe is armed, and how many around reads went out while it was.
        self.armed = False
        self.under_probe = 0
        self.errors: list[str] = []
        page.on("request", self.on_request)
        page.on("requestfinished", self.on_finished)
        page.on("requestfailed", self.on_finished)
        page.on("response", self.on_response)
        page.on("pageerror", lambda error: self.errors.append(str(error)))
        page.route(TIMELINE, self.hold_around)

    def on_request(self, request: Request) -> None:
        if TIMELINE.search(request.url):
            self.sent.append(request.url)

    def on_finished(self, request: Request) -> None:
        if TIMELINE.search(request.url):
            self.finished += 1

    def on_response(self, response: Response) -> None:
        if TIMELINE.search(response.url) and is_around(response.url):
            try:
                body = response.json()
            except Exception:
                body = None
            self.answers.append((response.url, response.status, body.get("around") if isinstance(body, dict) else None))

    def hold_around(self, route: Route) -> None:
        around = is_around(route.request.url)
        if self.holding and around:
            self.held.append(route)
            return
        if around and self.armed:
            self.under_probe += 1
        route.fallback()

    def release(self) -> None:
        self.holding = False
        if self.armed:
            self.under_probe += len(self.held)
        for route in self.held:
            route.fallback()
        self.held.clear()

    def landed(self) -> None:
        """Every timeline request sent has finished, and the list has settled after them."""
        pump(self.page, lambda: self.finished >= len(self.sent), f"{self.tag}: a timeline read never finished")
        settle(self.page)

    def census(self) -> Census:
        raw = self.page.evaluate(CENSUS_JS)
        return Census([str(one) for one in raw["ids"]], [str(one) for one in raw["seams"]])

    def shot(self, stage: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.tag}-{stage}.png"))

    def run(self) -> list[str]:
        page, tag = self.page, self.tag
        notes: list[str] = []
        page.goto(f"{self.server.base}/?c={CHANNEL}&v=main", wait_until="load")
        browser_waits.wait_until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 5",
                                 f"{tag}: the channel did not open")
        view = page.evaluate("() => { const s = document.getElementById('thread-select'); return s ? s.value : null; }")
        check(view == "main", f"{tag}: the deep link opened view {view!r}, not Main")
        self.landed()
        phone = bool(page.evaluate("() => (document.getElementById('scroll-area').clientWidth || 0) < 600"))
        newest_rows, island_rows = (150, 150) if phone else (200, 200)
        limit = newest_rows + island_rows + 2 * PAGE + 30

        # 1. Step back with the "Older messages" control until the island breaks off, and on a little.
        broke_at = None
        most = 0
        for step in range(MAX_STEPS):
            asked = len(self.sent)
            page.locator("#load-older").click()
            pump(page, lambda: len(self.sent) > asked, f"{tag}: step {step} back asked for no older page")
            self.landed()
            census = self.census()
            held = len(census.ids)
            most = max(most, held)
            check(held <= limit, f"{tag}: step {step}: {held} messages held, over the budgets ({limit}), "
                                 f"{len(census.seams)} seams standing")
            if census.seams and broke_at is None:
                broke_at = step
            if broke_at is not None and step >= broke_at + STEPS_PAST_BREAK:
                break
        census = self.census()
        check(broke_at is not None, f"{tag}: {MAX_STEPS} steps back ({len(census.ids)} held) never broke off: no seam stood")
        check(len(census.seams) == 1, f"{tag}: {len(census.seams)} seams stand, not one: {census.seams}")
        seam = NOT_LOADED.match(census.seams[0])
        check(seam is not None, f"{tag}: the seam says {census.seams[0]!r}")
        assert seam is not None
        index = {one: at for at, one in enumerate(self.server.ids)}
        held_ids = [one for one in census.ids if one in index]
        check([index[one] for one in held_ids] == sorted(index[one] for one in held_ids), f"{tag}: held rows out of order")
        notes.append(f"scroll-back: broke off at step {broke_at}, {len(census.ids)} held (most {most}, limit {limit}), "
                     f"one seam {census.seams[0]!r}")
        self.shot("1-far-back")

        # 2. Newest from the far-back island: no timeline request, the newest message at the foot.
        page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight * 0.1; "
                      "a.dispatchEvent(new Event('scroll')); }")
        frames(page, 3)
        quiet(page)
        self.landed()
        before = len(self.sent)
        jump = page.locator("#jump-newest")
        if jump.is_visible():
            jump.click()
        else:
            page.keyboard.press("End")
        browser_waits.wait_until(page, AT_FOOT_JS, f"{tag}: Newest did not reach the foot")
        settle(page)
        last = page.evaluate("() => { const rows = document.querySelectorAll('#discord-log > li[data-id]'); "
                             "return rows.length ? rows[rows.length - 1].getAttribute('data-id') : null; }")
        check(last == self.server.ids[-1], f"{tag}: the newest row is {last}, not {self.server.ids[-1]}")
        newest_reads = self.sent[before:]
        check(not newest_reads, f"{tag}: Newest sent {len(newest_reads)} timeline requests: {newest_reads[:3]}")
        census = self.census()
        check(len(census.seams) == 1, f"{tag}: at the newest, seams {census.seams}")
        seam = NOT_LOADED.match(census.seams[0])
        assert seam is not None
        before_fill = int(seam.group(1))
        notes.append("Newest from far back: at the newest message with 0 timeline requests")
        self.shot("2-newest")

        # 3. Up from the newest to the seam: it fills through the real route from the newer side. Every
        # around read is held at the page until the row being read is noted and the per-frame probe
        # armed, whether the walk's scroll asked for it or the page did after a landing; one that goes
        # out while a landing is still settling lands under that landing's probe.
        start = len(self.sent)
        answers_from = len(self.answers)
        counts = [before_fill]
        worst = 0.0
        fills = 0
        self.holding = True
        for _ in range(60):
            census = self.census()
            if not census.seams and not self.held:
                break
            if not self.held:
                quiet(page)
            if not self.held:
                seam_top = page.evaluate(SEAM_TOP_JS)
                check(seam_top is not None, f"{tag}: the seam left the list")
                area_top = page.evaluate("() => document.getElementById('scroll-area').getBoundingClientRect().top")
                # The seam just above the band, the reader below it: it fills from below.
                page.evaluate(SCROLL_BY_JS, (seam_top - area_top) - 200)
                waited = 0
                while not self.held and waited < 300:
                    frames(page, 1)
                    waited += 1
                if not self.held:
                    continue
            frames(page, 2)
            reading = page.evaluate(READING_JS)
            check(reading is not None, f"{tag}: no row on screen below the seam")
            page.evaluate(ARM_JS, reading["id"])
            self.armed = True
            self.release()
            self.landed()
            frames(page, 2)
            worst = max(worst, worst_move(page.evaluate(DISARM_JS), reading["top"]))
            self.armed = False
            self.holding = True
            fills += 1
            census = self.census()
            if census.seams:
                now = NOT_LOADED.match(census.seams[0])
                counts.append(int(now.group(1)) if now else -1)
        self.release()
        arounds = [url for url in self.sent[start:] if is_around(url)]
        answers = self.answers[answers_from:]
        check(bool(arounds), f"{tag}: reaching the seam sent no around read")
        asks = sorted({(query_of(url).get("below", [""])[0], query_of(url).get("above", [""])[0]) for url in arounds})
        check(asks == [(str(PAGE), "0")], f"{tag}: the fills from the newer side asked (below, above) {asks}")
        check(len(answers) == len(arounds), f"{tag}: {len(arounds)} around reads sent, {len(answers)} answered")
        check(self.under_probe == len(arounds), f"{tag}: {len(arounds)} around reads, {self.under_probe} under the probe")
        refused = [(status, around) for _, status, around in answers
                   if status != 200 or not isinstance(around, dict) or around.get("held") is not True]
        check(not refused, f"{tag}: the real route answered around reads {refused[:3]}")
        census = self.census()
        check(not census.seams, f"{tag}: the seam never closed: {census.seams} (counts {counts})")
        held_ids = [one for one in census.ids if one in index]
        positions = [index[one] for one in held_ids]
        check(positions == list(range(positions[0], positions[0] + len(positions))),
              f"{tag}: a hole stands where the seam closed ({len(positions)} rows, {positions[0]}..{positions[-1]})")
        check(positions[-1] == TOTAL - 1, f"{tag}: the newest message left the list during the fills")
        check(worst <= 1.0, f"{tag}: a fill moved the row being read {worst:.2f} px")
        check(not self.errors, f"{tag}: the page threw: {self.errors}")
        notes.append(f"up to the seam: {len(arounds)} around reads (below={PAGE}, above=0), each answered 200 held by the "
                     f"real route; seam {' -> '.join(str(c) for c in counts)} -> closed, no hole; "
                     f"row being read moved at most {worst:.2f} px on any frame, every read under the probe ({fills} landings)")
        self.shot("3-filled")
        return [f"{tag}: {note}" for note in notes]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/vibe-talk",
                        help="the server binary to run (default: this checkout's release build)")
    parser.add_argument("--web-root", type=Path,
                        help="serve this page build instead of the binary's own (the negative control: main's page)")
    parser.add_argument("--browser-executable", help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    parser.add_argument("--scratch-root", type=Path, default=browser_scratch.SCRATCH_ROOT,
                        help="where the run's servers and browser profiles live (default: target/test-scratch)")
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
    with browser_scratch.run_scratch("islands-server-", root=args.scratch_root) as work, sync_playwright() as pw:
        # The message cache refuses a directory other accounts can read or enter.
        (work / "server").mkdir(mode=0o700)
        server = Server(work / "server", binary)
        try:
            report.append(server.fill_copy())
            for label, (width, height), scheme in (("dark-phone", (412, 915), "dark"), ("light-desk", (1280, 800), "light")):
                mobile = width < 600
                context: BrowserContext = pw.chromium.launch_persistent_context(
                    str(work / f"profile-{label}"), headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                    is_mobile=mobile, has_touch=mobile, color_scheme=scheme,  # type: ignore[arg-type]
                    timezone_id="America/New_York")
                try:
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0] if context.pages else context.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)});"
                        " localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
                    if args.web_root is not None:
                        serve_page_from(page, args.web_root)
                    report += Walk(page, server, label, args.screenshots).run()
                finally:
                    context.close()
        except AssertionError:
            print(f"server log tail:\n{server.log_tail()}")
            raise
        finally:
            server.close()
    for line in report:
        print(line)
    print("islands server browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
