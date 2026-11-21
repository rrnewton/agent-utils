.PHONY: local-tts-sidecar-self-test
# The pace-recording controls for the local TTS sidecar (`#2053
# vibe-talk-pace-has-no-effect`): a fake piper must be handed the inverted
# pace as a length scale, a fake kokoro the pace itself, both clamped.
local-tts-sidecar-self-test:
	python3 scripts/local_tts_sidecar.py --self-test
