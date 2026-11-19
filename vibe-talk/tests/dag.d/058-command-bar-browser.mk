.PHONY: command-bar-browser
# `#406 command-bar-one-row` in a real Chromium at 1280x800, 1920x1080 and 900x700: the desk's dock is one
# row across the window on a 120-character thread, the thread list, Main, All and two columns, its view
# picker's tooltip naming the thread whole; a 393x852 phone's pane is one row above the bar. Pass
# BASELINE_WEB_ROOT=dir (a copy of the page from before) to require the phone's dock to match it to the
# pixel, WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each.
command-bar-browser:
	python3 tests/command_bar_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(BASELINE_WEB_ROOT),--baseline-web-root $(BASELINE_WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
