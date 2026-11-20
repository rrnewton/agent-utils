.PHONY: cmd-d-browser
# `#1978 vibe-talk-cmd-d-dock-toggle` against the real server binary: plain
# Cmd+D (Mac-emulated) / Ctrl+D docks and floats the quote reply and Discuss.
cmd-d-browser:
	python3 tests/cmd_d_browser.py
