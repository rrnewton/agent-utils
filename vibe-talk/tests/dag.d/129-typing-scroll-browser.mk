.PHONY: typing-scroll-browser
# `#1144 scrollback-typing-jump` in a real Chromium at 412x915 and 360x800 by touch and 1280x800: typing
# at the foot follows arrivals, strip resizes and a row grown late with the whole composer on screen;
# reading scrollback holds across arrivals; unfocused keys navigate; hover between the columns scrolls
# neither. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir for PNGs.
typing-scroll-browser:
	python3 tests/typing_scroll_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
