.PHONY: dark-composer-black-browser
# `#2294 vibe-talk-dark-composer-black` in a real Chromium at 412x915 by touch and 1280x800: in dark every
# box the reader writes in (Discuss's field too) is the message cards' black (`--theirs`), at rest, focused
# and with the keyboard up, and its words and placeholder keep 4.5:1 on it; in light every box keeps its own
# light surface. Pass WEB_ROOT=dir to run it against another copy of the page.
dark-composer-black-browser:
	python3 tests/dark_composer_black_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
