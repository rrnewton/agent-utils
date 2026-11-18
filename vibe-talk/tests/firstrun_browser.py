#!/usr/bin/env python3
"""First run in real Chromium (`#1781 vibe-talk-first-run-welcome-channel-default`).

The owner's asks, as the browser lives them:

  * a fresh sign-in (no saved record, no dismissed mark) lands on the Welcome screen, and
    dismissing it lands on the Channel view — the app opens on the Channel, not the call;
  * the Welcome never comes back unasked: a reload after dismissing stays on the Channel,
    and a saved Voice choice still opens on Voice with no Welcome at all;
  * the Welcome re-opens from Settings (`#open-welcome`) and from Help (`#welcome-from-help`);
  * with no channel configured the Welcome is the guided setup — the config steps and a
    copyable snippet — and Check again, once the (fake) server has a channel, lands on it;
  * a deep link (`?v=docs`) still wins over the default.

At a phone (412x915, scale 2.6) and a desk (1280x800), in light and dark, with the on-screen
keyboard emulated as a visualViewport shrink (the stand-in from keyboard_hides_bar_browser)
for the keyboard-open shots, and the Discuss half-pane open in the phone shots. Screenshots
of every state go to --screenshots DIR when given: the before (sign-in form) and after
(Welcome, then Channel) states included, so the liaison can review the exact fix.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import threading
from dataclasses import dataclass, field
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import CHANNEL, TOKEN, Json, check
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

PHONE = {"width": 412, "height": 915}
DESK = {"width": 1280, "height": 800}
# The visual viewport an open keyboard leaves on the phone (keys + suggestion strip).
KEYBOARD_LEFT_PX = 500

VIEWPORT_JS = """(() => {
    const real = window.visualViewport;
    const events = new EventTarget();
    const sim = {height: null};
    const fake = {
        get width() { return real.width; },
        get height() { return sim.height === null ? real.height : sim.height; },
        get offsetTop() { return 0; },
        get offsetLeft() { return 0; },
        get pageTop() { return window.scrollY; },
        get pageLeft() { return window.scrollX; },
        get scale() { return real.scale; },
        addEventListener: (type, fn, options) => events.addEventListener(type, fn, options),
        removeEventListener: (type, fn, options) => events.removeEventListener(type, fn, options),
    };
    Object.defineProperty(window, 'visualViewport', {configurable: true, get: () => fake});
    window.__keyboard = (height) => { sim.height = height; events.dispatchEvent(new Event('resize')); };
    window.__keyboardClose = () => { sim.height = null; events.dispatchEvent(new Event('resize')); };
})();"""

WELCOME_HEADINGS = ["Channel", "Threads", "Discuss", "Documents", "Voice",
                    "Read / Pace", "Done / Undo", "Search"]


@dataclass
class FirstRunApi(kb.FakeApi):
    """The fixture's channels, or none until the walk adds one, as a config change would."""

    listed: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        with self.lock:
            return {**super().client_config(), "channels": list(self.listed),
                    "discuss_available": True}


def serve(api: FirstRunApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=None,
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per state in (default: none kept)")
    return parser.parse_args()


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"firstrun-{name}.png"))


def on(page: Page, element: str) -> bool:
    return bool(page.evaluate(f"() => !document.getElementById('{element}').hidden"))


def sign_in(page: Page, url: str) -> None:
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")


def expect_welcome(page: Page, where: str) -> None:
    page.wait_for_function("() => !document.getElementById('screen-welcome').hidden",
                           timeout=10_000)
    # `#1837 vibe-talk-status-pill-avoids-text`: while the Welcome is up the status
    # pill waits rather than cover its text (on a desk, the foot hint). The pill may
    # be unhidden with "Ready." behind it; it must not be drawn.
    page.wait_for_function(
        "() => getComputedStyle(document.getElementById('status-line')).visibility === 'hidden'",
        timeout=10_000)
    check(page.evaluate("""() => {
        const pill = document.getElementById('status-line').getBoundingClientRect();
        const hint = [...document.querySelectorAll('#screen-welcome .hint')]
            .map((el) => el.getBoundingClientRect())
            .find((r) => r.height > 0);
        if (!hint) return true;
        return pill.bottom <= hint.top || pill.top >= hint.bottom
            || getComputedStyle(document.getElementById('status-line')).visibility === 'hidden';
    }"""), f"{where}: the status pill is drawn over the Welcome")
    headings = page.evaluate("""() => [...document.querySelectorAll('#screen-welcome .welcome-item dt')]
        .map((dt) => dt.textContent.trim())""")
    check(headings == WELCOME_HEADINGS, f"{where}: the Welcome names {headings}, not {WELCOME_HEADINGS}")
    check(on(page, "screen-welcome") and not on(page, "screen-main"),
          f"{where}: the Welcome is not the screen showing")


