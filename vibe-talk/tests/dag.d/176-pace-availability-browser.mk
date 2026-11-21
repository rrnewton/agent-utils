.PHONY: pace-availability-browser
# `#2053 vibe-talk-pace-has-no-effect` in a real Chromium at 412 by touch and 1280, in the light and
# the dark theme: Pace greyed with "Pace isn't available with this voice" for the conversational
# cascade, and live for a capable voice. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep PNGs.
pace-availability-browser:
	python3 tests/pace_availability_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
