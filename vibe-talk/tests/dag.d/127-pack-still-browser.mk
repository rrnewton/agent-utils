.PHONY: pack-still-browser
# `#459 pack-keeps-card-still` and `#460 pack-gap-slide` in a real Chromium at 1280x800 and 393x852: a card
# packed at the foot of the Threads view still in every frame, held past the foot in room that gives way one
# way and to a live reply, what is below holding and then sliding into the gap, the mirror for unpack, and
# nothing animated under reduced motion. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
pack-still-browser:
	python3 tests/pack_still_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
