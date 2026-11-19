.PHONY: thread-slug-browser
# `#1819 vibe-talk-thread-button-shows-slug` in a real Chromium at 412x915 (touch, scale 2.6) and
# 1280x800 (mouse), light and dark, keyboard open and closed, Discuss open on the phone: a slugged
# root's tag reads "🧵 my-thread-slug (57)" with a greyed emoji, an unslugged one keeps Thread(N),
# the grey start tag is unchanged, a narrowed list keeps the tag in its row, and the narrowest
# falls back. Pass WEB_ROOT=dir against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
thread-slug-browser:
	python3 tests/thread_slug_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
