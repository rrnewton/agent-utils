.PHONY: view-switch-browser
# `#220 view-switch-instant` in a real Chromium: All to Main and back with every timeline request
# intercepted -- no read, never an empty list or the loading line, the reader's message kept -- All
# never read drawn at once from held rows and read once behind them, and, paged, Main drawn from All
# keeping its way back and All walked back to the reader's message before it is drawn. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
view-switch-browser:
	python3 tests/view_switch_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
