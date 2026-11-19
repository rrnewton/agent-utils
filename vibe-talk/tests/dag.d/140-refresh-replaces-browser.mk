.PHONY: refresh-replaces-browser
# `#1839 vibe-talk-composer-stays-in-view-while-dictating` (the re-render half): a live
# poll refresh whose only difference is the server's restamped `observed_ms` replaces
# zero rows in `#discord-log` and zero cards in `#thread-list`, in a real Chromium at
# 412x915 (touch) and 1280x800 (mouse). Pass WEB_ROOT=dir against another copy of the
# page, and SCREENSHOTS=dir to keep a PNG of each state.
refresh-replaces-browser:
	python3 tests/refresh_replaces_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
