.PHONY: send-order-browser
# `#277 send-order` in a real Chromium against the real server binary, which posts through its real
# provider client to a mock Google Chat bridge on loopback that fails A at once: B waits behind A,
# saying so, and the bridge never sees it; A goes again under the same nonce and lands, then B;
# across a real reload a message still waits behind the one before it, sending nothing, until Retry
# lands both in order; Dismiss on a failing message lets the next go; a slow bridge is given one post
# at a time, in order; and a post whose caller hangs up mid-post still lands before the next one.
# Builds the release binary first, because web/ is compiled into it. Pass SCREENSHOTS=dir to keep a
# PNG of each waiting state.
send-order-browser:
	cargo build --release --locked
	python3 tests/send_order_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
