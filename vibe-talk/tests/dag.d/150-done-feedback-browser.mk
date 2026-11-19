.PHONY: done-feedback-browser
# `#245 done-feedback` in a real Chromium: at 1280x800 in both schemes, a selected unread message ringed
# solid in the accent and a selected read one dashed in grey on the same lit surface, each at 3:1 or
# more against it; e draws the moment on the row it left and lets it go; the Undo chip names the
# message, and an Undo inside the hold takes the ring away at once; reduced motion gets the look with
# no animation; at 412x915 and 1280x800 in both schemes, the pictures; on a 360px and a 412px phone the
# Undo chip says only "Undo", taking no line, and at 1280 it names the message on one line; and at all
# three, walked up from the end of the list through the message box, no chip or status pill meets the
# box or its Send. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each step.
done-feedback-browser:
	python3 tests/done_feedback_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
