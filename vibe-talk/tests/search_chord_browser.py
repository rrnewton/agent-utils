#!/usr/bin/env python3
"""Press the search's Ctrl+/ (Cmd+/) and Help's Ctrl+? in real Chromium, from wherever the focus is.

`#344 search-shortcut`. The owner: "want reliable cmd-/ for search ... Sometimes just pressing '/'
doesn't work ... prefer it to just be Cmd-/. Currently that pops open help ... map it to Cmd-? (i.e.
Cmd-shift-/)." A bare "/" searches only from the list: in a text field it is a slash, on a picker it is
the picker's type-to-find, and with single keys off it is nothing. The chord is heard by a listener at
the window in the capture phase, before anything else on the page, and searches from all of those.
The fake-DOM suite pins the order of the listeners and what each screen does; this presses real keys
through CDP `Input.dispatchKeyEvent`, the path a hardware keyboard takes into the renderer, so the
browser's own handling of each key (a character typed into a field, a picker's own keys, the default a
chord would have) runs as it would under a person's fingers.

At 1280x800, after each chord: the search open, its field holding the focus, and the chord's default
refused (`defaultPrevented`, read by a listener on the window after the page's own):

  the channel's message box holding a draft -> Ctrl+/ opens the search with the draft as it was and
  no slash typed, and what is typed next goes into the search; a bare "/" in the box types a slash ->
  the reading-pace popover open, the focus on its slider: a bare "/" opens nothing, Ctrl+/ closes the
  popover and searches -> a message's ⋯ menu open, the focus on an item: closed, and the search ->
  the channel picker focused: a bare "/" opens nothing, Ctrl+/ searches -> Settings, the focus in a
  text field with words in it: Settings left, the words as they were -> Help, opened by "?" -> two
  columns, the focus in the right column's message box with a draft -> a listener on the message box
  that stops every key and refuses its default: the search still opens, the draft as it was -> the
  Reply screen with a draft: left, and the reply opened again holds the draft -> single keys off: a
  bare "/" opens nothing, Ctrl+/ searches -> Ctrl+Shift+? opens Help at the Keyboard entry, whose list
  names Ctrl+/ and Ctrl+?, and the glass and Help's link carry them in `aria-keyshortcuts` -> on a
  German layout's report of Ctrl+Shift+7 ("/"), the search -> a "/" sent as AltGr sends it (Ctrl and Alt
  together) opens nothing and is not refused -> a Ctrl+/ held for three repeats opens the search once,
  every repeat refused.

Then a second tab whose `navigator.platform` is emulated as a Mac's: Cmd+/ from the message box
searches; Cmd+Shift+? opens Help, and so does Cmd+Shift+/ reported as "/" with Shift; Cmd+Shift+7
reported as "/" searches; and the Settings hint with single keys off says "Cmd+? lists them".

What it cannot show: CDP's keys reach the page past the operating system and the browser's menus, so
a Mac's Help menu taking Cmd+?, or Safari's status-bar item taking Cmd+/, before the page sees the key
is not something any check here can see.

It reuses keyboard_browser's fixture: its fake API, asset server and CDP key helpers. --web-root DIR
runs it against another copy of the page; against one from before the change it fails at the first
chord, which opens Help instead. --screenshots DIR keeps a PNG of each step.
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
from keyboard_browser import CHANNEL, TOKEN, Json, Walk, check

if TYPE_CHECKING:
    from playwright.sync_api import CDPSession, Page

FIRST_ROW = "#discord-log > li[data-id]"

# The first text field on Settings the reader can see, for the focus to be in. Seen by `checkVisibility`:
# a field in a fold hidden until found (`#346 settings-collapsible`) still has boxes.
SETTINGS_FIELD_JS = """() => {
  const fields = [...document.querySelectorAll('#screen-settings input, #screen-settings textarea')];
  const untyped = ['checkbox', 'radio', 'range', 'button', 'submit', 'reset', 'image', 'file', 'color', 'hidden'];
  const field = fields.find((one) => one.id && !untyped.includes(String(one.type).toLowerCase())
    && one.checkVisibility() && !one.disabled && !one.readOnly);
  return field ? field.id : null;
}"""

# `#346 settings-collapsible`: Settings opens folded. Expand all, clicked by the script so the focus stays
# where it is, opens every fold, and they stay open while the page is.
SETTINGS_UNFOLD_JS = """() => {
  const all = document.getElementById('fold-settings-all');
  if (all.textContent.trim() === 'Expand all') all.click();
}"""

# Count the search field's `select()` calls, which the search makes each time it takes the focus.
COUNT_SELECTS_JS = """() => {
  const field = document.getElementById('search-field');
  window.__selects = 0;
  const own = field.select.bind(field);
  field.select = () => { window.__selects += 1; own(); };
}"""

# A listener on the message box that stops every keydown there and refuses its default: what a listener
# on the page that swallowed the key would do. Removable, by the same reference.
HOSTILE_ON_JS = """() => {
  window.__hostile = (event) => { event.stopPropagation(); event.preventDefault(); };
  document.getElementById('channel-compose-text').addEventListener('keydown', window.__hostile);
}"""
HOSTILE_OFF_JS = """() => {
  document.getElementById('channel-compose-text').removeEventListener('keydown', window.__hostile);
}"""


def value(page: Page, element_id: str) -> str:
    found = page.evaluate("(id) => document.getElementById(id).value", element_id)
    assert isinstance(found, str)
    return found


def recorded(walk: Walk) -> list[dict[str, object]]:
    """Every keydown the window's recorder has heard, as the page left it."""
    keys = walk.state()["keys"]
    assert isinstance(keys, list)
    return [one for one in keys if isinstance(one, dict)]


