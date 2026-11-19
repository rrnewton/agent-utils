.PHONY: incremental-refresh-browser
# `#1846 vibe-talk-windowed-rendering` step 1 in a real Chromium at 412x915 (touch) and
# 1280x800 (mouse), dark: a no-change poll replaces zero rows, one upstream edit rebuilds
# exactly its row, one arrival keeps the rest. Pass WEB_ROOT=dir, SCREENSHOTS=dir as usual.
incremental-refresh-browser:
	python3 tests/incremental_refresh_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
