#!/usr/bin/env python3
"""Measure the command bar in real Chromium: one row across the window on a desk, the phone's as it was.

`#406 command-bar-one-row`. The owner, on a desk: "Thread list view (because of the length of the words
'Thread list') spills onto two rows in the command bar even though there is PLENTY of horizontal space..
an artificial width construction. The other views don't do this but selecting a specific long thread name
does it. Let's keep that bar one row that can go up to full width on desktop and size our widgets
appropriately." The dock used to be held to the reading column and to wrap the reading buttons onto a
second row when the pickers and they did not fit it; on a desk it now spans the window and does not wrap,
and a picker too long for the room left ends in an ellipsis, its tooltip naming the whole thread.

At 1280x800 and 1920x1080, and at 900x700, the narrowest window the desk's layout is drawn in, on a thread whose name is 120 characters, the thread list, Main and All: the
gear, the pickers, the layout button, the reading buttons and the switch on one row (their vertical
centres within 4px of each other), the dock one control tall, nothing past the window's edges, the
switch in the window's lower right corner, and for the thread its whole name in the view picker's
tooltip, and the picker set to end a label too long for it in an ellipsis. In two columns (1000px and
wider) the same. And the widest the pane gets, the call view after a call with its note under Talk, in a
24px default font at 900x700 and 1280x800: nothing drawn over anything, the pane wrapping its own
buttons as a last resort once the bar is at its floor.

The negative control, a 393x852 phone: the bar is one row above the pane's tile, the dock spans the
window, and the dock is the same height whichever view is chosen. With --baseline-web-root DIR (a copy
of the page from before the change) the phone's dock is measured under that page too, and every
control's box must be the same to the pixel.

Against the page from before the change it fails at 1280x800 on the long thread, whose reading buttons
wrap onto a second row.

`#820 voice-page-action-menu`, at 393x852 by touch and 1280x800 by mouse, on a channel whose newest messages
are long: the hamburger, immediately left of the view button on the same row (`#880 voice-page-menu-placement`),
opens the action menu over the list; Expand all
in it opens every message and shuts the menu, Collapse all is then offered and folds them again, and
Summaries is offered; none of the three floats in the chip row any more; Escape shuts the menu and gives
the focus back to the hamburger. `#880 voice-page-menu-placement`: Hide reader in the menu takes the reader's row of
big buttons away, the list taking its room and the dock only the bar, says Show reader, leaves h working,
and is still in force after a reload; Show reader puts the row back. The view button says the view on screen and is pressed on the call's (#845),
by a tap and by Enter and Space from the keys. It reuses keyboard_browser's fixture: its fake API and asset server. --web-root DIR runs it
against another copy of the page; --screenshots DIR keeps a PNG of each step; --port N serves the
fixture on that port (default: one the system picks).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import TOKEN, Json, check
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page

FIRST_ROW = "#discord-log > li[data-id]"

# The 120-character name the owner's long thread stands for: the thread's title and its first message.
LONG_NAME = ("Release train for the autumn platform update: canary widening, staging migration check, "
             "cache headers, retry wrapper fix")
assert len(LONG_NAME) == 120, len(LONG_NAME)

# Every control of the dock that is drawn, by id, with its box; and the dock's own box.
DOCK_JS = """() => {
  const box = (node) => { const r = node.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
  const drawn = (node) => node.getClientRects().length > 0 && getComputedStyle(node).visibility !== 'hidden';
  // The pack scrolls sideways: a member is drawn only as far as the pack shows it, and not at all past it.
  const pack = document.getElementById('bar-pack').getBoundingClientRect();
  const clipped = (node, r) => {
    if (!node.parentNode || node.parentNode.id !== 'bar-pack') return r;
    const left = Math.max(r.left, pack.left), right = Math.min(r.right, pack.right);
    return right - left > 0.5 ? {...r, left, right, width: right - left} : null;
  };
  const controls = [...document.querySelectorAll(
    '#open-settings, #bar-pack > *, #action-menu-open, #view-switch, #control-pane > button, #control-pane > .control')]
    .filter(drawn).map((node) => [node, clipped(node, box(node))]).filter(([, r]) => r)
    .map(([node, r]) => ({id: node.id || node.className, ...r}));
  const select = document.getElementById('thread-select');
  return {dock: box(document.getElementById('dock')), pane: box(document.getElementById('control-pane')),
          bar: box(document.getElementById('control-bar')), controls, width: innerWidth, height: innerHeight,
          view: select.value, label: select.selectedOptions[0] ? select.selectedOptions[0].textContent : '',
          title: select.title, pickerOverflows: select.scrollWidth > select.clientWidth + 1};
}"""


def dock(page: Page) -> Json:
    found = page.evaluate(DOCK_JS)
    assert isinstance(found, dict)
    return found


def controls(state: Json) -> list[dict[str, float | str]]:
    found = state["controls"]
    assert isinstance(found, list)
    return [one for one in found if isinstance(one, dict)]


def number(box: object, key: str) -> float:
    assert isinstance(box, dict)
    value = box[key]
    assert isinstance(value, (int, float))
    return float(value)


def overlaps(state: Json, where: str) -> None:
    """No two controls of the dock drawn over each other, and none past the window's edges."""
    drawn = controls(state)
    width = float(state["width"])  # type: ignore[arg-type]
    for i, one in enumerate(drawn):
        check(number(one, "left") >= -0.5 and number(one, "right") <= width + 0.5,
              f"{where}: #{one['id']} runs past the window: {one}")
        for other in drawn[i + 1:]:
            across = min(number(one, "right"), number(other, "right")) - max(number(one, "left"), number(other, "left"))
            down = min(number(one, "bottom"), number(other, "bottom")) - max(number(one, "top"), number(other, "top"))
            check(across <= 0.5 or down <= 0.5, f"{where}: #{one['id']} and #{other['id']} overlap: {one} {other}")


def one_row(state: Json, where: str) -> None:
    """Every control of the dock on one row, inside the window, overlapping none, the switch in its corner."""
    overlaps(state, where)
    drawn = controls(state)
    check(len(drawn) >= 5, f"{where}: fixture: only {len(drawn)} controls drawn in the dock: {drawn}")
    centres = [(number(one, "top") + number(one, "bottom")) / 2 for one in drawn]
    spread = max(centres) - min(centres)
    check(spread <= 4, f"{where}: the dock is more than one row, its controls' centres {spread:.0f}px apart: "
          + ", ".join(f"{one['id']}@{number(one, 'top'):.0f}" for one in drawn))
    tallest = max(number(one, "height") for one in drawn)
    check(number(state["dock"], "height") <= tallest + 20,
          f"{where}: the dock is {number(state['dock'], 'height'):.0f}px tall for {tallest:.0f}px controls")
    width = float(state["width"])  # type: ignore[arg-type]
    for one in drawn:
        check(number(one, "left") >= 0 and number(one, "right") <= width + 0.5,
              f"{where}: #{one['id']} runs past the window: {one}")
    switch = next((one for one in drawn if one["id"] == "view-switch"), None)
    check(switch is not None, f"{where}: the switch is not drawn")
    assert switch is not None
    check(width - number(switch, "right") <= 16, f"{where}: the switch is not at the window's right edge: {switch}")


def choose(page: Page, value: str) -> Json:
    page.select_option("#thread-select", value)
    page.wait_for_function("(v) => document.getElementById('thread-select').value === v", arg=value, timeout=10_000)
    page.wait_for_timeout(300)
    return dock(page)


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}.png"))


