#!/usr/bin/env python3
"""The bar's channel picker grouped by chat service, in real Chromium at a desk's size and a phone's.

`#355 provider-grouped-channels`. The owner, 2026-10-18: with several chat services in one server, the
channel drop-down should be "segmented/grouped ... PER service". Once two or more services have
channels, the picker on the bar holds one `<optgroup>` per service, in the order the server configures
them, and Alt+Down and Alt+Up step through the channels in the order shown, across groups. The fake-DOM
suite (tests/js/voice_page.test.mjs) pins the groups the page builds, but its `<select>` is a plain
object: nothing there proves that a value inside an `<optgroup>` is chosen, restored after a reload, or
stepped through as a real picker holds it. This does, with keys pressed through CDP
`Input.dispatchKeyEvent`:

  1280x800: signed in, on the channel view -> the picker holds a "Slack" group and then a "Google
  Chat" group, each with its own channels, although the server lists the four interleaved; the
  closed picker names the channel alone; with nothing stored it is on the first channel shown ->
  Alt+Down from the list steps to the second Slack channel, then across into Google Chat's first,
  then its second, and stops there; Alt+Up crosses back -> a real reload: the picker grouped again
  and on the channel chosen, inside its group -> Ctrl+K focuses the picker, and a channel chosen
  from it is read -> Settings, its Channels section unfolded: the Service row, inside the window,
  opens on the service of the channel being read and lists only its channels; choosing the other
  service lists the other's, and its facts line names it.
  375x812, a phone: the same groups, the picker inside the window and nothing scrolling sideways;
  Settings' Service row and channel picker inside the window, nothing scrolling sideways there.

It reuses keyboard_browser's fixture (its fake API, asset server and CDP keys), with four channels on
two services. --web-root DIR runs it against another copy of the page; against one from before the
change it fails at the first check, a flat picker. --screenshots DIR keeps a PNG of each step.
"""

from __future__ import annotations

import argparse
import tempfile
import threading
from dataclasses import dataclass
from http.server import ThreadingHTTPServer
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import CHANNEL, SECOND, TOKEN, Json, check

if TYPE_CHECKING:
    from pathlib import Path

    from playwright.sync_api import BrowserContext, CDPSession, Page, Playwright

# Four channels on two services, as a server might list them: interleaved.
SPACE_A = {**CHANNEL, "label": "lead team", "provider": "gchat"}
TEAM_A = {**SECOND, "label": "release", "provider": "slack"}
SPACE_B = {**CHANNEL, "id": "1110000000000000003", "label": "build noise", "provider": "gchat"}
TEAM_B = {**CHANNEL, "id": "1110000000000000004", "label": "general", "provider": "slack"}
LISTED = [SPACE_A, TEAM_A, SPACE_B, TEAM_B]
# Slack configured first, so its group comes first.
GROUPED = [
    {"group": "Slack", "channels": [TEAM_A["id"], TEAM_B["id"]]},
    {"group": "Google Chat", "channels": [SPACE_A["id"], SPACE_B["id"]]},
]
FIRST_ROW = "#discord-log > li[data-id]"

# Settings' Service row and channel picker as the browser draws them: what each holds, and their boxes
# against the window and the Settings screen.
SETTINGS_JS = """() => {
  const box = (id) => { const r = document.getElementById(id).getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
  const screen = document.getElementById('screen-settings');
  return {
    row: document.getElementById('settings-service-row').checkVisibility(),
    line: document.getElementById('chat-provider-line').checkVisibility(),
    service: document.getElementById('settings-service').value,
    services: [...document.getElementById('settings-service').options].map((o) => o.textContent),
    channels: [...document.getElementById('settings-channel').options].map((o) => o.value),
    facts: document.getElementById('service-facts').textContent,
    boxes: ['settings-service', 'settings-channel', 'service-facts'].map(box),
    width: window.innerWidth, sideways: screen.scrollWidth - screen.clientWidth,
  };
}"""


def service(key: str, name: str) -> Json:
    return {"key": key, "name": name, "threading_supported": True, "live_delivery": "poll",
            "live_poll_seconds": 30, "channel_registration_supported": False,
            "channel_discovery_supported": False, "upstream_read_mark_supported": False,
            "self_author_id": None, "owner_author_id": None}


