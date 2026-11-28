.PHONY: read-run-line-browser
# `#2419 vibe-talk-read-run-separator-over-label` in a real Chromium at 412x915 by touch, 1280x800 and
# 1117x800, light and dark: a folded run's line with pins stays centred between rules that never leave its
# flow. Pass WEB_ROOT=dir to run it against another copy of the page (main before the change fails
# every pinned, reply and selected head), and SCREENSHOTS=dir to keep a PNG per case.
read-run-line-browser:
	python3 tests/read_run_line_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
