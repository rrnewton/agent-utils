#!/usr/bin/env python3
"""A local text-to-speech sidecar for vibe-talk read-aloud. `#561 local-tts-provider`.

It runs an open-source TTS model ON THIS MACHINE'S CPU and serves the two
calls vibe-talk's `local` read-aloud backend makes, on loopback only:

  GET  /voices     -> {"voices": [{"name": ..., "locale": "en"}, ...]}
  POST /synthesize   {"q": text, "voice": name, "speed": 1.0} -> audio/wav
  GET  /health     -> {"ok": true, "engine": "kokoro"}

Engines:

  kokoro (default)  Kokoro-82M via kokoro-onnx. Model card licence:
                    Apache-2.0 for the weights AND the voices in
                    voices-v1.0.bin (hexgrad/Kokoro-82M). Files needed in
                    --model-dir: kokoro-v1.0.onnx, voices-v1.0.bin.
  piper             Piper via piper-tts. Code licence: MIT. Each voice is
                    a separate download whose licence follows its dataset
                    (many are MIT/CC0, some are not) — read the voice's
                    .onnx.json before using it. Files needed: the voice
                    .onnx (and its .onnx.json) in --model-dir.

Synthesis makes NO network call: the model files are fetched once, by
hand, into --model-dir (outside any repository), and this process only
ever reads them. Measured on the development server (4 threads, busy
host): Kokoro loads in about 1 s, speaks a sentence in 0.5-1.1 s, and
runs at 4-6.5x real time.

Run (from the checkout, with the project's environment):

  vibe-talk/.venv/bin/python vibe-talk/scripts/local_tts_sidecar.py \
      --model-dir /path/to/kokoro-models --port 18771

vibe-talk then wants, in its configuration:

  [read_aloud]
  backend = "local"
  [local_tts]
  endpoint = "http://127.0.0.1:18771"
  voice = "af_heart"
"""

from __future__ import annotations

import argparse
import io
import json
import struct
import sys
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

Json = dict[str, object]


