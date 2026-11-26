.PHONY: windowed-composer-foot-browser
# `#2292 vibe-talk-windowed-composer-foot-fight` in a real Chromium at 412x915 by touch (keyboard up)
# and 1280x800, windowed lists on and off: typing at the foot through arrivals, every scroll write is
# the composer keep and the composer moves under one whole pixel; a jump into a windowed list has a
# row in the band as its scroll is answered. Pass WEB_ROOT=dir to run it against another copy of the page.
windowed-composer-foot-browser:
	python3 tests/windowed_composer_foot_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