def open_page(browser: Browser, url: str, width: int, height: int, mobile: bool = False) -> Page:
    context = browser.new_context(viewport={"width": width, "height": height}, is_mobile=mobile, has_touch=mobile)
    page = context.new_page()
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    return page


def thread_value(page: Page) -> str:
    value = page.evaluate("() => [...document.getElementById('thread-select').options]"
                          ".map((o) => o.value).find((v) => v.startsWith('thread:'))")
    check(isinstance(value, str), "fixture: the view picker offers no thread")
    assert isinstance(value, str)
    return value


def desk(browser: Browser, url: str, width: int, height: int, shots: Path | None, notes: list[str]) -> None:
    page = open_page(browser, url, width, height)
    errors: list[str] = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    size = f"{width}x{height}"
    # The owner's two first: the long thread, then the thread list; then the views that never wrapped.
    state = choose(page, thread_value(page))
    shot(page, shots, f"{size}-long-thread")
    one_row(state, f"{size} on a thread named in 120 characters")
    label = str(state["label"])
    check(label.endswith("…"), f"{size}: fixture: the long thread's option is not cut: {label!r}")
    check(LONG_NAME in str(state["title"]), f"{size}: the view picker's tooltip does not name the whole thread: "
          f"{state['title']!r}")
    cut = page.evaluate("() => getComputedStyle(document.getElementById('thread-select')).textOverflow")
    check(cut == "ellipsis", f"{size}: a view picker too long for the bar is not cut with an ellipsis: {cut!r}")
    for value, name in (("threads", "Thread list"), ("main", "Main"), ("flat", "All")):
        state = choose(page, value)
        check(state["label"] == name, f"{size}: fixture: the picker shows {state['label']!r}, not {name!r}")
        shot(page, shots, f"{size}-{value}")
        one_row(state, f"{size} in {name}")
    # Two columns where the window draws them, at 1000px and wider.
    columns = width >= 1000
    if columns:
        page.click("#column-toggle")
        page.wait_for_function("() => document.getElementById('screen-main').getAttribute('data-columns') === 'two'",
                               timeout=10_000)
        page.wait_for_timeout(300)
        state = dock(page)
        shot(page, shots, f"{size}-two-columns")
        one_row(state, f"{size} in two columns")
    check(not errors, f"{size}: the page threw: {errors}")
    notes.append(f"{size}: one row across the window in Main, All, Thread list, a 120-character thread "
                 f"(its option cut to {label!r}, the whole name in the tooltip){' and two columns' if columns else ''},"
                 f" dock {number(state['dock'], 'height'):.0f}px tall")
    page.context.close()


