.PHONY: reply-continuity-browser
# `#247 reply-continuity` in a real Chromium at 1280x800 (mouse) and 412x915 (touch), at 80%, 100% and
# 130% text: Reply on a message in the middle of the list, just under the list's top, half under its top
# edge, low on the screen, a reply in its gutter, and one taller than the screen. Since `#410
# inline-reply-editor` the reply opens in the list: the row is not moved (a low one only as far as shows
# the box), the box directly under it on its tint, and Cancel moves nothing but the box. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each.
reply-continuity-browser:
	python3 tests/reply_continuity_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
