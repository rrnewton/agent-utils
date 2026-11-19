.PHONY: pairing-link-browser
# `#369 pairing-link` in a real Chromium at a phone's size: a pairing link signs a fresh browser in
# once, its code out of the address before any request; the same link in a second browser is refused.
# Pass WEB_ROOT=dir to run it against another copy of the page.
pairing-link-browser:
	python3 tests/pairing_link_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