# After a call, as renderControls draws it: Talk says "Start a new call" with its one-clause note under it.
AFTER_CALL_JS = """() => { document.getElementById('hang-up').hidden = true;
  document.getElementById('control-pane').className = 'solo';
  document.getElementById('talk').className = 'control control-talk';
  document.getElementById('talk-label').textContent = 'Start a new call';
  const note = document.getElementById('talk-note');
  note.textContent = 'the agent starts fresh — the earlier conversation was too long to replay';
  note.hidden = false; }"""


def large_font_after_call(browser: Browser, url: str, width: int, height: int, notes: list[str]) -> None:
    """The widest the pane gets — after a call, its note under Talk — in a 24px default font: past the
    bar's floor the pane wraps its own buttons, as a last resort, and nothing is drawn over anything."""
    page = open_page(browser, url, width, height)
    page.click("#view-switch")
    page.wait_for_function("() => document.getElementById('view-switch').getAttribute('aria-pressed') !== 'false'",
                           timeout=10_000)
    page.evaluate("() => { document.documentElement.style.fontSize = '24px'; }")
    page.evaluate(AFTER_CALL_JS)
    page.wait_for_timeout(300)
    state = dock(page)
    overlaps(state, f"{width}x{height}, 24px type, after a call")
    bar = next((one for one in controls(state) if one["id"] == "open-settings"), None)
    check(bar is not None, f"{width}x{height}, 24px type, after a call: the gear is not drawn")
    notes.append(f"{width}x{height} at a 24px default font after a call: nothing drawn over anything, the dock "
                 f"{number(state['dock'], 'height'):.0f}px tall")
    page.context.close()


# `#820 voice-page-action-menu`: the menu, its items, the chips over the list, and the view button.
MENU_JS = """() => {
  const shown = (id) => { const n = document.getElementById(id); return Boolean(n && !n.hidden && n.getClientRects().length); };
  const box = (id) => { const r = document.getElementById(id).getBoundingClientRect(); return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, height: r.height}; };
  const tools = document.getElementById('scroll-tools');
  return {
    open: shown('action-menu'), expanded: document.getElementById('action-menu-open').getAttribute('aria-expanded'),
    expand: shown('expand-all'), collapse: shown('collapse-all'), summaries: shown('summarise'),
    floating: ['expand-all', 'collapse-all', 'summarise'].filter((id) => tools.contains(document.getElementById(id))),
    folded: document.querySelectorAll('#discord-log > li[data-collapsed="true"]').length,
    hamburger: box('action-menu-open'), switch: box('view-switch'),
    pane: shown('control-pane'), reader: document.getElementById('reader-toggle-label').textContent,
    dock: box('dock'), barHeight: document.getElementById('control-bar').getBoundingClientRect().height,
    list: box('scroll-area'), mode: document.getElementById('todo-filter').getAttribute('data-read-mode'),
    focus: document.activeElement ? document.activeElement.id : null,
    pressed: document.getElementById('view-switch').getAttribute('aria-pressed'),
    word: document.getElementById('view-switch-label').textContent,
  };
}"""


