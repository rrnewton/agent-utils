#!/usr/bin/env python3
"""Back after a link jump lands in the view it left, on the row it left (`#2273`).

`#2273 vibe-talk-cross-view-back-lands-wrong` (owner request 377, 2026-11-25): in
All, a reply's `:email:` short reference to an older message the list already
drew landed it in place but took no Back level, so Back undid an EARLIER jump and
put the reader on that jump's spot: an unrelated older message in another thread,
"Back where you were". The same walk found two more ways a Back missed: coming back
into a thread, the thread's own first-unread entry landing ran over the spot put
back; and the page's keyboard Back (Ctrl+[) walked past a link jump as a visited
place, leaving the jump standing.

This serves the real server (fake chat upstream) seeded with a long channel. Two
threads are injected at the network edge into the flat timeline (the fake upstream
seeds no threads), and a thread-view read is answered from those same rows; link
bodies are injected the same way, as the server renders a link to a message
(`data-msg`, and `data-shortref="email"` for a short reference). Everything else —
sign-in, All, the thread view, the jump, the older-page walk, Back — is the shipped
page and router. Condition waits only; the clocks are hang backstops.

The matrix, each walk from a fresh profile:

  t2a  thread A's reply -> `:email:` to a main message older than All's first window
       (the older-page walk) -> All;
  a2t  an All row -> a link to a thread B reply -> thread B's view;
  t2t  thread A's reply -> a link to a thread B reply -> thread B's view;
  a2a  an UNRELATED earlier jump left on the stack, then thread A's reply in All ->
       `:email:` to a message All already draws -> landed in place (the owner's shape);

each followed by Back by three means: the browser's Back (`page.go_back`), the
system Back (`history.back()`, the traversal a phone's back gesture makes), and the
page's own keyboard Back (Ctrl+[). After Back the view is the one the jump left and
the reader's row stands within 0.5 px of its pre-jump top on the first frame it is
back and on each of the next 45 frames (a per-frame trace, `#2261`'s). At phone
412x915 and desk 1280x800, light and dark (`--schemes`).

Pass --screenshots DIR to keep, per walk, the frame before the tap, the landing and
the frame after Back, with a sidecar naming the row, its text and both tops.
--page DIR serves another page build (a `web/build.mjs` output) in place of the
binary's embedded one, for a negative control; --server BINARY another binary.
"""

from __future__ import annotations

import argparse
import json
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import Page, Route, sync_playwright

import browser_waits
from summary_link_browser import BACK_DUMP, BACK_TRACE_START

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
THREAD_A = "100000000000000771"
THREAD_B = "100000000000000772"
READ_TOKEN = "cross-view-back-check-read-token-not-a-real-credential"
WRITE_TOKEN = "cross-view-back-check-write-token-not-a-real-credential"
SEEDED = 200
LINK_ATTRS = 'target="_blank" rel="noopener noreferrer nofollow"'
EPOCH = datetime(2026, 8, 17, 9, 0, tzinfo=timezone.utc)
# How near its pre-jump top Back must put the reader's row (sub-pixel placement:
# the stricter of the two bars), and for how many drawn frames after the first
# frame it is back it must stay there.
BACK_TOLERANCE_PX = 0.5
BACK_WATCH_FRAMES = 45
CASES = ("t2a", "a2t", "t2t", "a2a")
MEANS = ("browser", "system", "keys")
Json = dict[str, object]


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def call(base: str, method: str, path: str, token: str, body: Json | None = None) -> Json:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=browser_waits.HANG_BACKSTOP_MS / 1000) as res:
        parsed: object = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def all_messages(base: str) -> dict[str, Json]:
    """Every message in the channel by id, read back through the server's own pages."""
    seen: dict[str, Json] = {}
    before: str | None = None
    for _ in range(40):
        path = f"/api/v1/channels/{CHANNEL}/timeline?view=flat&limit=100"
        if before:
            path += f"&before={before}"
        page = call(base, "GET", path, READ_TOKEN)
        messages = page.get("messages")
        assert isinstance(messages, list)
        for message in messages:
            if isinstance(message, dict):
                seen[str(message["id"])] = message
        nxt = page.get("next_before")
        if not messages or not page.get("has_more") or not nxt:
            break
        before = str(nxt)
    return seen


def mark(message: str, thread: str = "") -> str:
    """The `data-msg` the server writes on a link to a message (a reply in `thread`)."""
    return f"discord:{CHANNEL}:{message}:{thread}"


