.PHONY: server-thread-hues-browser
# `#427 server-thread-hues` in a real Chromium at 1280x800: two browser contexts, each remembering the
# same six threads in different colours of its own, both drawn in the colours the first offered the
# server and the server took; a reload of the second draws them still. SCREENSHOTS=dir keeps one PNG
# per device.
server-thread-hues-browser:
	python3 tests/server_thread_hues_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
