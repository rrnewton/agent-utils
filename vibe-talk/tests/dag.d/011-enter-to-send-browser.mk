.PHONY: enter-to-send-browser
# `#225 enter-to-send` in a real Chromium at a desk's size: real keys in the reply box. By default
# Shift+Enter is a new line and Enter posts the reply; with "Enter to send" off in Settings (its hint
# naming the key that sends, kept across a real reload), Enter is a new line and Ctrl+Enter posts.
# Pass WEB_ROOT=dir to run it against another copy of the page.
enter-to-send-browser:
	python3 tests/enter_to_send_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
