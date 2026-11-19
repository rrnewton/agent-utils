.PHONY: android-speech-self-test
# The physical-device run is opt-in, but its signal/language evaluator has offline negative
# controls. Keep those controls in the ordinary gate so a broken harness cannot silently bless a
# later Android recording.
android-speech-self-test:
	python3 scripts/android-device-speech.py --self-test
