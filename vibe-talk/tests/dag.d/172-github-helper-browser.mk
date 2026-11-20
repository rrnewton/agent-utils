.PHONY: github-helper-browser
# `#1997 vibe-talk-github-login-error-text` + `#1998
# vibe-talk-github-helper-health-fallback` in a real Chromium at 412x915
# (touch) and 1280x800 (mouse), light and dark: a missing GitHub helper
# named on the document row and in the reader, with /healthz standing.
github-helper-browser:
	python3 tests/github_helper_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
