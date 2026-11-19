.PHONY: pace-stepper-browser
# `#567 pace-stepper` in a real Chromium at 393x852 by touch and 1280x800: the Pace popover's [-] [100%] [+]
# stepper, its steps, typed and clamped values, arrows, and a change mid-read. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
pace-stepper-browser:
	python3 tests/pace_stepper_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
