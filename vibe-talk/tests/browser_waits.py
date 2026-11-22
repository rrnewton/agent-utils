"""Hot-host waiting shared by the browser walks (`#2077 vibe-talk-browser-checks-hot-host`).

Owner rule 346: every gating check passes on a host above 75% CPU, and no wall-clock budget
decides a verdict. A Chromium boxed beside builders on a loaded host can take seconds to paint a
frame, so Playwright's 30 s defaults turned ordinary starvation into rotating click/wait
timeouts in unrelated gates. The walks therefore wait on conditions; the only clocks left are
hang backstops, sized far beyond any plausible scheduling delay and well inside each step's own
dagrun wall timeout, whose expiry means the page is wedged, not merely slow.

Every walk applies `hot_host_timeouts` to each context (or standalone page) it creates. New
walks: do the same, and prefer `wait_until` / `click_when_clear` over a fixed dwell followed by
a one-shot read — a dwell races layout settling exactly as the pre-#2012 walk did.
"""

from __future__ import annotations

import time
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page

HANG_BACKSTOP_MS = 120_000
"""The outer bound on any one wait or action: a hang detector, never a budget. Matches the
backstop `documents_browser` and `github_helper_browser` already used before this module."""

_POLL_S = 0.05


def hot_host_timeouts(target: BrowserContext | Page) -> None:
    """Give a fresh context (or a page with no context of its own) the hang backstop as its
    default action and navigation timeout, replacing Playwright's 30 s defaults."""
    target.set_default_timeout(HANG_BACKSTOP_MS)
    target.set_default_navigation_timeout(HANG_BACKSTOP_MS)


def wait_until(page: Page, expression: str, what: str, *, seconds: float = HANG_BACKSTOP_MS / 1000) -> None:
    """Poll a page-side condition until it holds; the failure names the condition (`what`),
    never a bare timeout. `seconds` is the hang backstop, as above."""
    deadline = time.monotonic() + seconds
    while True:
        if page.evaluate(expression):
            return
        if time.monotonic() >= deadline:
            raise AssertionError(f"waiting for {what}: still false after the hang backstop ({seconds:g}s)")
        page.wait_for_timeout(int(_POLL_S * 1000))


def click_when_clear(page: Page, selector: str, *, seconds: float = HANG_BACKSTOP_MS / 1000) -> None:
    """Click as soon as the target can actually be clicked, retrying the click itself.

    Under load a folding bar or a still-settling scroll area can cover a control for far
    longer than a fixed dwell allows, and a home-grown hit test can disagree with the one the
    click will use. Retrying the action in short trials (the #2012 pattern) waits on the real
    condition — the click landing — and the failure names the selector at the backstop."""
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError

    deadline = time.monotonic() + seconds
    while True:
        try:
            page.click(selector, timeout=2_000)
            return
        except PlaywrightTimeoutError:
            if time.monotonic() >= deadline:
                # Name what covers the target, so a backstop failure reads as state, not luck.
                try:
                    state = str(page.evaluate(
                        """(sel) => { const el = document.querySelector(sel); if (!el) return 'missing';
                        const r = el.getBoundingClientRect();
                        const stack = document.elementsFromPoint(r.left + r.width / 2, r.top + r.height / 2)
                          .slice(0, 5).map((e) => e.id ? '#' + e.id : e.tagName);
                        return JSON.stringify({rect: [r.left, r.top, r.width, r.height], stack}); }""",
                        selector))
                except Exception:
                    state = "state unavailable"
                raise AssertionError(
                    f"{selector} still not clickable after the hang backstop ({seconds:g}s): {state}") from None
            page.wait_for_timeout(int(_POLL_S * 1000))
