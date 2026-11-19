.PHONY: voice-call-drops-browser
# `#268 voice-call-drops` in a real Chromium: a spoken call on the fake microphone replaying the
# voice bridge's event order -- no microphone frame inside the greeting's hand-off, a "Yes." whose
# final transcript arrives after the agent's reply committing the card once and never laid out as a
# turn of its own (`#269 approval-card-inline`: it is drawn into the frozen card), the page's notice
# waiting for that turn, the server's close said and recorded, and a hang-up sending one goodbye;
# then (`#330 voice-long-turn-drop`, `#337 voice-relay-silent`) a long answer shown as it arrives and
# heard out before a message from another watched channel goes, a typed cut that keeps the cut
# answer's words, and a turn heard without text saying so.
# Pass WEB_ROOT=dir to run it against another copy of the page.
voice-call-drops-browser:
	python3 tests/voice_call_drops_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
