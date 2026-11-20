.PHONY: windowed-all-browser
# `#1985 vibe-talk-windowed-all-view-fixes`: windowed All view chips, fold anchor,
# END/Newest and the settle oscillation detector in a real Chromium at 412x915 and
# 1280x800, windowed on and off. Pass WEB_ROOT=dir, SCREENSHOTS=dir as usual.
windowed-all-browser:
	python3 tests/windowed_all_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
