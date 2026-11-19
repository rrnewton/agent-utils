.PHONY: windowed-refresh-browser
# `#1846 vibe-talk-windowed-rendering` step 3 budgets in a real Chromium at 412x915 and
# 1280x800: 5000 messages mount at most the cap, one screenful mounts at most 20, an
# off-screen edit does no DOM work. Pass WEB_ROOT=dir, SCREENSHOTS=dir as usual.
windowed-refresh-browser:
	python3 tests/windowed_refresh_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