def count(value: object) -> int:
    """A count the page measured, read out of its JSON answer."""
    assert isinstance(value, int), value
    return value


def menu_state(page: Page) -> Json:
    found: object = page.evaluate(MENU_JS)
    assert isinstance(found, dict)
    return {str(key): value for key, value in found.items()}


def press(page: Page, selector: str, touch: bool) -> None:
    box = page.locator(selector).first.bounding_box()
    check(box is not None, f"{selector} is not drawn to press")
    assert box is not None
    if touch:
        page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def beside(now: Json, where: str) -> None:
    """`#880 voice-page-menu-placement`: the hamburger IMMEDIATELY LEFT of the view button, on its row."""
    menu, switch = now["hamburger"], now["switch"]
    gap = number(switch, "left") - number(menu, "right")
    check(-0.5 <= gap <= 12, f"{where}: the hamburger is not immediately left of the view button ({gap:.1f}px): {menu} {switch}")
    down = min(number(menu, "bottom"), number(switch, "bottom")) - max(number(menu, "top"), number(switch, "top"))
    check(down > 0.5 * number(menu, "height"), f"{where}: the hamburger is not on the view button's row: {menu} {switch}")


def hide_reader(page: Page, where: str, touch: bool, shots: Path | None, width: int, height: int) -> None:
    """`#880 voice-page-menu-placement`: Hide reader, its keys, a reload, and Show reader."""
    before = menu_state(page)
    check(before["pane"] is True and before["reader"] == "Hide reader", f"{where}: the premise, the row shown: {before}")
    press(page, "#action-menu-open", touch)
    page.wait_for_timeout(200)
    press(page, "#reader-toggle", touch)
    page.wait_for_timeout(300)
    now = menu_state(page)
    shot(page, shots, f"{width}x{height}-reader-hidden")
    check(now["pane"] is False and now["reader"] == "Show reader" and now["open"] is False,
          f"{where}: Hide reader left {now}")
    # No band where the row was: the dock holds only the bar, and the list took the room.
    check(number(now["dock"], "height") - float(now["barHeight"]) <= 14,  # type: ignore[arg-type]
          f"{where}: with the reader hidden the dock is {number(now['dock'], 'height'):.0f}px for a "
          f"{now['barHeight']}px bar")
    check(number(now["dock"], "bottom") >= height - 0.5, f"{where}: the dock left the window's foot: {now['dock']}")
    # On a phone the row is a band of its own, and the list takes all of it; on a desk the row stands beside the
    # bar, so the dock keeps its height and the bar takes the row's width.
    shrank = number(before["dock"], "height") - number(now["dock"], "height")
    grown = number(now["list"], "height") - number(before["list"], "height")
    check(abs(grown - shrank) <= 2, f"{where}: the dock gave {shrank:.0f}px and the list took {grown:.0f}px")
    if width < 1000:
        check(shrank >= 30, f"{where}: hiding the reader gave the list only {shrank:.0f}px")
    beside(now, f"{where}, reader hidden")
    # A way out of sound stays: with Hang up, Stop or a lit Read the row is drawn whatever the choice.
    for way, show, hide in (
        ("Hang up", "el('hang-up').hidden = false", "el('hang-up').hidden = true"),
        ("Stop", "el('read-aloud-stop').hidden = false", "el('read-aloud-stop').hidden = true"),
        ("a lit Read", "el('read-aloud').setAttribute('data-active', 'true')",
         "el('read-aloud').setAttribute('data-active', 'false')"),
    ):
        page.evaluate(f"() => {{ const el = (id) => document.getElementById(id); {show}; }}")
        page.wait_for_timeout(100)
        check(menu_state(page)["pane"] is True, f"{where}: the reader hidden over {way}, with no way out of the sound")
        page.evaluate(f"() => {{ const el = (id) => document.getElementById(id); {hide}; }}")
        page.wait_for_timeout(100)
    check(menu_state(page)["pane"] is False, f"{where}: the row did not go again after the sound")
    # The keys still act through the hidden row: h steps the read mode, and back round.
    page.focus("#scroll-area")
    page.keyboard.press("h")
    page.wait_for_timeout(200)
    stepped = menu_state(page)["mode"]
    check(stepped != before["mode"], f"{where}: h did nothing with the reader hidden ({stepped})")
    for _ in range(2):
        page.keyboard.press("h")
        page.wait_for_timeout(150)
    check(menu_state(page)["mode"] == before["mode"], f"{where}: h did not come back round")
    # Kept: a reload opens with it hidden.
    page.reload()
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    check(page.get_attribute("#view-switch", "aria-pressed") == "false", f"{where}: the reload left the channel")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pane"] is False and now["reader"] == "Show reader", f"{where}: a reload forgot Hide reader: {now}")
    press(page, "#action-menu-open", touch)
    page.wait_for_timeout(200)
    press(page, "#reader-toggle", touch)
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pane"] is True and now["reader"] == "Hide reader", f"{where}: Show reader left {now}")