def press_raw(cdp: CDPSession, key: str, code: str, vk: int, modifiers: int, text: str = "") -> None:
    """A key as a layout or a platform reports it, which `kb.press` cannot make: `key` and `code` apart."""
    down: Json = {"type": "keyDown" if text else "rawKeyDown", "modifiers": modifiers, "key": key, "code": code,
                  "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk}
    if text:
        down["text"] = text
        down["unmodifiedText"] = text
    cdp.send("Input.dispatchKeyEvent", down)
    cdp.send("Input.dispatchKeyEvent", {"type": "keyUp", "modifiers": modifiers, "key": key, "code": code,
                                        "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk})


def searched(walk: Walk, where: str, refused: bool = True) -> Json:
    """The state after a chord that must have opened the search with its field focused, its default refused."""
    state = walk.settled()
    check(state["screen"] == "main", f"{where}: the chord left the reader on {state['screen']!r}")
    check(state["search"] is True, f"{where}: the chord did not open the search: {state}")
    check(state["focus"] == "search-field", f"{where}: the chord left the focus on {state['focus']!r}")
    if refused:
        last = walk.last_key()
        check(last["prevented"] is True, f"{where}: the chord was left to the browser: {last}")
    return state


def close_search(walk: Walk) -> None:
    walk.page.focus("#search-field")
    closed = walk.key("Escape")
    check(closed["search"] is False, f"Escape did not close the search: {closed}")


def chord(walk: Walk, keys: str, where: str) -> Json:
    kb.press(walk.cdp, keys)
    state = searched(walk, where)
    walk.shot(where.replace(" ", "-").replace("'", "")[:48])
    close_search(walk)
    return state


def bare_slash_opens_nothing(walk: Walk, where: str) -> None:
    kb.press(walk.cdp, "/")
    state = walk.settled()
    check(state["search"] is False, f'a bare "/" {where} opened the search: {state}')


def from_the_box(walk: Walk) -> None:
    """The focus in the channel's box with a draft: the search, the draft as it was; a bare "/" there types a slash."""
    page = walk.page
    page.fill("#channel-compose-text", "draft words")
    page.focus("#channel-compose-text")
    kb.press(walk.cdp, "Control+/")
    searched(walk, "in the message box")
    check(value(page, "channel-compose-text") == "draft words",
          f"Ctrl+/ in the message box changed the draft: {value(page, 'channel-compose-text')!r}")
    walk.shot("chord-from-the-box")
    kb.press(walk.cdp, "d")
    kb.press(walk.cdp, "e")
    check(value(page, "search-field") == "de", f"after Ctrl+/ the typing went elsewhere: {value(page, 'search-field')!r}")
    close_search(walk)
    page.focus("#channel-compose-text")
    page.evaluate("() => { const box = document.getElementById('channel-compose-text');"
                  " box.setSelectionRange(box.value.length, box.value.length); }")
    kb.press(walk.cdp, "/")
    state = walk.settled()
    check(state["search"] is False, f'a bare "/" in the message box opened the search: {state}')
    check(value(page, "channel-compose-text") == "draft words/",
          f'a bare "/" in the message box typed no slash: {value(page, "channel-compose-text")!r}')
    page.fill("#channel-compose-text", "")
    walk.notes.append('Ctrl+/ in the message box opened the search with the draft untouched and took the typing; a '
                      'bare "/" there typed a slash')


def from_the_pace_popover(walk: Walk) -> None:
    """The pace popover open, the focus on its slider: the dialog of this page."""
    page = walk.page
    page.click("#read-speed")
    page.wait_for_selector("#speed-popover", state="visible", timeout=5000)
    page.focus("#read-speed-range")
    bare_slash_opens_nothing(walk, "on the pace popover's slider")
    kb.press(walk.cdp, "Control+/")
    searched(walk, "with the pace popover open")
    check(page.evaluate("() => document.getElementById('speed-popover').hidden") is True,
          "Ctrl+/ left the pace popover open")
    walk.shot("chord-from-the-pace-popover")
    close_search(walk)
    walk.notes.append('with the pace popover open, a bare "/" did nothing and Ctrl+/ closed it and searched')


def from_a_menu(walk: Walk) -> None:
    """A message's ⋯ menu open by ".", the focus on its first item."""
    page = walk.page
    page.focus("#scroll-area")
    kb.press(walk.cdp, "Home")
    kb.press(walk.cdp, ".")
    walk.settled()
    item = page.evaluate("() => Boolean(document.activeElement && document.activeElement.closest('.row-more-menu'))")
    check(item is True, "fixture: '.' did not put the focus in the ⋯ menu")
    kb.press(walk.cdp, "Control+/")
    searched(walk, "with a ⋯ menu open")
    open_menus = page.evaluate("() => [...document.querySelectorAll('.row-more-menu')].filter((m) => !m.hidden).length")
    check(open_menus == 0, f"Ctrl+/ left {open_menus} ⋯ menu(s) open")
    close_search(walk)
    walk.notes.append("with a ⋯ menu open, Ctrl+/ closed it and searched")


def from_the_picker(walk: Walk) -> None:
    walk.page.focus("#discord-channel")
    bare_slash_opens_nothing(walk, "on the channel picker")
    check(walk.page.evaluate(kb.PICKER_JS) == CHANNEL["id"], 'a bare "/" on the picker changed the channel')
    chord(walk, "Control+/", "on the channel picker")
    walk.notes.append('on the channel picker a bare "/" did nothing and Ctrl+/ searched')


def from_settings_and_help(walk: Walk) -> None:
    page = walk.page
    walk.page.focus("#scroll-area")
    walk.key(",", "screen-settings")
    check(walk.state()["screen"] == "settings", "',' did not open Settings")
    page.evaluate(SETTINGS_UNFOLD_JS)
    field = page.evaluate(SETTINGS_FIELD_JS)
    check(isinstance(field, str), "fixture: Settings shows no text field")
    assert isinstance(field, str)
    was = value(page, field)
    page.fill(f"#{field}", "words in a field")
    page.focus(f"#{field}")
    kb.press(walk.cdp, "Control+/")
    searched(walk, f"in #{field} on Settings")
    check(value(page, field) == "words in a field", f"Ctrl+/ on Settings changed #{field}: {value(page, field)!r}")
    close_search(walk)
    # What it held before, through Settings again, which keeps a value as it changes.
    page.click("#open-settings")
    page.fill(f"#{field}", was)
    page.click("#close-settings")
    walk.page.focus("#scroll-area")
    helped = walk.key("Shift+?", "screen-help")
    check(helped["screen"] == "help", f'"?" did not open Help: {helped}')
    chord(walk, "Control+/", "on Help")
    walk.notes.append(f"from #{field} on Settings, its words kept, and from Help, Ctrl+/ searched")


def from_the_right_column(walk: Walk) -> None:
    page = walk.page
    page.click("#column-toggle")
    page.wait_for_selector("#side-cards > li[data-id] .side-card-open", state="visible", timeout=10_000)
    page.click("#side-cards > li[data-id] .side-card-open")
    page.wait_for_selector("#side-compose-text", state="visible", timeout=10_000)
    page.fill("#side-compose-text", "side draft")
    page.focus("#side-compose-text")
    kb.press(walk.cdp, "Control+/")
    searched(walk, "in the right column's message box")
    check(value(page, "side-compose-text") == "side draft", "Ctrl+/ in the right column's box changed its draft")
    walk.shot("chord-from-the-right-column")
    close_search(walk)
    page.fill("#side-compose-text", "")
    page.click("#column-toggle")
    page.wait_for_function("() => document.getElementById('side-column').getClientRects().length === 0", timeout=5000)
    walk.notes.append("in two columns, from the right column's message box, Ctrl+/ searched with its draft kept")


def past_a_listener_that_stops_it(walk: Walk) -> None:
    """A listener on the box stops every key and refuses its default: the window's capture phase hears it first."""
    page = walk.page
    page.evaluate(HOSTILE_ON_JS)
    page.fill("#channel-compose-text", "kept")
    page.focus("#channel-compose-text")
    kb.press(walk.cdp, "Control+/")
    # The recorder listens at the window after the page, so the stopped key never reaches it: refused is
    # read off the page itself instead, the search open and the box holding no slash.
    searched(walk, "past a listener on the box that stops every key", refused=False)
    check(value(page, "channel-compose-text") == "kept", "a stopped Ctrl+/ changed the draft")
    page.evaluate(HOSTILE_OFF_JS)
    close_search(walk)
    page.fill("#channel-compose-text", "")
    walk.notes.append("a listener on the box that stopped every key and refused its default did not keep Ctrl+/ from "
                      "the search")


def from_reply(walk: Walk) -> None:
    """The Reply screen with a draft: left with it kept, and the reply opened again holds it."""
    page = walk.page
    page.focus("#scroll-area")
    kb.press(walk.cdp, "End")
    walk.settled()
    row = page.evaluate("() => document.activeElement && document.activeElement.dataset ? document.activeElement.dataset.id : null")
    check(isinstance(row, str), f"fixture: End selected no message: {row!r}")
    replied = walk.key("r")
    check(replied["screen"] == "reply", f"fixture: r did not open the Reply screen: {replied}")
    page.fill("#reply-text", "reply words")
    page.focus("#reply-text")
    kb.press(walk.cdp, "Control+/")
    searched(walk, "on the Reply screen")
    walk.shot("chord-from-reply")
    close_search(walk)
    page.click(f"#discord-log > li[data-id='{row}'] .reply-button")
    page.wait_for_function("() => !document.getElementById('screen-reply').hidden", timeout=5000)
    check(value(page, "reply-text") == "reply words", f"Ctrl+/ on Reply lost the draft: {value(page, 'reply-text')!r}")
    page.fill("#reply-text", "")
    page.click("#close-reply")
    page.wait_for_function("() => !document.getElementById('screen-main').hidden", timeout=5000)
    walk.notes.append("on the Reply screen, Ctrl+/ left it for the search and the reply opened again held its draft")


def with_single_keys_off(walk: Walk) -> None:
    page = walk.page
    page.click("#open-settings")
    page.wait_for_selector("#single-keys", state="attached", timeout=5000)
    page.evaluate("() => { const box = document.getElementById('single-keys'); box.checked = false;"
                  " box.dispatchEvent(new Event('change')); }")
    page.click("#close-settings")
    page.focus("#scroll-area")
    bare_slash_opens_nothing(walk, "on the list with single keys off")
    title = page.evaluate("() => document.getElementById('search-toggle').title")
    check(title == "Search messages (Ctrl+/)", f"with single keys off the glass says {title!r}")
    chord(walk, "Control+/", "on the list with single keys off")
    page.click("#open-settings")
    page.evaluate("() => { const box = document.getElementById('single-keys'); box.checked = true;"
                  " box.dispatchEvent(new Event('change')); }")
    page.click("#close-settings")
    walk.notes.append('with single keys off, a bare "/" did nothing and Ctrl+/ searched')


def help_chord(walk: Walk) -> None:
    page = walk.page
    page.focus("#scroll-area")
    helped = walk.key("Control+Shift+?", "screen-help")
    check(helped["screen"] == "help", f"Ctrl+Shift+? did not open Help: {helped}")
    check(walk.last_key()["prevented"] is True, "Ctrl+Shift+? was left to the browser")
    entry = page.evaluate("() => document.getElementById('help-keyboard').open")
    check(entry is True, "Ctrl+Shift+? did not open Help at the Keyboard entry")
    keys = str(page.evaluate("() => document.getElementById('keyboard-keys').textContent"))
    for words in ("Ctrl+/ searches from anywhere", "Ctrl+? is Ctrl+Shift+/", "? or Ctrl+?", "/ or Ctrl+/"):
        check(words in keys, f"the list of keys does not say {words!r}")
    shortcuts = page.evaluate("() => [document.getElementById('search-toggle').getAttribute('aria-keyshortcuts'),"
                              " document.getElementById('help-link-keyboard').getAttribute('aria-keyshortcuts')]")
    check(shortcuts == ["/ Control+/ Meta+/", "? Control+? Meta+?"], f"aria-keyshortcuts: {shortcuts}")
    walk.shot("help-by-ctrl-shift-question")
    back = walk.key("Escape")
    check(back["screen"] == "main", f"Escape did not leave Help: {back}")
    walk.notes.append("Ctrl+Shift+? opened Help at the Keyboard entry, whose list names Ctrl+/ and Ctrl+?")


def layouts_and_repeats(walk: Walk) -> None:
    page = walk.page
    control, alt, shift = kb.MODIFIERS["Control"], kb.MODIFIERS["Alt"], kb.MODIFIERS["Shift"]
    # German: "/" is Shift+7, so Ctrl+/ there arrives as key "/" on the Digit7 key, with Shift.
    page.focus("#channel-compose-text")
    press_raw(walk.cdp, "/", "Digit7", 55, control | shift)
    searched(walk, "as Ctrl+Shift+7 on a German layout")
    close_search(walk)
    # AltGr, as Windows sends it: Ctrl and Alt together. Never the search, and the key left alone.
    page.focus("#channel-compose-text")
    press_raw(walk.cdp, "/", "Digit7", 55, control | alt, "/")
    state = walk.settled()
    check(state["search"] is False, f'a "/" typed with AltGr opened the search: {state}')
    check(walk.last_key()["prevented"] is False, f'a "/" typed with AltGr was refused: {walk.last_key()}')
    page.fill("#channel-compose-text", "")
    # Held: one search, every repeat refused.
    page.focus("#scroll-area")
    page.evaluate(COUNT_SELECTS_JS)
    before = recorded(walk)
    kb.hold(walk.cdp, "Control+/", 3)
    searched(walk, "a held Ctrl+/")
    held = recorded(walk)[len(before):]
    check(len(held) == 4 and all(one["prevented"] is True for one in held),
          f"a held Ctrl+/'s keydowns were not all refused: {held}")
    selects = page.evaluate("() => window.__selects")
    check(selects == 1, f"a held Ctrl+/ selected the search field {selects} times")
    close_search(walk)
    walk.notes.append('Ctrl+Shift+7 reported as "/" (German) searched; a "/" with AltGr opened nothing and was not '
                      "refused; a held Ctrl+/ searched once, its repeats refused")


def mac(walk: Walk) -> None:
    page = walk.page
    meta, shift = kb.MODIFIERS["Meta"], kb.MODIFIERS["Shift"]
    page.fill("#channel-compose-text", "mac draft")
    page.focus("#channel-compose-text")
    kb.press(walk.cdp, "Meta+/")
    searched(walk, "Cmd+/ in the message box on a Mac")
    check(value(page, "channel-compose-text") == "mac draft", "Cmd+/ changed the draft")
    close_search(walk)
    page.fill("#channel-compose-text", "")
    for how in ("Cmd+Shift+?", 'Cmd+Shift+/ reported as "/"'):
        page.focus("#scroll-area")
        if how == "Cmd+Shift+?":
            kb.press(walk.cdp, "Meta+Shift+?")
        else:
            press_raw(walk.cdp, "/", "Slash", 191, meta | shift)
        helped = walk.settled("screen-help")
        check(helped["screen"] == "help", f"{how} on a Mac did not open Help: {helped}")
        check(walk.last_key()["prevented"] is True, f"{how} on a Mac was left to the browser")
        walk.key("Escape")
    page.focus("#scroll-area")
    press_raw(walk.cdp, "/", "Digit7", 55, meta | shift)
    searched(walk, 'Cmd+Shift+7 reported as "/" on a Mac')
    close_search(walk)
    page.click("#open-settings")
    page.evaluate("() => { const box = document.getElementById('single-keys'); box.checked = false;"
                  " box.dispatchEvent(new Event('change')); }")
    hint = str(page.evaluate("() => document.getElementById('single-keys-hint').textContent"))
    page.evaluate("() => { const box = document.getElementById('single-keys'); box.checked = true;"
                  " box.dispatchEvent(new Event('change')); }")
    page.click("#close-settings")
    check("Cmd+? lists them" in hint, f"the Mac's Settings hint with single keys off says {hint!r}")
    walk.notes.append('on an emulated Mac, Cmd+/ searched from the box, Cmd+Shift+? and Cmd+Shift+/ reported as "/" '
                      'opened Help, Cmd+Shift+7 reported as "/" searched, and the Settings hint said Cmd+?')


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=Path(__file__).resolve().parents[1] / "web",
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
    return parser.parse_args()


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

    api = kb.FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"

    try:
        with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1280, "height": 800},
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            cdp = context.new_cdp_session(page)
            walk = Walk(page, cdp, args.screenshots)

            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
            page.evaluate(kb.RECORD_KEYS_JS)
            walk.settled()
            walk.shot("loaded")

            from_the_box(walk)
            from_the_pace_popover(walk)
            from_a_menu(walk)
            from_the_picker(walk)
            from_settings_and_help(walk)
            from_the_right_column(walk)
            past_a_listener_that_stops_it(walk)
            from_reply(walk)
            with_single_keys_off(walk)
            help_chord(walk)
            layouts_and_repeats(walk)

            # A Mac, as `navigator.platform` reports it there: a second tab of the signed-in profile.
            mac_page = context.new_page()
            mac_page.on("pageerror", lambda error: errors.append(str(error)))
            mac_cdp = context.new_cdp_session(mac_page)
            agent = str(page.evaluate("() => navigator.userAgent"))
            mac_cdp.send("Emulation.setUserAgentOverride", {"userAgent": agent, "platform": "MacIntel"})
            mac_page.goto(url, wait_until="load")
            mac_page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            if mac_page.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
                mac_page.click("#view-switch")
            mac_page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
            check(mac_page.evaluate("() => navigator.platform") == "MacIntel", "the Mac's platform was not emulated")
            mac_page.evaluate(kb.RECORD_KEYS_JS)
            mac_walk = Walk(mac_page, mac_cdp, args.screenshots)
            mac_walk.step = walk.step
            mac(mac_walk)
            walk.notes.extend(mac_walk.notes)

            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()

        print(f"{browser_version} at 1280x800, keys through CDP Input.dispatchKeyEvent: " + "; ".join(walk.notes))
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
