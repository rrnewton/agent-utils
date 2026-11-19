.PHONY: message-summaries-browser
message-summaries-browser:
	python3 tests/message_summaries_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
