.PHONY: typing-lag-browser
# `#2429 vibe-talk-typing-never-lags` in a real Chromium at 1280x800 and 412x915 by touch: 100 keys in the
# channel composer and in the reply box over 100k px of rows walk no list text and read a constant
# handful of boxes each. Pass WEB_ROOT=dir to run it against another copy of the page (main before the
# change walks every row's text at each key).
typing-lag-browser:
	python3 tests/typing_lag_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
