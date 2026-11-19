.PHONY: scroll-stability-browser
# `#280 scroll-stability` in a real Chromium at 1280x800 in two columns (mouse) and 412x915 in one (touch):
# the reader scrolled up in Main stays on the same row, within a pixel, while a message and a reply arrive
# live, a refresh redraws the list with a message above them grown, a read sent while they were on the
# newest line lands after they scrolled up (the owner's jump), summaries arrive, and the focus moves, and
# at the desk while the mouse crosses the cards to the dock or the top bar, rests, or crosses back —
# mouse movement or hover scrolls nothing (`#1144 scrollback-typing-jump`) — and ] is pressed on its
# own, each sampled every frame for 2.5 s; a reader on the newest line follows a live message to the foot; the list of threads and a thread open at
# their foot and stay there, older replies coming in under a reader at a thread's head; after a
# reload the read behind the saved rows leaves the reader where they scrolled; `#281 view-keys`: the view
# keys show each view where it is read and nothing moves it for 2.5 s after, the thread list opening at its
# foot from two columns, Main and All, Main coming back from it where the reader left it, a thread
# opened from it opening at its foot, and a reload in it opening it at its foot with the desk's dock on two
# rows; and, last, a card highlighted by a click, the mouse going on over another card to the dock, or
# resting on the highlighted card, leaves Main still. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each stage.
scroll-stability-browser:
	python3 tests/scroll_stability_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
