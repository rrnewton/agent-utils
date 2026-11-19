.PHONY: keyboard-hides-bar-browser
# `#429 keyboard-hides-bar` in a real Chromium at 393x852 by touch, the on-screen keyboard simulated by a
# shorter viewport as Chrome on Android lays it out: both bars away while it is up and back as it goes or
# the box loses the focus, the reader's place kept, and a slim Stop while reading; and a desk's shorter
# window keeping both bars. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
keyboard-hides-bar-browser:
	python3 tests/keyboard_hides_bar_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