def link_html(target: str, thread: str, text: str, short: bool) -> str:
    """A body as the server renders a link to a message: a short reference's carries its kind."""
    href = f"?c={CHANNEL}&m={mark(target, thread)}"
    extra = ' data-shortref="email"' if short else ""
    return f'<p>see <a href="{href}" data-msg="{mark(target, thread)}"{extra} {LINK_ATTRS}>{text}</a> above</p>\n'


class Fixture:
    """Which ids are which, decided once the server's ids are known (oldest first)."""

    def __init__(self, ids: list[str]) -> None:
        self.ids = ids
        # Thread A, the one the reader is in: a long thread inside All's first window,
        # its members every third message so that authors alternate and rows stay single.
        self.a_root = self.back(46)
        self.a_replies = [self.back(k) for k in range(43, 3, -3)]
        self.a_linker = self.back(22)
        # Thread B, another thread, mostly older than All's first window.
        self.b_root = self.back(150)
        self.b_replies = [self.back(140), self.back(120), self.back(100), self.back(47)]
        # Targets: a main message older than All's first window; one All draws at once.
        self.far = self.back(110)
        self.held = self.back(44)
        # The All row that links into thread B, and the unrelated earlier jump's link and target.
        self.all_linker = self.back(20)
        self.prior_linker = self.back(47)
        self.prior_target = self.back(24)

    def back(self, count: int) -> str:
        return self.ids[len(self.ids) - count]

    def thread_of(self, mid: str) -> tuple[str, str] | None:
        if mid == self.a_root or mid in self.a_replies:
            return THREAD_A, self.a_root
        if mid == self.b_root or mid in self.b_replies:
            return THREAD_B, self.b_root
        return None


class Case:
    """One jump shape: where it starts, the row whose link is tapped, and what that link names."""

    def __init__(self, fx: Fixture, name: str) -> None:
        self.name = name
        self.start = "all" if name in ("a2t", "a2a") else "thread"
        self.prior = name == "a2a"
        self.linker = fx.all_linker if name == "a2t" else fx.a_linker
        self.thread = "" if name in ("t2a", "a2a") else THREAD_B
        self.short = name in ("t2a", "a2a")
        self.target = {"t2a": fx.far, "a2a": fx.held, "a2t": fx.b_replies[-1], "t2t": fx.b_replies[2]}[name]
        self.lands = "flat" if name in ("t2a", "a2a") else f"thread:{THREAD_B}"
        self.view = f"thread:{THREAD_A}" if self.start == "thread" else "flat"


