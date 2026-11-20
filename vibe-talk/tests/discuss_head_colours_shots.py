#!/usr/bin/env python3
"""#1960 Task B: shoot the Discuss bar colour mock-ups as contact sheets.

Writes light-contact.png and dark-contact.png from
discuss_head_colours_mock.html (beside this file), one per theme, each
showing four 412x915 phone frames with the Discuss bar at half height
in the middle of the screen. This is a colour mock-up only; the real
feature is evidenced through the real server by scripts/run.sh
--screenshots, never by this page.

Needs Playwright/Chromium. On a host without the browser libraries it
exits 2 saying so; shoot then on another host or after the reinstall.
Condition-based waits only (owner rule 346): it waits for the mock
document to be complete, never for a wall-clock duration.
"""
import argparse
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
MOCK = HERE / "discuss_head_colours_mock.html"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="output directory for light-contact.png and dark-contact.png")
    args = parser.parse_args()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        print("blocked: playwright is not installed on this host; shoot on a host with a browser", file=sys.stderr)
        return 2
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(viewport={"width": 1820, "height": 1100})
            page.goto(MOCK.as_uri())
            page.wait_for_function("() => document.readyState === 'complete'")
            for theme in ("dark", "light"):
                section = page.locator(f"section[data-theme='{theme}']")
                section.scroll_into_view_if_needed()
                page.wait_for_function(
                    "(sel) => { const el = document.querySelector(sel); const r = el.getBoundingClientRect(); return r.height > 0; }",
                    arg=f"section[data-theme='{theme}']",
                )
                section.screenshot(path=str(out / f"{theme}-contact.png"))
            browser.close()
    except Exception as exc:  # browser libraries missing etc: report, do not fake sheets
        print(f"blocked: browser could not run on this host: {exc}", file=sys.stderr)
        return 2
    print(f"wrote {out / 'dark-contact.png'} and {out / 'light-contact.png'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
