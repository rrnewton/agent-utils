.PHONY: threads-unpack-browser
# `#404 threads-unpack-all` and `#405 root-under-summary` in a real Chromium at 1280x800 with a mouse
# and the keys and 393x852 by touch, in both schemes: Unpack msgs draws every thread's messages under
# its card on a bridge in the thread's colour, the summarised thread's root first as the card's child
# and a raw thread with no root row; Collapse read folds each block's read run onto its spine and Hide
# read leaves read replies out while keeping the summarised root; off is cards alone, and a reload
# keeps it on. Forty threads say what unpacking costs. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
threads-unpack-browser:
	python3 tests/threads_unpack_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
