.PHONY: thread-card-focus-browser
# `#277 thread-card-focus` in a real Chromium, at 1280x800 over the right column's cards with a mouse and
# the keys, and on a 412x915 phone over the Threads list with a finger: every card contracted, the one
# under the mouse too; a click or a tap highlights one, shown whole and not entered, and every other
# card's edge and every other link turns the page's grey — by the colour Chromium computes and by the
# pixels a screenshot holds there — the highlighted card's staying in its colour; its replies button
# enters its thread, <- or Back comes out to it still highlighted, Esc puts the highlight out, and -> enters
# the card the keys are on. `#281 view-keys`: then the phone's thread list, chosen in the view picker, the
# same cards across the window: the channel's highlight on them, the others grey, each replies button a
# thumb high, a tap highlighting and showing one whole, its replies button opening the thread across the
# window and Back coming to the list. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
thread-card-focus-browser:
	python3 tests/thread_card_focus_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
