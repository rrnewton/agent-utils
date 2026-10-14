#!/usr/bin/env python3
"""Ask Chromium's DevTools protocol whether the app is installable, from / and from /voice.

The app is the page at / (`#302 root-is-the-app`). It used to live at /voice, which now answers a
302 to /, so this asks Chromium about both addresses as a browser reaches them:

* at /, the manifest is found with no errors, the page is installable, and the manifest says what
  an installed app needs: start_url /, scope /, standalone with the browser fallback;
* the manifest's id is still /voice, and Chromium resolves the same id from either address, so an
  app installed while the page lived at /voice is the same app, and takes the new start_url as an
  update rather than being a second, stranded installation;
* /voice?x=1#y is forwarded by a 302 and lands on /?x=1#y, the query kept by the server and the
  fragment carried over by the browser, and the page it lands on is installable too;
* every address an installed app can open, new or old, is inside the manifest's scope by
  Chromium's rule (same origin, path under the scope's), so a launch from either never leaves the
  app window.

The start_url, scope and id checked are Chromium's own reading of the manifest, resolved against
the page as its installer resolves them (`Page.getAppManifest`'s `manifest`), not this script's.

By default this starts this checkout's release binary on a loopback port, which Chromium treats as
secure: `cargo build --release --locked` first (the `pwa-installability` make target does). Pass
--url to inspect a deployed origin instead. This is a browser-engine check, not proof that a
physical Android launcher accepted and opened the installation.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urljoin, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, CDPSession, Page

READ_TOKEN = "pwa-installability-read-token-0000"
WRITE_TOKEN = "pwa-installability-write-token-000"
ANDROID_UA = (
    "Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"
)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def write_config(workdir: Path, port: int) -> Path:
    """The smallest configuration that serves the page.

    Nothing here reaches a chat service: the page opens signed out, so it asks the server for
    nothing but its own files, and the provider's address is a loopback port nobody listens on.
    """
    config = workdir / "vibe-talk.toml"
    config.write_text(f"""
[server]
bind = "127.0.0.1:{port}"

[discord]
bot_token = "pwa-installability-bot-token"
api_base = "http://127.0.0.1:{free_port()}"
live_poll_seconds = 0
request_timeout_seconds = 5

[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[channels]]
id = "1111111111111111111"
label = "installability"

[storage]
path = "{workdir}/state/vibe-talk.sqlite3"
""")
    return config


def start_server(binary: Path, config: Path, port: int, log: Path) -> subprocess.Popen[bytes]:
    env = {key: value for key, value in os.environ.items() if not key.startswith("VIBE_TALK_")}
    env["RUST_LOG"] = "warn"
    with log.open("wb") as sink:
        server = subprocess.Popen([str(binary), "--config", str(config), "--skip-startup-probe"],
                                  cwd=config.parent, stdout=sink, stderr=subprocess.STDOUT, env=env)
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if server.poll() is not None:
            raise RuntimeError(f"the server exited: {log.read_text(errors='replace')[-2000:]}")
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=2).read()
            return server
        except (urllib.error.URLError, OSError):
            time.sleep(0.2)
    stop(server)
    raise RuntimeError("the server never answered /healthz")


def stop(server: subprocess.Popen[bytes]) -> None:
    server.terminate()
    try:
        server.wait(timeout=10)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def in_scope(url: str, scope: str) -> bool:
    """Chromium's scope rule: the same origin, and the path, without query or fragment, under the scope's."""
    target, within = urlsplit(url), urlsplit(scope)
    same_origin = (target.scheme, target.netloc) == (within.scheme, within.netloc)
    return same_origin and target.path.startswith(within.path)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--url",
        help="origin of a deployed vibe-talk to inspect, e.g. https://host/; its / and its /voice are "
        "both asked about (default: start this checkout's release binary on a loopback port)",
    )
    parser.add_argument(
        "--server-bin",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "target" / "release" / "vibe-talk",
        help="the vibe-talk server binary to run when --url is not given "
        "(default: this checkout's target/release/vibe-talk)",
    )
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: $VIBE_TALK_CHROMIUM, else Playwright's bundled Chromium)",
    )
    return parser.parse_args()


def manifest_of(page: Page, session: CDPSession) -> dict[str, object]:
    """What Chromium made of the page's manifest, once it has fetched one."""
    manifest: dict[str, object] = {}
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        manifest = session.send("Page.getAppManifest")
        if manifest.get("url"):
            break
        page.wait_for_timeout(100)
    return manifest


def resolved(manifest: dict[str, object]) -> dict[str, object]:
    """Chromium's own reading of the manifest: its fields resolved as the installer resolves them."""
    reading = manifest.get("manifest")
    if not isinstance(reading, dict):
        raise AssertionError("this Chrome does not report its parsed manifest; use a current one")
    return {str(key): value for key, value in reading.items()}


