.PHONY: composer-stays-browser
# `#1839 vibe-talk-composer-stays-in-view-while-dictating` in a real Chromium at 412x915
# (touch) and 1280x800 (mouse), in the dark scheme, keyboard really open: dictating into a
# drilled thread's composer through live arrivals and a refresh never moves it out of the
# visual viewport and never jumps scrollTop. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
composer-stays-browser:
	python3 tests/composer_stays_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
