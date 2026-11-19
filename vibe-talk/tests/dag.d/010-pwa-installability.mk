.PHONY: pwa-installability
# A real current Chromium, speaking CDP. This validates the installability decision the DOM-only
# page suite cannot make, at / and from the app's old address /voice (`#302 root-is-the-app`): a
# 302 keeping query and fragment, the same app id, inside the scope. It runs the release binary on a
# loopback secure context by default, so it builds that first; set VIBE_TALK_CHROMIUM to inspect
# with a particular Chrome executable, or pass --url directly to the script for a deployed server.
# It deliberately states that this is not physical-device launcher proof.
pwa-installability:
	cargo build --release --locked
	python3 tests/pwa_installability.py
