.PHONY: voice-expand-browser
voice-expand-browser:
	python3 tests/voice_expand_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