@dataclass
class GroupsApi(kb.FakeApi):
    """keyboard_browser's channel, read as each of four channels on two services."""

    def client_config(self) -> Json:
        return {**super().client_config(), "chat_provider_name": "Slack and Google Chat",
                "channels": LISTED, "providers": [service("slack", "Slack"), service("gchat", "Google Chat")]}


# The picker as the browser holds it: its groups, its value, the name it shows closed, and its box.
PICKER_JS = """() => {
  const picker = document.getElementById('discord-channel');
  const r = picker.getBoundingClientRect();
  return {
    value: picker.value,
    shown: picker.selectedOptions.length ? picker.selectedOptions[0].textContent : null,
    groups: [...picker.children].map((child) => child.tagName === 'OPTGROUP'
      ? {group: child.label, channels: [...child.children].map((option) => option.value)} : child.value),
    left: r.left, right: r.right, width: window.innerWidth,
    sideways: document.documentElement.scrollWidth - window.innerWidth,
    focus: document.activeElement ? document.activeElement.id : null,
  };
}"""


def picker(page: Page) -> Json:
    found = page.evaluate(PICKER_JS)
    assert isinstance(found, dict)
    return found


def number(found: Json, key: str) -> float:
    value = found[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}, not a number"
    return float(value)


def on_channel(page: Page, channel: Json, why: str) -> Json:
    """Wait for the picker to be on `channel` and its rows to be drawn; then the picker as it is."""
    page.wait_for_function("(id) => document.getElementById('discord-channel').value === id",
                           arg=str(channel["id"]), timeout=10_000)
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    found = picker(page)
    check(found["shown"] == channel["label"], f"{why}: the closed picker shows {found['shown']!r}, not the channel's name")
    return found


def settings(page: Page) -> Json:
    found = page.evaluate(SETTINGS_JS)
    assert isinstance(found, dict)
    return found


def settings_by_service(page: Page, reading: str, other: str, other_channels: list[str], where: str) -> None:
    """Settings, Channels unfolded: the Service row opens on `reading`'s service, inside the window;
    choosing `other` lists `other_channels` and its facts."""
    page.click("#open-settings")
    page.wait_for_selector("#close-settings", state="visible", timeout=10_000)
    if page.get_attribute("#fold-settings-channels", "aria-expanded") != "true":
        page.click("#fold-settings-channels")
    page.wait_for_function("() => document.getElementById('settings-service').checkVisibility()", timeout=10_000)
    page.evaluate("() => document.getElementById('settings-service').scrollIntoView({block: 'center'})")
    found = settings(page)
    check(found["row"] is True and found["line"] is False, f"{where}: the Service row is not shown in place of the line: {found}")
    check(found["services"] == ["Slack", "Google Chat"], f"{where}: the services are {found['services']}")
    check(found["service"] == reading, f"{where}: Settings opened on {found['service']!r}, not the service read ({reading})")
    width = number(found, "width")
    boxes = found["boxes"]
    assert isinstance(boxes, list)
    for name, one in zip(("the Service picker", "the channel picker", "the facts line"), boxes):
        assert isinstance(one, dict)
        check(float(one["left"]) >= 0 and float(one["right"]) <= width,
              f"{where}: {name} leaves the window: {one['left']}..{one['right']} of {width}")
    check(number(found, "sideways") <= 0, f"{where}: Settings scrolls sideways by {found['sideways']}px")
    page.select_option("#settings-service", other)
    page.wait_for_function("(key) => document.getElementById('service-facts').textContent.startsWith(key)",
                           arg="Google Chat:" if other == "gchat" else "Slack:", timeout=10_000)
    found = settings(page)
    listed = [value for value in found["channels"] if value != "*"] if isinstance(found["channels"], list) else []
    check(listed == other_channels, f"{where}: with {other} chosen Settings lists {listed}, not {other_channels}")


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"provider-groups-{name}.png"))


def sign_in(page: Page, url: str) -> None:
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
        page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)