ROW_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log li')]
    .find(r => (r.getAttribute('data-ids') || '').split(' ').includes(id) || r.getAttribute('data-id') === id);
  if (!row) return null;
  const r = row.getBoundingClientRect();
  return {top: r.top, height: r.height, linked: row.getAttribute('data-linked'),
          text: (row.querySelector('.md') || row).textContent.trim().slice(0, 80)};
}"""

VIEW_JS = """() => ({
  view: document.getElementById('thread-select')?.value,
  status: document.getElementById('status')?.textContent || '',
  search: location.search,
  scrollTop: document.getElementById('scroll-area').scrollTop,
})"""

# Resolves once the view, the list and the row stand still for `frames` drawn frames.
STABLE_JS = """([id, frames]) => new Promise((done) => {
  let last = null, same = 0;
  const sig = () => {
    const area = document.getElementById('scroll-area');
    const row = [...document.querySelectorAll('#discord-log li')]
      .find(r => (r.getAttribute('data-ids') || '').split(' ').includes(id) || r.getAttribute('data-id') === id);
    return JSON.stringify([document.getElementById('thread-select')?.value, Math.round(area.scrollTop * 10),
      document.querySelectorAll('#discord-log li[data-id]').length,
      row ? Math.round(row.getBoundingClientRect().top * 10) : null]);
  };
  const step = () => {
    const s = sig();
    if (s === last) same += 1; else { same = 0; last = s; }
    if (same >= frames) done(true); else requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
})"""

LINK_POS_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log li')]
    .find(r => (r.getAttribute('data-ids') || '').split(' ').includes(id));
  const a = row && row.querySelector('a[data-msg]');
  if (!a) return null;
  const r = a.getBoundingClientRect();
  return {x: r.left + Math.min(r.width / 2, 20), y: r.top + r.height / 2, inApp: a.hasAttribute('data-in-app'),
          text: a.textContent};
}"""

PLACE_JS = """([id, f]) => {
  const area = document.getElementById('scroll-area');
  const row = [...document.querySelectorAll('#discord-log li')]
    .find(r => (r.getAttribute('data-ids') || '').split(' ').includes(id));
  area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - area.clientHeight * f;
}"""

# The trace's marks for what Back does: the status line and the view picker as they change.
MARKS_JS = """() => { const t = window.__backTrace; const st = document.getElementById('status');
  new MutationObserver(() => t.mark('status', st.textContent)).observe(st, {childList: true, characterData: true, subtree: true});
  const sel = document.getElementById('thread-select');
  new MutationObserver(() => t.mark('view', sel.value)).observe(sel, {childList: true, attributes: true, subtree: true});
  t.mark('back'); }"""


def number(value: object) -> float | None:
    return float(value) if isinstance(value, (int, float)) else None


class Walk:
    """One reader, one jump, one Back: a fresh profile against the shared server."""

    def __init__(self, page: Page, base: str, fx: Fixture, case: Case, means: str, label: str,
                 phone: bool, page_dir: Path | None, shots: Path | None) -> None:
        self.page = page
        self.base = base
        self.fx = fx
        self.case = case
        self.means = means
        self.label = label
        self.phone = phone
        self.page_dir = page_dir
        self.shots = shots
        self.messages: dict[str, Json] = {}
        self.errors: list[str] = []
        self.page_build: str | None = (
            str(json.loads((page_dir / "assets.json").read_text())["page_build"]) if page_dir else None)

    # --- the network edge -------------------------------------------------------------------

    def dress(self, message: Json) -> Json:
        """A message as this channel has it: alternate authors, thread membership, link bodies."""
        fx, case = self.fx, self.case
        mid = str(message.get("id"))
        index = fx.ids.index(mid) if mid in fx.ids else 0
        odd = index % 2 == 1
        # One minute apart in the order sent: the fake upstream's clock wraps every hour,
        # which would sort the newest messages among older ones.
        at = EPOCH + timedelta(minutes=index)
        dressed: Json = {**message, "author": "Ada" if odd else "Ben",
                         "author_id": "2000000000000000101" if odd else "2000000000000000102",
                         "timestamp": at.isoformat(), "spoken_time": at.strftime("%H:%M")}
        member = fx.thread_of(mid)
        if member:
            thread, root = member
            replies = len(fx.a_replies) if thread == THREAD_A else len(fx.b_replies)
            dressed["thread"] = {"id": thread, "root_message_id": root, "is_root": mid == root,
                                 "reply_count": replies, "reply_count_exact": True}
        if mid == case.linker:
            text = ":email:899" if case.short else "the cited note"
            dressed["content_html"] = link_html(case.target, case.thread, text, case.short)
        if case.prior and mid == fx.prior_linker:
            dressed["content_html"] = link_html(fx.prior_target, "", "the earlier note", False)
        self.messages[mid] = dressed
        return dressed

    def thread_entry(self, thread: str) -> Json | None:
        fx = self.fx
        root = fx.a_root if thread == THREAD_A else fx.b_root
        held = self.messages.get(root)
        if held is None:
            return None
        name = "Thread A" if thread == THREAD_A else "Thread B"
        replies = len(fx.a_replies) if thread == THREAD_A else len(fx.b_replies)
        return {"id": thread, "root": held, "reply_count": replies, "reply_count_exact": True,
                "title": name, "display_name": name, "updated_at": held.get("timestamp")}

    def own_build(self, payload: Json) -> None:
        if self.page_build is not None and "page_build" in payload:
            payload["page_build"] = self.page_build

    def timeline(self, route: Route, every: dict[str, Json]) -> None:
        query = parse_qs(urlsplit(route.request.url).query)
        if (query.get("view") or [""])[0] == "thread":
            thread = (query.get("thread_id") or [""])[0]
            if thread not in (THREAD_A, THREAD_B):
                route.fallback()
                return
            fx = self.fx
            members = [fx.a_root, *fx.a_replies] if thread == THREAD_A else [fx.b_root, *fx.b_replies]
            response = route.fetch(url=f"{self.base}/api/v1/channels/{CHANNEL}/timeline?view=flat&limit=1")
            payload: Json = json.loads(response.text())
            rows = [self.messages.get(mid) or self.dress(dict(every[mid])) for mid in members]
            payload.update({"view": "thread", "thread": self.thread_entry(thread), "messages": rows,
                            "threads": [], "has_threads": True, "has_more": False, "next_before": None,
                            "returned": len(rows)})
            self.own_build(payload)
            route.fulfill(response=response, body=json.dumps(payload))
            return
        response = route.fetch()
        payload = json.loads(response.text())
        got = payload.get("messages")
        if isinstance(got, list):
            payload["messages"] = [self.dress(m) if isinstance(m, dict) else m for m in got]
            payload["has_threads"] = True
            threads = [t for t in (self.thread_entry(THREAD_A), self.thread_entry(THREAD_B)) if t]
            old = payload.get("threads")
            payload["threads"] = threads + [t for t in (old if isinstance(old, list) else [])
                                            if isinstance(t, dict) and str(t.get("id")) not in (THREAD_A, THREAD_B)]
        self.own_build(payload)
        route.fulfill(response=response, body=json.dumps(payload))

    def client_config(self, route: Route) -> None:
        body = call(self.base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        # The fake upstream seeds no threads; the threaded channel this walk stands in is the
        # edge's, so the provider must advertise threading for All and the thread view.
        body["threading_supported"] = True
        providers = body.get("providers")
        if isinstance(providers, list):
            for provider in providers:
                if isinstance(provider, dict):
                    provider["threading_supported"] = True
        self.own_build(body)
        route.fulfill(status=200, content_type="application/json", body=json.dumps(body))

    def serve_page(self) -> None:
        """With --page, the shell names that build's hashed files, and they are answered from it."""
        page_dir = self.page_dir
        if page_dir is None:
            return
        mine = self.page_build
        css = sorted(p.name for p in (page_dir / "assets").glob("voice.*.css"))

        def shell(route: Route) -> None:
            response = route.fetch()
            text = re.sub(r"/assets/voice\.[0-9a-f]+\.js", f"/assets/voice.{mine}.js", response.text())
            if css:
                text = re.sub(r"/assets/voice\.[0-9a-f]+\.css", f"/assets/{css[0]}", text)
            route.fulfill(response=response, body=text)

        def asset(route: Route) -> None:
            name = urlsplit(route.request.url).path.rsplit("/", 1)[-1]
            path = page_dir / "assets" / name
            if not path.is_file():
                route.fulfill(status=404, body="")
                return
            kind = ("text/javascript" if name.endswith(".js") else "text/css" if name.endswith(".css")
                    else "application/json")
            route.fulfill(status=200, content_type=kind, body=path.read_bytes())

        self.page.context.route(lambda url: urlsplit(url).path == "/", shell)
        self.page.context.route("**/assets/voice.*", asset)

    # --- the page ---------------------------------------------------------------------------

    def row(self, mid: str) -> Json | None:
        found: Json | None = self.page.evaluate(ROW_JS, mid)
        return found

    def view(self) -> Json:
        found: Json = self.page.evaluate(VIEW_JS)
        return found

    def stable(self, mid: str, frames: int = 20) -> None:
        self.page.evaluate(STABLE_JS, [mid, frames])

    def until(self, condition: str, what: str) -> None:
        browser_waits.wait_until(self.page, condition, f"{self.label}: {what}")

    def tap_link(self, mid: str) -> None:
        link: Json | None = self.page.evaluate(LINK_POS_JS, mid)
        check(link is not None and bool(link["inApp"]), f"{self.label}: {mid} draws no in-app link: {link}")
        assert link is not None
        x, y = number(link["x"]), number(link["y"])
        assert x is not None and y is not None
        if self.phone:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def open(self, every: dict[str, Json]) -> None:
        page = self.page
        self.serve_page()
        page.on("pageerror", lambda err: self.errors.append(str(err)))
        page.context.route("**/api/v1/client-config", self.client_config)
        page.context.route(f"**/api/v1/channels/{CHANNEL}/timeline*", lambda route: self.timeline(route, every))
        page.context.add_init_script(
            f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
        page.context.add_init_script("try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
        page.goto(self.base + "/", wait_until="load")
        page.wait_for_selector("#view-switch", state="visible")
        if page.locator("#discord-channel").is_hidden():
            browser_waits.click_when_clear(page, "#view-switch")
        page.wait_for_selector("#discord-log > li[data-id]", state="visible")
        self.until("() => document.getElementById('thread-select')?.value === 'flat'", "the channel opening in All")
        check(self.row(self.fx.far) is None, f"{self.label}: the far target is drawn before any walk")

    def into_thread_a(self) -> None:
        """Thread A, as a reader opens it: its root's thread chip in All."""
        fx = self.fx
        self.until(f"() => ({ROW_JS})('{fx.a_root}') !== null", "thread A's root drawn in All")
        self.page.evaluate(PLACE_JS, [fx.a_root, 0.5])
        self.stable(fx.a_root)
        browser_waits.click_when_clear(self.page, f"#discord-log li[data-id='{fx.a_root}'] .thread-badge")
        self.until(f"() => document.getElementById('thread-select')?.value === 'thread:{THREAD_A}'"
                   f" && ({ROW_JS})('{fx.a_linker}') !== null", "thread A open with its linking reply")

    def prior_jump(self) -> None:
        """An unrelated earlier jump, taken and never gone back from."""
        fx = self.fx
        self.until(f"() => ({ROW_JS})('{fx.prior_linker}') !== null", "the earlier jump's link drawn")
        self.page.evaluate(PLACE_JS, [fx.prior_linker, 0.4])
        self.stable(fx.prior_linker)
        self.tap_link(fx.prior_linker)
        self.until(f"() => ({ROW_JS})('{fx.prior_target}')?.linked === 'true'", "the earlier jump landed")
        self.stable(fx.prior_target)
        self.until(f"() => ({ROW_JS})('{self.case.linker}') !== null", "the linking reply drawn in All")

    def back(self) -> None:
        if self.means == "browser":
            self.page.go_back(wait_until="commit")
        elif self.means == "system":
            self.page.evaluate("() => history.back()")
        else:
            self.page.keyboard.press("Control+BracketLeft")

    def run(self, every: dict[str, Json]) -> Json:
        case = self.case
        self.open(every)
        if case.prior:
            self.prior_jump()
        if case.start == "thread":
            self.into_thread_a()
        # The reader on the linking row, a third of the way down, still. (All draws its
        # first window in steps, so the row is waited for before it is scrolled to.)
        self.until(f"() => ({ROW_JS})('{case.linker}') !== null", "the linking row drawn")
        self.stable(case.linker)
        self.page.evaluate(PLACE_JS, [case.linker, 0.33])
        self.stable(case.linker, 30)
        before = self.row(case.linker)
        before_view = self.view()
        check(before is not None and before_view["view"] == case.view,
              f"{self.label}: not on the linking row in {case.view}: {before_view} {before}")
        assert before is not None
        expected = number(before["top"])
        assert expected is not None
        self.shot("1-before-tap")
        self.tap_link(case.linker)
        self.until(f"() => ({ROW_JS})('{case.target}')?.linked === 'true'"
                   f" && document.getElementById('thread-select')?.value === '{case.lands}'",
                   f"the jump landing on {case.target} in {case.lands}")
        self.stable(case.target)
        landed_view = self.view()
        self.shot("2-landed")

        self.page.evaluate(BACK_TRACE_START, case.linker)
        self.page.evaluate(MARKS_JS)
        self.back()
        # Back is over once the view and the row have stood still for the frames watched.
        self.stable(case.linker, BACK_WATCH_FRAMES)
        after = self.row(case.linker)
        after_view = self.view()
        self.shot("3-after-back")
        dump: Json = self.page.evaluate(BACK_DUMP, [case.linker, expected])
        changed = dump.get("trace_frames_changed")
        frames = [f for f in (changed if isinstance(changed, list) else []) if isinstance(f, list)]
        total = int(str(dump.get("trace_frames_total", 0)))
        back_at: int | None = None
        worst = 0.0
        for frame in frames:
            top = number(frame[2])
            if back_at is None:
                if top is not None and abs(top - expected) <= BACK_TOLERANCE_PX:
                    back_at = int(frame[0])
                continue
            worst = max(worst, abs(top - expected) if top is not None else float("inf"))
        after_top = number(after["top"]) if after else None
        problems: list[str] = []
        if after_view["view"] != case.view:
            problems.append(f"Back left the reader in {after_view['view']}, not {case.view}")
        if after_top is None:
            problems.append("the linking row is not drawn after Back")
        else:
            worst = max(worst, abs(after_top - expected))
            if abs(after_top - expected) > BACK_TOLERANCE_PX:
                problems.append(f"the linking row stands at {after_top:.2f}, {abs(after_top - expected):.2f} px "
                                f"from {expected:.2f}")
        if back_at is None:
            problems.append("no frame put the row back within the tolerance")
        elif total - back_at < BACK_WATCH_FRAMES:
            problems.append(f"only {total - back_at} frames watched after the row was back")
        elif worst > BACK_TOLERANCE_PX:
            problems.append(f"the row moved {worst:.2f} px after it was back")
        if after_view["status"] != "Back where you were.":
            problems.append(f"the status says {after_view['status']!r}")
        if self.errors:
            problems.append(f"the page threw: {self.errors}")
        result: Json = {
            "label": self.label, "ok": not problems, "problems": problems,
            "row": case.linker, "row_text": before.get("text"), "before_top": expected, "after_top": after_top,
            "max_drift_px": round(worst, 3), "frames_after_back": (total - back_at) if back_at is not None else 0,
            "before_view": before_view, "landed_view": landed_view, "after_view": after_view,
        }
        if self.shots is not None:
            (self.shots / f"{self.label}-sidecar.json").write_text(json.dumps(result, indent=1))
            (self.shots / f"{self.label}-trace.json").write_text(json.dumps(dump, indent=1))
        elif problems:
            result["dump"] = dump
        return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/vibe-talk",
                        help="the vibe-talk binary to serve (default: this checkout's release build)")
    parser.add_argument("--page", type=Path, default=None,
                        help="serve this page build (a web/build.mjs output) in place of the embedded one")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="directory for each walk's three frames, sidecar and Back trace (default: none)")
    parser.add_argument("--profiles", nargs="+", default=["phone-412", "desk-1280"], choices=["phone-412", "desk-1280"])
    parser.add_argument("--schemes", nargs="+", default=["light", "dark"], choices=["light", "dark"])
    parser.add_argument("--cases", nargs="+", default=list(CASES), choices=list(CASES))
    parser.add_argument("--means", nargs="+", default=list(MEANS), choices=list(MEANS))
    args = parser.parse_args()
    shots: Path | None = args.screenshots
    if shots is not None:
        shots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    results: list[Json] = []
    with tempfile.TemporaryDirectory(prefix="cross-view-back-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "cross-view-back-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
        )
        server = subprocess.Popen(
            [str(args.server.resolve()), "--config", str(work / "vibe-talk.toml"), "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + browser_waits.HANG_BACKSTOP_MS / 1000
            while True:
                try:
                    call(base, "GET", "/healthz", READ_TOKEN)
                    break
                except Exception:
                    check(server.poll() is None and time.monotonic() < deadline, "the server did not come up")
                    time.sleep(0.25)
            for i in range(SEEDED):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"seed message {i} with enough words to take a row in the list"})
            every = all_messages(base)
            ids = sorted(every, key=int)
            check(len(ids) >= SEEDED, f"seeded {SEEDED} messages, read back {len(ids)}")
            fx = Fixture(ids)
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                for profile in args.profiles:
                    phone = profile == "phone-412"
                    for scheme in args.schemes:
                        for name in args.cases:
                            for means in args.means:
                                label = f"{profile}-{scheme}-{name}-{means}"
                                if phone:
                                    context = browser.new_context(
                                        viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
                                        device_scale_factor=2.625, color_scheme=scheme)
                                else:
                                    context = browser.new_context(viewport={"width": 1280, "height": 800},
                                                                  color_scheme=scheme)
                                browser_waits.hot_host_timeouts(context)
                                try:
                                    walk = Walk(context.new_page(), base, fx, Case(fx, name), means, label, phone,
                                                args.page, shots)
                                    result = walk.run(every)
                                except Exception as error:  # noqa: BLE001 - one walk's failure is reported with the rest
                                    result = {"label": label, "ok": False,
                                              "problems": [f"{type(error).__name__}: {error}"]}
                                finally:
                                    context.close()
                                results.append(result)
                                problems = result.get("problems")
                                verdict = "ok" if result["ok"] else "FAIL " + "; ".join(
                                    map(str, problems if isinstance(problems, list) else []))
                                print(f"{label}: {verdict} (drift {result.get('max_drift_px')} px)", flush=True)
                browser.close()
        finally:
            server.terminate()
            server.wait()
    failed = [r for r in results if not r["ok"]]
    for result in failed:
        if "dump" in result:
            print(f"BACK DUMP {result['label']}: {json.dumps(result['dump'])}")
    print(f"cross-view back: {len(results) - len(failed)} of {len(results)} walks came back to the view and the row "
          "they left")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