def expect_channel(page: Page, where: str) -> None:
    page.wait_for_function("""() => !document.getElementById('screen-main').hidden
        && !document.getElementById('pane-discord').hidden
        && document.getElementById('pane-voice').hidden""", timeout=10_000)
    label = page.evaluate("() => document.getElementById('view-switch-label').textContent")
    check(label == "Channel", f"{where}: the view switch says {label!r}, not 'Channel'")
    page.wait_for_selector("#discord-log > li[data-id]", state="visible", timeout=10_000)
    at_bottom = page.evaluate("""() => { const a = document.getElementById('scroll-area');
        return a.scrollTop >= a.scrollHeight - a.clientHeight - 2; }""")
    check(at_bottom, f"{where}: the Channel did not open at its newest message")


def dismiss(page: Page, where: str) -> None:
    page.click("#welcome-start")
    expect_channel(page, where)
    seen = page.evaluate("() => localStorage.getItem('vibe-talk.welcome.dismissed')")
    check(seen == "1", f"{where}: dismissing the Welcome left no mark (got {seen!r})")


def open_discuss(page: Page, where: str) -> None:
    """The Discuss half-pane open, as the screenshot matrix asks of the phone shots."""
    if page.evaluate("() => { const b = document.getElementById('discuss-open'); return !!b && !b.hidden; }"):
        page.click("#discuss-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=10_000)


def walk_fresh(context: BrowserContext, url: str, shots: Path | None, where: str,
               scheme: str) -> None:
    """First run with a channel configured: Welcome once, then Channel forever."""
    page = context.pages[0]
    shot(page, shots, f"{where}-{scheme}-01-signin-before")
    sign_in(page, url)
    expect_welcome(page, where)
    check(not on(page, "welcome-guide"), f"{where}: the guided setup shows though a channel is configured")
    shot(page, shots, f"{where}-{scheme}-02-welcome-after")
    dismiss(page, where)
    # `#1837`: back on the Channel the pill draws again when there is something to say.
    page.evaluate("""() => {
        document.getElementById('status').textContent = 'Ready.';
        document.getElementById('status-line').hidden = false;
    }""")
    page.wait_for_function(
        "() => getComputedStyle(document.getElementById('status-line')).visibility === 'visible'",
        timeout=10_000)
    page.evaluate("() => { document.getElementById('status-line').hidden = true; }")
    # `#1837` (b): the current freshness pill takes itself away, so at the head of the
    # list the first card's text and top-row buttons are left clear (app landings
    # already measure the pill while it is up, `floatingClearance`); and while the
    # reader sits mid-list, it taking itself away moves nothing.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = 0; }")
    mid = page.evaluate("""() => {
        const area = document.getElementById('scroll-area');
        area.scrollTop = Math.min(300, Math.max(0, area.scrollHeight - area.clientHeight));
        return area.scrollTop;
    }""")
    if mid > 0:
        page.wait_for_function(
            "() => document.getElementById('channel-freshness').hidden", timeout=15_000)
        after = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
        check(abs(after - mid) <= 1,
              f"{where}: the freshness pill taking itself away moved the reader ({mid} -> {after})")
        page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    shot(page, shots, f"{where}-{scheme}-03-channel")
    # A reload is a returning reader: no Welcome, still the Channel.
    page.reload(wait_until="load")
    expect_channel(page, f"{where} after reload")
    check(not on(page, "screen-welcome"), f"{where}: the Welcome returned on reload")
    shot(page, shots, f"{where}-{scheme}-04-channel-reloaded")
    # Re-openable from Settings, and from Help.
    page.click("#open-settings")
    page.wait_for_selector("#open-welcome", state="visible", timeout=10_000)
    page.click("#open-welcome")
    expect_welcome(page, f"{where} from Settings")
    shot(page, shots, f"{where}-{scheme}-05-welcome-from-settings")
    page.click("#welcome-start")
    expect_channel(page, f"{where} back from the Welcome")
    page.click("#open-settings")
    page.wait_for_selector("#open-help", state="visible", timeout=10_000)
    page.click("#open-help")
    page.wait_for_selector("#welcome-from-help", state="visible", timeout=10_000)
    page.click("#welcome-from-help")
    expect_welcome(page, f"{where} from Help")
    page.click("#welcome-start")
    expect_channel(page, f"{where} back from Help's Welcome")


def walk_phone(context: BrowserContext, url: str, shots: Path | None, scheme: str) -> None:
    """The phone matrix row: Discuss open, and the keyboard open then closed."""
    page = context.pages[0]
    page.add_init_script(VIEWPORT_JS)
    sign_in(page, url)
    expect_welcome(page, "phone")
    shot(page, shots, f"phone-{scheme}-02-welcome")
    dismiss(page, "phone")
    open_discuss(page, "phone")
    shot(page, shots, f"phone-{scheme}-03-channel-discuss-keyboard-closed")
    page.evaluate(f"() => window.__keyboard({KEYBOARD_LEFT_PX})")
    page.wait_for_function(f"() => window.visualViewport.height === {KEYBOARD_LEFT_PX}",
                           timeout=10_000)
    shot(page, shots, f"phone-{scheme}-04-channel-discuss-keyboard-open")
    page.evaluate("() => window.__keyboardClose()")
    page.wait_for_function(f"() => window.visualViewport.height === {PHONE['height']}",
                           timeout=10_000)
    shot(page, shots, f"phone-{scheme}-05-channel-discuss-keyboard-closed-again")
    # `#1837`: the phone welcome-from-settings shot the #1781 matrix was missing.
    page.click("#open-settings")
    page.wait_for_selector("#open-welcome", state="visible", timeout=10_000)
    page.click("#open-welcome")
    expect_welcome(page, "phone from Settings")
    shot(page, shots, f"phone-{scheme}-06-welcome-from-settings")
    page.click("#welcome-start")
    expect_channel(page, "phone back from the Welcome")


def walk_saved_voice(context: BrowserContext, url: str, shots: Path | None) -> None:
    """A saved Voice choice is honoured: Voice first, and no Welcome unasked."""
    page = context.pages[0]
    sign_in(page, url)
    expect_welcome(page, "saved-voice setup")
    # Choose Voice, so the record the page keeps says so, then reload.
    dismiss(page, "saved-voice setup")
    page.click("#view-switch")  # Channel -> Documents
    page.click("#view-switch")  # Documents -> Voice
    page.wait_for_function("() => !document.getElementById('pane-voice').hidden", timeout=10_000)
    page.reload(wait_until="load")
    page.wait_for_function("""() => !document.getElementById('screen-main').hidden
        && !document.getElementById('pane-voice').hidden""", timeout=10_000)
    check(not on(page, "screen-welcome"), "saved-voice: the Welcome blocked a returning Voice reader")
    shot(page, shots, "saved-voice-after")


def walk_no_channel(context: BrowserContext, api: FirstRunApi, url: str,
                    shots: Path | None) -> None:
    """No channel configured: the guided setup, then Check again onto the new channel."""
    page = context.pages[0]
    sign_in(page, url)
    expect_welcome(page, "no-channel")
    check(on(page, "welcome-guide"), "no-channel: the guided setup is not showing")
    snippet = page.evaluate("() => document.getElementById('welcome-snippet').textContent")
    check("[[channels]]" in snippet and "writable = true" in snippet,
          f"no-channel: the snippet is not the config step: {snippet!r}")
    shot(page, shots, "no-channel-02-welcome-guide")
    page.click("#welcome-check-again")
    page.wait_for_function(
        "() => document.getElementById('welcome-guide-state').textContent.includes('Still no channel')",
        timeout=10_000)
    with api.lock:
        api.listed = [dict(CHANNEL)]
    page.click("#welcome-check-again")
    expect_channel(page, "no-channel after Check again")
    shot(page, shots, "no-channel-03-channel-after-check")


def walk_pairing(context: BrowserContext, url: str, shots: Path | None) -> None:
    """A pairing link (`#369`) is a first run too: redeem, then the Welcome, then the Channel."""
    page = context.pages[0]
    code = "A" * 43
    page.route("**/api/v1/pair/redeem", lambda route: route.fulfill(
        status=200, content_type="application/json",
        body=json.dumps({"token": TOKEN})))
    page.goto(url + f"#pair={code}", wait_until="load")
    page.wait_for_selector("#pair-confirm", state="visible", timeout=10_000)
    shot(page, shots, "pairing-01-offer-before")
    page.click("#pair-confirm")
    expect_welcome(page, "pairing")
    shot(page, shots, "pairing-02-welcome-after")
    dismiss(page, "pairing")


def walk_deep_link(context: BrowserContext, url: str) -> None:
    """`?v=docs` outranks the default: Documents, not the Channel, and no Welcome over it."""
    page = context.pages[0]
    sign_in(page, url + "?v=docs")
    page.wait_for_function("() => !document.getElementById('pane-docs').hidden", timeout=10_000)
    check(not on(page, "screen-welcome"), "deep-link: the Welcome covered the Documents deep link")


def browse(playwright: Playwright, args: argparse.Namespace, web_root: Path) -> dict[str, str]:
    notes: dict[str, str] = {}
    page_errors: list[str] = []
    served = page_sources.served(web_root)

    def fresh_context(viewport: dict[str, int], scheme: str, mobile: bool = False) -> BrowserContext:
        profile = tempfile.mkdtemp(prefix="vibe-talk-firstrun-")
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=args.browser_executable,
            viewport={"width": viewport["width"], "height": viewport["height"]},
            is_mobile=mobile, has_touch=mobile,
            device_scale_factor=2.6 if mobile else 1,
            color_scheme="dark" if scheme == "dark" else "light")
        context.pages[0].on("pageerror", lambda error: page_errors.append(str(error)))
        return context

    for scheme in ("light", "dark"):
        api = FirstRunApi(listed=[dict(CHANNEL)])
        server, thread, url = serve(api, served)
        try:
            context = fresh_context(DESK, scheme)
            try:
                walk_fresh(context, url, args.screenshots, "desk", scheme)
            finally:
                context.close()
            context = fresh_context(PHONE, scheme, mobile=True)
            try:
                walk_phone(context, url, args.screenshots, scheme)
            finally:
                context.close()
        finally:
            api.stopping.set()
            server.shutdown()
            server.server_close()
            thread.join()
        notes[scheme] = "Welcome once on first run, Channel on dismiss and on reload, re-opened from Settings and Help"

    api = FirstRunApi(listed=[dict(CHANNEL)])
    server, thread, url = serve(api, served)
    try:
        context = fresh_context(DESK, "light")
        try:
            walk_saved_voice(context, url, args.screenshots)
        finally:
            context.close()
        notes["saved-voice"] = "a saved Voice choice opens on Voice, the Welcome not shown"
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()

    api = FirstRunApi(listed=[])
    server, thread, url = serve(api, served)
    try:
        # `#1837`: the desk no-channel guide shot the #1781 matrix was missing.
        context = fresh_context(DESK, "light")
        try:
            page = context.pages[0]
            sign_in(page, url)
            expect_welcome(page, "desk no-channel")
            check(on(page, "welcome-guide"), "desk no-channel: the guided setup is not showing")
            shot(page, args.screenshots, "desk-no-channel-02-welcome-guide")
        finally:
            context.close()
        context = fresh_context(PHONE, "light", mobile=True)
        try:
            walk_no_channel(context, api, url, args.screenshots)
        finally:
            context.close()
        context = fresh_context(DESK, "light")
        try:
            walk_deep_link(context, url)
        finally:
            context.close()
        context = fresh_context(PHONE, "light", mobile=True)
        try:
            walk_pairing(context, url, args.screenshots)
        finally:
            context.close()
        notes["no-channel"] = "guided config steps shown; Check again lands on the channel once configured"
        notes["deep-link"] = "?v=docs opens Documents over the default, uncovered"
        notes["pairing"] = "a pairing link redeems into the Welcome, and dismissing lands on the Channel"
        check(not page_errors, f"the page threw: {page_errors}")
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()
    return notes


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        notes = browse(playwright, args, args.web_root)
    print(json.dumps(notes, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