def action_menu(browser: Browser, url: str, width: int, height: int, touch: bool, shots: Path | None,
                notes: list[str]) -> None:
    """`#820 voice-page-action-menu`: the hamburger's menu of the channel's actions, and the view button."""
    where = f"{width}x{height} menu"
    page = open_page(browser, url, width, height, mobile=touch)
    errors: list[str] = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pressed"] == "false" and now["word"] == "Channel", f"{where}: the view button on the channel says {now}")
    check(now["open"] is False and now["expanded"] == "false", f"{where}: the menu is open before it is asked for: {now}")
    check(now["floating"] == [], f"{where}: {now['floating']} still float in the chip row")
    beside(now, where)
    folded = count(now["folded"])
    check(folded > 0, f"{where}: the premise, long messages folded in the list: {now}")
    press(page, "#action-menu-open", touch)
    page.wait_for_timeout(200)
    now = menu_state(page)
    check(now["open"] is True and now["expanded"] == "true", f"{where}: the hamburger opened no menu: {now}")
    check(now["expand"] is True and now["summaries"] is True, f"{where}: Expand all and Summaries are not offered: {now}")
    shot(page, shots, f"{width}x{height}-action-menu")
    press(page, "#expand-all", touch)
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["open"] is False, f"{where}: choosing Expand all left the menu open: {now}")
    check(count(now["folded"]) == 0, f"{where}: Expand all left {now['folded']} messages folded")
    press(page, "#action-menu-open", touch)
    page.wait_for_timeout(200)
    now = menu_state(page)
    check(now["collapse"] is True and now["expand"] is False, f"{where}: with every message open, the menu offers {now}")
    press(page, "#collapse-all", touch)
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(count(now["folded"]) >= folded and now["open"] is False, f"{where}: Collapse all did not fold them again: {now}")
    # Escape shuts it, the focus back on the hamburger.
    page.click("#action-menu-open")
    page.wait_for_timeout(150)
    page.keyboard.press("Escape")
    page.wait_for_timeout(150)
    now = menu_state(page)
    check(now["open"] is False and now["focus"] == "action-menu-open", f"{where}: Escape left {now}")
    hide_reader(page, where, touch, shots, width, height)
    # The view button: a press, then Enter and Space from the keys.
    press(page, "#view-switch", touch)
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pressed"] == "true" and now["word"] == "Voice", f"{where}: a press on the view button left {now}")
    page.focus("#view-switch")
    page.keyboard.press("Enter")
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pressed"] == "false" and now["word"] == "Channel", f"{where}: Enter on the view button left {now}")
    # Coming to the channel puts the keys in its list, as it always has: back on the button for Space.
    page.focus("#view-switch")
    page.keyboard.press(" ")
    page.wait_for_timeout(300)
    now = menu_state(page)
    check(now["pressed"] == "true" and now["word"] == "Voice", f"{where}: Space on the view button left {now}")
    # The call's transcript folds too: the menu is on the call's view as well.
    check(page.is_visible("#action-menu-open"), f"{where}: no action menu on the call's view")
    check(page.get_attribute("#view-switch", "aria-label") == "Voice view",
          f"{where}: the view button's name changed with it: {page.get_attribute('#view-switch', 'aria-label')!r}")
    check(not errors, f"{where}: the page threw: {errors}")
    page.context.close()
    notes.append(f"{where}: the hamburger immediately left of the view button opened the menu, Hide reader took the "
                 "row away with no band, kept its keys and a reload, Show reader put it back, Expand all opened {folded} folded "
                 "messages and shut it, Collapse all folded them again, Summaries offered, none of them in the chip "
                 "row, Escape shut it onto the hamburger; the view button pressed on the call's view and not on the channel's, by a tap, Enter and Space")


