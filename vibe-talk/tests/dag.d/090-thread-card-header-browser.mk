.PHONY: thread-card-header-browser
# `#298 thread-card-header` in a real Chromium at 1280x800 with a mouse, in both schemes: a card's replies
# button opens its thread on the right under its card at the top of the column — the card's words, place,
# height and colour, by computed colour and pixels, the X on it — joined to the thread's root on Main by
# one line, clipped at Main's top while the root is above it, and a mouse resting on the card or crossing
# it scrolling nothing (`#1144 scrollback-typing-jump`); a long question's card the same height, and Esc closing
# the thread; the card moving up from its
# place in the list, frame by frame, with nothing under it moving; the same card heading a thread opened
# from the thread list (real Ctrl+3) across the window, at the desk and on a 412x915 phone by touch;
# nothing moving under reduced motion; and a redraw of Main leaving the keys in the right column, so Esc
# still closes its thread (`#284 redraw-keeps-pane`). Pass WEB_ROOT=dir to run it against another copy of
# the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-card-header-browser:
	python3 tests/thread_card_header_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