def installability_errors(session: CDPSession) -> object:
    return session.send("Page.getInstallabilityErrors").get("installabilityErrors", [])


def inspect(context: BrowserContext, origin: str) -> list[str]:
    """Every check, against one origin. Returns what was verified, one line each."""
    page = context.pages[0]
    session = context.new_cdp_session(page)
    session.send("Page.enable")
    verified: list[str] = []

    # The app's own address.
    root = urljoin(origin, "/")
    page.goto(root, wait_until="load")
    check(page.url == root, f"{root} ended at {page.url}")
    manifest = manifest_of(page, session)
    check(bool(manifest.get("url")), f"Chrome did not discover a web app manifest at {root}")
    errors = manifest.get("errors", [])
    check(not errors, f"Chrome reported manifest errors at {root}:\n{json.dumps(errors, indent=2)}")
    blockers = installability_errors(session)
    check(not blockers, f"Chrome did not consider {root} installable:\n{json.dumps(blockers, indent=2)}")
    manifest_url = str(manifest["url"])
    parsed = json.loads(str(manifest.get("data", "{}")))
    check(parsed.get("display_override") == ["standalone"], f"Chrome read the wrong display_override: {parsed}")
    check(parsed.get("display") == "browser", f"Chrome read the wrong display fallback: {parsed}")
    reading = resolved(manifest)
    start_url, scope = str(reading.get("startUrl")), str(reading.get("scope"))
    check(start_url == root, f"a new installation would open {start_url}, not the app at {root}")
    check(scope == root, f"the manifest's scope is {scope}, not the whole origin {root}")
    identity = urljoin(origin, "/voice")
    check(
        reading.get("id") == identity,
        f"Chrome identifies the app as {reading.get('id')}, not {identity}: a copy installed from "
        "/voice would be a different app, and would never take the new start_url",
    )
    verified.append(f"{root} is installable: no manifest or installability errors")
    verified.append("start_url / and scope /; display_override [standalone], display browser")
    verified.append(f"app id {identity}, the one the app was installed under while it lived at /voice")

    # The old address, as a bookmark or an app installed before the move opens it.
    old = urljoin(origin, "/voice?x=1#y")
    landed = urljoin(origin, "/?x=1#y")
    response = page.goto(old, wait_until="load")
    check(page.url == landed, f"{old} landed on {page.url}, not {landed}")
    forwarded = response.request.redirected_from if response is not None else None
    hop = forwarded.response() if forwarded is not None else None
    check(hop is not None, f"{old} reached {page.url} without a redirect")
    status = hop.status if hop is not None else None
    check(status == 302, f"{old} was forwarded with {status}, not 302")
    again = manifest_of(page, session)
    check(again.get("url") == manifest_url, f"{landed} names a different manifest")
    blockers = installability_errors(session)
    check(not blockers, f"Chrome did not consider {landed} installable:\n{json.dumps(blockers, indent=2)}")
    check(resolved(again).get("id") == identity, f"{landed} is a different app from {root}")
    verified.append(f"{old} is a 302 to {landed}, query and fragment kept, and installable there")

    # Inside the scope, so neither launch leaves the app window.
    for address in (start_url, landed, urljoin(origin, "/voice"), urljoin(origin, "/voice/")):
        check(in_scope(address, scope), f"{address} is outside the scope {scope}")
    verified.append(f"/, /?x=1#y, /voice and /voice/ are all inside the scope {scope}")
    return verified


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error

    server: subprocess.Popen[bytes] | None = None
    with tempfile.TemporaryDirectory(prefix="vibe-talk-pwa-") as scratch:
        workdir = Path(scratch)
        try:
            origin = args.url
            if origin is None:
                binary = Path(args.server_bin)
                if not binary.is_file():
                    raise SystemExit(f"no server binary at {binary}: run `cargo build --release --locked` first")
                port = free_port()
                server = start_server(binary, write_config(workdir, port), port, workdir / "server.log")
                origin = f"http://127.0.0.1:{port}/"
            with sync_playwright() as playwright:
                # `browser.new_context()` is incognito by design, and Chrome correctly reports
                # `in-incognito` as an installation blocker there. A throwaway persistent profile
                # is still isolated while exercising the ordinary installability path.
                context = playwright.chromium.launch_persistent_context(
                    str(workdir / "profile"),
                    headless=True,
                    executable_path=args.browser_executable,
                    user_agent=ANDROID_UA,
                    viewport={"width": 412, "height": 915},
                    device_scale_factor=2.625,
                    is_mobile=True,
                    has_touch=True,
                )
                try:
                    verified = inspect(context, origin)
                    version = context.new_cdp_session(context.pages[0]).send("Browser.getVersion")["product"]
                finally:
                    context.close()
        finally:
            if server is not None:
                stop(server)

    print(f"Chrome {version}, {origin}:")
    for line in verified:
        print(f"  {line}")
    print("CDP verification passed; physical Android installation was not exercised")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
