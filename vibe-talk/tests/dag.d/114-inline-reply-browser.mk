.PHONY: inline-reply-browser
# `#410 inline-reply-editor` in a real Chromium at 1280x800 in two columns and one, and 393x852 by touch:
# Reply opens the reply's box in the stream, under the message and the replies gathered under it, later
# rows below, one message box on screen; Grow, Send, Cancel, a refused reply, the view keys, and two
# columns collapsing with a reply open. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each open reply.
inline-reply-browser:
	python3 tests/inline_reply_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