def phone_boxes(browser: Browser, url: str, shots: Path | None, tag: str) -> dict[str, list[dict[str, float | str]]]:
    page = open_page(browser, url, 393, 852, mobile=True)
    found: dict[str, list[dict[str, float | str]]] = {}
    for value in ("main", "flat", "threads", thread_value(page)):
        state = choose(page, value)
        key = "thread" if value.startswith("thread:") else value
        shot(page, shots, f"393x852-{tag}-{key}")
        drawn = controls(state)
        bar = [one for one in drawn if one["id"] not in ("todo-filter", "read-speed", "read-aloud", "speaker",
                                                          "clear-view", "hang-up", "talk")]
        centres = [(number(one, "top") + number(one, "bottom")) / 2 for one in bar]
        check(max(centres) - min(centres) <= 6, f"393x852 {key}: the phone's bar is more than one row: {bar}")
        check(number(state["pane"], "top") >= number(state["bar"], "bottom") - 1,
              f"393x852 {key}: the pane is no longer under the bar: {state['pane']} {state['bar']}")
        check(number(state["dock"], "left") <= 0.5 and number(state["dock"], "right") >= 392.5,
              f"393x852 {key}: the phone's dock does not span the window: {state['dock']}")
        found[key] = [{**one, "dock": number(state["dock"], "height")} for one in drawn]
    heights = {key: boxes[0]["dock"] if boxes else 0 for key, boxes in found.items()}
    check(len(set(heights.values())) == 1, f"393x852: the phone's dock changes height with the view: {heights}")
    page.context.close()
    return found


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--baseline-web-root", type=Path, default=None,
                        help="a copy of the page from before the change: the phone's dock must match it to the "
                             "pixel (default: none, the phone checked on its own)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
    parser.add_argument("--port", type=int, default=0,
                        help="the first local port to serve a fixture on, the baseline on the next "
                             "(default: 0, ones the system picks)")
    return parser.parse_args()


def serve(web_root: Path, port: int) -> tuple[ThreadingHTTPServer, threading.Thread, kb.FakeApi]:
    api = kb.FakeApi()
    for message in api.messages:
        if message["id"] == api.threads[0]["root"]["id"]:  # type: ignore[index]
            message["content"] = LONG_NAME
    api.threads[0]["title"] = LONG_NAME
    api.threads[0]["root"] = {**api.threads[0]["root"], "content": LONG_NAME}  # type: ignore[dict-item]
    server = ThreadingHTTPServer(("127.0.0.1", port), kb.handler_for(api, web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, api


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    servers = [serve(page_sources.served(args.web_root), args.port)]
    # `#820 voice-page-action-menu`: a channel whose newest messages are long, so there is something to fold.
    long_server, long_thread, long_api = serve(page_sources.served(args.web_root), 0)
    for held in long_api.messages[-4:]:
        if held.get("thread") is None:
            held["content"] = kb.LONG
    if args.baseline_web_root is not None:
        servers.append(serve(args.baseline_web_root, args.port + 1 if args.port else 0))
    urls = [f"http://127.0.0.1:{server.server_port}/" for server, _, _ in servers]
    notes: list[str] = []
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
            for width, height in ((1280, 800), (1920, 1080), (900, 700)):
                desk(browser, urls[0], width, height, args.screenshots, notes)
            for width, height in ((900, 700), (1280, 800)):
                large_font_after_call(browser, urls[0], width, height, notes)
            phone = phone_boxes(browser, urls[0], args.screenshots, "page")
            if len(urls) > 1:
                baseline = phone_boxes(browser, urls[1], args.screenshots, "baseline")
                check(phone == baseline, f"393x852: the phone's dock differs from the baseline page's:\n{phone}\n"
                      f"{baseline}")
                notes.append("393x852 phone: bar one row above the pane's tile, each control's box the same as the "
                             "baseline page's in Main, All, Thread list and the long thread")
            else:
                notes.append("393x852 phone: bar one row above the pane's tile, the dock spanning the window and the "
                             "same height in Main, All, Thread list and the long thread")
            long_url = f"http://127.0.0.1:{long_server.server_port}/"
            action_menu(browser, long_url, 393, 852, True, args.screenshots, notes)
            action_menu(browser, long_url, 1280, 800, False, args.screenshots, notes)
            version = browser.version
            browser.close()
        print(f"Chromium {version}: " + "; ".join(notes))
        return 0
    finally:
        long_api.stopping.set()
        long_server.shutdown()
        long_server.server_close()
        long_thread.join()
        for server, thread, api in servers:
            api.stopping.set()
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