def wav_bytes(samples: list[float], sample_rate: int) -> bytes:
    """One WAV (16-bit mono PCM) carrying float samples in [-1, 1]."""
    pcm = struct.pack(
        f"<{len(samples)}h",
        *[max(-32768, min(32767, int(sample * 32767))) for sample in samples],
    )
    out = io.BytesIO()
    with wave.open(out, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(pcm)
    return out.getvalue()


def clamp_speed(speed: float) -> float:
    """The pace bounds the server clamps to (0.5 to 2.0), applied again here.

    The server clamps before it sends (`clamp_speed` in the Rust local_tts
    adapter), but this sidecar's contract is the JSON it is handed: a caller
    that sends 4.0 gets a doubled voice, not a quadrupled one.
    """
    return min(2.0, max(0.5, speed))


def piper_length_scale(speed: float) -> float:
    """Piper's pace knob for a pace multiplier: it stretches, so it inverts.

    A length scale of 2.0 says every sound twice as long — half the pace.
    """
    return 1.0 / clamp_speed(speed)


class Engine:
    """What the handlers speak through; one model, serialized calls."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.lock = threading.Lock()
        self.kind = args.engine
        self.lang = args.lang
        self.sample_rate = 24_000
        self._kokoro: object = None
        self._piper: object = None
        self._voices: list[str] = []
        model_dir = Path(args.model_dir)
        if args.engine == "kokoro":
            from kokoro_onnx import Kokoro  # type: ignore[import-not-found]

            self._kokoro = Kokoro(
                str(model_dir / "kokoro-v1.0.onnx"),
                str(model_dir / "voices-v1.0.bin"),
            )
            self._voices = list(self._kokoro.get_voices())  # type: ignore[attr-defined]
        else:
            from piper import PiperVoice  # type: ignore[import-not-found]

            models = sorted(model_dir.glob("*.onnx"))
            if not models:
                raise SystemExit(f"no piper voice .onnx in {model_dir}")
            self._piper = PiperVoice.load(str(models[0]))
            self.sample_rate = int(self._piper.config.sample_rate)  # type: ignore[attr-defined]
            self._voices = [models[0].stem]

    def voices(self) -> list[Json]:
        return [{"name": name, "locale": "en"} for name in self._voices]

    def synthesize(self, text: str, voice: str, speed: float) -> bytes:
        with self.lock:
            if self.kind == "kokoro":
                samples, rate = self._kokoro.create(  # type: ignore[attr-defined]
                    text, voice=voice, speed=clamp_speed(speed), lang=self.lang)
                return wav_bytes([float(sample) for sample in samples], int(rate))
            # Piper has no speed: its synthesis config stretches every sound
            # by a length scale, so the pace arrives inverted (`#2053
            # vibe-talk-pace-has-no-effect`: this call used to pass nothing,
            # and a piper deployment heard no pace change ever).
            from piper.config import SynthesisConfig  # type: ignore[import-not-found]

            config = SynthesisConfig(length_scale=piper_length_scale(speed))
            chunks: list[float] = []
            for chunk in self._piper.synthesize(text, config):  # type: ignore[attr-defined]
                floats = getattr(chunk, "audio_float_array", None)
                if floats is not None:
                    chunks.extend(float(sample) for sample in floats)
                else:
                    raw = chunk.audio_int16_bytes
                    chunks.extend(
                        sample / 32768.0
                        for sample in struct.unpack(f"<{len(raw) // 2}h", raw))
            return wav_bytes(chunks, self.sample_rate)


def handler_for(engine: Engine) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            if self.path == "/voices":
                self.send_json(200, {"voices": engine.voices()})
            elif self.path == "/health":
                self.send_json(200, {"ok": True, "engine": engine.kind})
            else:
                self.send_json(404, {"error": "not found"})

        def do_POST(self) -> None:  # noqa: N802
            if self.path != "/synthesize":
                self.send_json(404, {"error": "not found"})
                return
            try:
                length = int(self.headers.get("Content-Length") or 0)
                body: Json = json.loads(self.rfile.read(length) or b"{}")
                text = str(body.get("q") or "")
                voice = str(body.get("voice") or "")
                speed = float(body.get("speed") or 1.0)  # type: ignore[arg-type]
                if not text or not voice:
                    self.send_json(400, {"error": "q and voice are required"})
                    return
                audio = engine.synthesize(text, voice, speed)
            except Exception as error:  # the model refused; say so, stay up
                self.send_json(500, {"error": f"synthesis failed: {error}"})
                return
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(audio)))
            self.end_headers()
            try:
                self.wfile.write(audio)
            except OSError:
                pass

        def send_json(self, status: int, payload: Json) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


class _FakeChunk:
    """One piper audio chunk, as the fake voice below yields it."""

    def __init__(self, samples: list[float]) -> None:
        self.audio_float_array = samples
        self.audio_int16_bytes = b""


class _FakeSynthesisConfig:
    """`piper.config.SynthesisConfig`, recording what it was built with."""

    def __init__(self, length_scale: float = 1.0) -> None:
        self.length_scale = length_scale


class _FakePiperVoice:
    """A piper voice that records the length scale of every synthesis."""

    instances: list["_FakePiperVoice"] = []

    def __init__(self) -> None:
        self.config = argparse.Namespace(sample_rate=22050)
        self.scales: list[float] = []
        _FakePiperVoice.instances.append(self)

    @classmethod
    def load(cls, _path: str) -> "_FakePiperVoice":
        return cls()

    def synthesize(self, text: str, config: _FakeSynthesisConfig) -> list[_FakeChunk]:
        self.scales.append(config.length_scale)
        return [_FakeChunk([0.1, -0.1])]


class _FakeKokoro:
    """A kokoro engine that records the speed of every synthesis."""

    instances: list["_FakeKokoro"] = []

    def __init__(self, *_paths: str) -> None:
        self.speeds: list[float] = []
        _FakeKokoro.instances.append(self)

    def get_voices(self) -> list[str]:
        return ["af_heart"]

    def create(
        self, _text: str, voice: str, speed: float, lang: str
    ) -> tuple[list[float], int]:
        del voice, lang
        self.speeds.append(speed)
        return ([0.1, -0.1], 24000)


def self_test() -> int:
    """Prove a pace reaches each engine, with recording fakes and no models.

    Run as `--self-test`: no model files, no network beyond loopback, and no
    piper or kokoro installation — both are stubbed in `sys.modules` with
    voices that record the pace parameter they were handed.
    """
    import tempfile
    import types
    import urllib.request

    failures: list[str] = []

    def check(name: str, ok: bool) -> None:
        print(f"{'ok' if ok else 'FAIL'}: {name}")
        if not ok:
            failures.append(name)

    check("clamp_speed keeps the server's bounds", clamp_speed(4.0) == 2.0
          and clamp_speed(0.1) == 0.5 and clamp_speed(1.5) == 1.5)
    check("piper_length_scale inverts the pace", piper_length_scale(2.0) == 0.5
          and piper_length_scale(0.5) == 2.0 and piper_length_scale(1.0) == 1.0)

    piper_module = types.ModuleType("piper")
    piper_module.PiperVoice = _FakePiperVoice  # type: ignore[attr-defined]
    piper_config = types.ModuleType("piper.config")
    piper_config.SynthesisConfig = _FakeSynthesisConfig  # type: ignore[attr-defined]
    kokoro_module = types.ModuleType("kokoro_onnx")
    kokoro_module.Kokoro = _FakeKokoro  # type: ignore[attr-defined]
    sys.modules["piper"] = piper_module
    sys.modules["piper.config"] = piper_config
    sys.modules["kokoro_onnx"] = kokoro_module

    with tempfile.TemporaryDirectory() as model_dir:
        (Path(model_dir) / "fake-voice.onnx").write_bytes(b"fake")
        piper_engine = Engine(argparse.Namespace(
            engine="piper", lang="en-us", model_dir=model_dir))
        piper_engine.synthesize("hello there", "fake-voice", 1.5)
        voice = _FakePiperVoice.instances[-1]
        check("piper is handed the inverted pace", voice.scales == [1.0 / 1.5])
        piper_engine.synthesize("hello there", "fake-voice", 4.0)
        check("piper pace is clamped before it inverts", voice.scales[-1] == 0.5)

        # The whole HTTP contract: a {"speed": 1.5} body becomes that scale.
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(piper_engine))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            body = json.dumps(
                {"q": "hello there", "voice": "fake-voice", "speed": 1.5}).encode()
            request = urllib.request.Request(
                f"http://127.0.0.1:{server.server_port}/synthesize",
                data=body, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=10) as response:
                audio = response.read()
            check("the sidecar answers audio for a paced request", len(audio) > 44)
            check("the posted speed reaches piper as a length scale",
                  voice.scales[-1] == 1.0 / 1.5)
        finally:
            server.shutdown()
            server.server_close()

        kokoro_engine = Engine(argparse.Namespace(
            engine="kokoro", lang="en-us", model_dir=model_dir))
        kokoro_engine.synthesize("hello there", "af_heart", 1.5)
        kokoro = _FakeKokoro.instances[-1]
        check("kokoro is handed the pace itself", kokoro.speeds == [1.5])
        kokoro_engine.synthesize("hello there", "af_heart", 4.0)
        check("kokoro pace is clamped", kokoro.speeds[-1] == 2.0)

    if failures:
        print(f"self-test: {len(failures)} failure(s)", file=sys.stderr)
        return 1
    print("self-test: all checks passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", choices=["kokoro", "piper"], default="kokoro")
    parser.add_argument("--model-dir",
                        help="directory holding the model files (outside any repository)")
    parser.add_argument("--self-test", action="store_true",
                        help="prove each engine receives the pace, with recording "
                             "fakes; needs no models and no TTS installation")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18771)
    parser.add_argument("--lang", default="en-us", help="kokoro language (default en-us)")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.model_dir:
        parser.error("--model-dir is required unless --self-test is given")
    engine = Engine(args)
    server = ThreadingHTTPServer((args.host, args.port), handler_for(engine))
    print(f"local tts sidecar ({engine.kind}) on http://{args.host}:{args.port} "
          f"with {len(engine.voices())} voices", file=sys.stderr)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
