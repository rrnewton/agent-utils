.PHONY: system-bots-browser
# `#323 slack-system-bots` in a real Chromium: a Slack welcome bot drawn as another bot, named and
# labelled, set in and coloured as a colleague is, and a named coding agent drawn as the agent beside
# it, at a phone and a desk, dark and light. Pass SCREENSHOTS=dir to keep PNGs.
system-bots-browser:
	python3 tests/system_bots_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