def desk(context: BrowserContext, url: str, shots: Path | None) -> str:
    page = context.pages[0]
    cdp: CDPSession = context.new_cdp_session(page)
    sign_in(page, url)
    found = picker(page)
    check(found["groups"] == GROUPED, f"the picker is not grouped by service in configuration order: {found['groups']}")
    on_channel(page, TEAM_A, "signed in, nothing stored: the first channel shown")
    shot(page, shots, "desk-grouped")

    # Alt+Down and Alt+Up from the list, in the order shown.
    page.evaluate("() => document.getElementById('scroll-area').focus()")
    for chord, channel, why in (
        ("Alt+ArrowDown", TEAM_B, "Alt+Down from the first Slack channel"),
        ("Alt+ArrowDown", SPACE_A, "Alt+Down from the last Slack channel, across into Google Chat"),
        ("Alt+ArrowDown", SPACE_B, "Alt+Down within Google Chat"),
        ("Alt+ArrowDown", SPACE_B, "Alt+Down at the last channel shown"),
        ("Alt+ArrowUp", SPACE_A, "Alt+Up within Google Chat"),
    ):
        kb.press(cdp, chord)
        page.wait_for_timeout(200)
        on_channel(page, channel, why)

    # A real reload: the groups again, and the channel chosen restored inside its group.
    page.reload(wait_until="load")
    found = on_channel(page, SPACE_A, "after a reload")
    check(found["groups"] == GROUPED, f"after a reload the picker is not grouped: {found['groups']}")

    # Ctrl+K, and a channel chosen from the open picker.
    page.evaluate("() => document.getElementById('scroll-area').focus()")
    kb.press(cdp, "Control+k")
    page.wait_for_function("() => document.activeElement && document.activeElement.id === 'discord-channel'",
                           timeout=10_000)
    page.select_option("#discord-channel", str(TEAM_B["id"]))
    on_channel(page, TEAM_B, "chosen from Ctrl+K")
    shot(page, shots, "desk-chosen")
    settings_by_service(page, "slack", "gchat", [str(SPACE_A["id"]), str(SPACE_B["id"])], "at 1280x800")
    shot(page, shots, "desk-settings")
    return ("the picker held a Slack group then a Google Chat group from an interleaved list; Alt+Down "
            "crossed from Slack into Google Chat and stopped at the last, Alt+Up crossed back; a reload "
            "kept the groups and the channel inside its group; Ctrl+K and a choice read that channel; "
            "Settings opened on Slack, its Service row inside the window, and Google Chat listed its two")


def phone(context: BrowserContext, url: str, shots: Path | None) -> str:
    page = context.pages[0]
    sign_in(page, url)
    found = picker(page)
    check(found["groups"] == GROUPED, f"on a phone the picker is not grouped: {found['groups']}")
    found = on_channel(page, TEAM_A, "on a phone")
    left, right, width, sideways = (number(found, key) for key in ("left", "right", "width", "sideways"))
    check(left >= 0 and right <= width, f"on a phone the picker leaves the window: {left}..{right} of {width}")
    check(sideways <= 0, f"on a phone the page scrolls sideways by {sideways}px")
    shot(page, shots, "phone-grouped")
    settings_by_service(page, "slack", "gchat", [str(SPACE_A["id"]), str(SPACE_B["id"])], "at 375x812")
    shot(page, shots, "phone-settings")
    return ("at 375x812 the same groups, the picker inside the window and nothing scrolling sideways; "
            "Settings' Service row, channel picker and facts inside the window, nothing scrolling sideways")


def browse(playwright: Playwright, args: argparse.Namespace, url: str, width: int, height: int,
           mobile: bool) -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-groups-") as profile:
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, is_mobile=mobile, has_touch=mobile,
            device_scale_factor=3 if mobile else 1,
        )
        errors: list[str] = []
        context.pages[0].on("pageerror", lambda error: errors.append(str(error)))
        note = phone(context, url, args.screenshots) if mobile else desk(context, url, args.screenshots)
        check(not errors, f"the page threw: {errors}")
        version = context.browser.version if context.browser else "Chromium"
        context.close()
    return version, note


def main() -> int:
    args = kb.arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    # The fixture answers a channel's reads by its id; these two are new to it.
    kb.CHANNELS.update({str(SPACE_B["id"]): SPACE_B, str(TEAM_B["id"]): TEAM_B})

    api = GroupsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            version, desk_note = browse(playwright, args, url, 1280, 800, mobile=False)
            _, phone_note = browse(playwright, args, url, 375, 812, mobile=True)
        print(f"{version}, keys through CDP Input.dispatchKeyEvent: at 1280x800 {desk_note}; {phone_note}")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
