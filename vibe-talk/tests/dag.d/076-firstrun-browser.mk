.PHONY: firstrun-browser
# `#1781 vibe-talk-first-run-welcome-channel-default` in a real Chromium at 412x915 (scale 2.6) and
# 1280x800, light and dark: a fresh sign-in meets the Welcome once and dismisses onto the Channel,
# a reload and a saved Voice choice meet neither surprise, the Welcome re-opens from Settings and
# Help, the no-channel Welcome is the guided config setup whose Check again lands on the channel
# once configured, and `?v=docs` still wins. Pass WEB_ROOT=dir to run it against another copy of
# the page, and SCREENSHOTS=dir to keep a PNG of each state.
firstrun-browser:
	python3 tests/firstrun_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
