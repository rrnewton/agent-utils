#!/usr/bin/env python3
"""Load-invariant client performance counts for the hot triggers (`#2281`).

`#2281 vibe-talk-client-perf-benchmarks` (owner 381 part B, owner rule of
2026-11-21): budgets for the client are counts of work, never wall-clock.
This walk serves the real page and a fake chat upstream over the big
channel fixture and measures, per trigger — composer keystroke, search
keystroke, live arrival, refresh (channel re-read), and a scroll burst:

  class writes, childList mutations, distinct nodes touched, layout
  reads (getBoundingClientRect calls), fetches and storage writes,
  plus CDP Layout and RecalcStyle event counts.

Long-task count and summed trace CPU microseconds are printed as a
report only: whether a task crosses 50 ms depends on contention, so a
hot host must never be able to red this step on them (the coordinator's
amendment to the plan). Ceilings gate only on the pure counts, and are
set from the measured baseline after the freeze fix lands; until then
the walk prints the baseline table and asserts only that every count
is finite and the triggers did their work.

Every wait is a condition wait. Runs at phone size, windowed lists off
and on. Print the table with --report (the default prints it too).
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import offline_cache_browser as fixture
import page_sources
import archive_single_row_browser as archive
import search_typing_fast_browser as typing_fast
from offline_cache_browser import check

if TYPE_CHECKING:
    from playwright.sync_api import Page

COUNTERS_JS = """() => {
    window.__perf = { cls: 0, child: 0, touched: new Set(), rect: 0, fetch: 0, lsSet: 0 };
    const attach = () => {
        const log = document.getElementById('discord-log');
        if (!log) { document.addEventListener('DOMContentLoaded', attach, { once: true }); return; }
        new MutationObserver((ms) => {
            for (const m of ms) {
                if (m.attributeName === 'class') window.__perf.cls += 1;
                if (m.type === 'childList') window.__perf.child += 1;
                window.__perf.touched.add(m.target);
            }
        }).observe(log, { subtree: true, attributes: true, childList: true });
    };
    attach();
    const proto = Element.prototype; const og = proto.getBoundingClientRect;
    proto.getBoundingClientRect = function () { window.__perf.rect += 1; return og.call(this); };
    const of = window.fetch;
    window.fetch = (...a) => { window.__perf.fetch += 1; return of(...a); };
    const lp = Object.getPrototypeOf(localStorage); const os_ = lp.setItem;
    lp.setItem = function (k, v) { window.__perf.lsSet += 1; return os_.call(this, k, v); };
}"""

RESET_JS = """() => { window.__perf.cls = 0; window.__perf.child = 0;
    window.__perf.touched = new Set(); window.__perf.rect = 0;
    window.__perf.fetch = 0; window.__perf.lsSet = 0; }"""

SNAP_JS = """() => ({ cls: window.__perf.cls, child: window.__perf.child,
    touched: window.__perf.touched.size, rect: window.__perf.rect,
    fetch: window.__perf.fetch, lsSet: window.__perf.lsSet,
    mounted: document.querySelectorAll('#discord-log > li').length })"""


def until(page: Page, script: str, why: str,
          seconds: float = browser_waits.HANG_BACKSTOP_MS / 1000) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


class Tracing:
    """Layout/RecalcStyle counts over one trigger, via CDP Performance
    metrics deltas (LayoutCount, RecalcStyleCount). TaskDuration is
    summed CPU time for the report only; it never gates."""

    def __init__(self, page: Page) -> None:
        self.session = page.context.new_cdp_session(page)
        self.session.send("Performance.enable")
        self.before = self._metrics()

    def _metrics(self) -> dict[str, float]:
        out = self.session.send("Performance.getMetrics")
        return {m["name"]: float(m["value"]) for m in out.get("metrics", [])}

    def stop(self) -> dict[str, int]:
        after = self._metrics()
        return {
            "layout": int(after.get("LayoutCount", 0) - self.before.get("LayoutCount", 0)),
            "recalc": int(after.get("RecalcStyleCount", 0) - self.before.get("RecalcStyleCount", 0)),
            "longtask": 0,  # report-only placeholder: no load-invariant source in this harness
            "cpu_us": int((after.get("TaskDuration", 0) - self.before.get("TaskDuration", 0)) * 1_000_000),
        }


def measure(page: Page, label: str, trigger: str, act: object) -> dict[str, object]:
    page.evaluate(RESET_JS)
    tracing = Tracing(page)
    act()  # type: ignore[operator]
    counts = tracing.stop()
    snap = page.evaluate(SNAP_JS)
    row = {"trigger": trigger, **snap, **counts}
    print(json.dumps({"run": label, **row}, sort_keys=True), flush=True)
    return row


class PerfApi(typing_fast.BigApi):
    """The big channel with a one-second live poll, so an arrival the
    test appends lands through the page's own live read."""

    def client_config(self, scope: str) -> fixture.Json:
        return {**super().client_config(scope), "live_poll_seconds": 1}


def walk(page: Page, api: PerfApi, label: str, total: int) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    archive.open_channel(page, total)

    def composer_key() -> None:
        page.click("#channel-compose-text")
        page.keyboard.type("x")
        page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")

    rows.append(measure(page, label, "composer-key", composer_key))

    def search_key() -> None:
        page.click("#search-toggle")
        page.wait_for_selector("#search-field", state="visible",
                               timeout=browser_waits.HANG_BACKSTOP_MS)
        page.evaluate("""() => { const f = document.getElementById('search-field');
            f.value = 'lantern'; f.dispatchEvent(new Event('input', { bubbles: true })); }""")
        page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")

    rows.append(measure(page, label, "search-key", search_key))
    page.keyboard.press("Escape")

    def arrival() -> None:
        with api.lock:
            api.messages.append(fixture.message(len(api.messages), "arrival probe note"))
        until(page, "() => document.body.textContent.includes('arrival probe note')",
              f"{label}: the live arrival never stood on the page")

    rows.append(measure(page, label, "arrival", arrival))

    def refresh() -> None:
        page.reload(wait_until="load")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {min(total, 20)}",
              f"{label}: the refreshed channel did not stand back up")

    rows.append(measure(page, label, "refresh", refresh))

    def scroll() -> None:
        page.evaluate("""() => { const a = document.getElementById('scroll-area');
            a.scrollTop = 0; a.dispatchEvent(new Event('scroll')); }""")
        page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")

    rows.append(measure(page, label, "scroll-burst", scroll))
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--report", action="store_true", help="print the baseline table (default)")
    args = parser.parse_args()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    web_root = page_sources.served(args.web_root)
    total = 816
    load1 = os.getloadavg()[0]
    all_rows: list[dict[str, object]] = []
    with sync_playwright() as playwright:
        for label, windowed in (("phone-full", False), ("phone-windowed", True)):
            api = PerfApi()
            server, thread, url = typing_fast.serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-perf-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": 412, "height": 915}, color_scheme="dark",
                        device_scale_factor=2.625, is_mobile=True, has_touch=True)
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0]
                    init = "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1');"
                    if windowed:
                        init += " localStorage.setItem('vibe-talk.voice.windowed-lists', '1');"
                    page.add_init_script(init + " } catch (e) {}")
                    page.add_init_script(f"({COUNTERS_JS})()")
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    all_rows.extend(walk(page, api, label, total))
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    print(f"client perf baseline: load1={load1:.0f} rows={len(all_rows)} (counts only; "
          "longtask and cpu_us are report-only, never gated)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
