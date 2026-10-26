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
                    text, voice=voice, speed=speed, lang=self.lang)
                return wav_bytes([float(sample) for sample in samples], int(rate))
            chunks: list[float] = []
            for chunk in self._piper.synthesize(text):  # type: ignore[attr-defined]
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", choices=["kokoro", "piper"], default="kokoro")
    parser.add_argument("--model-dir", required=True,
                        help="directory holding the model files (outside any repository)")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18771)
    parser.add_argument("--lang", default="en-us", help="kokoro language (default en-us)")
    args = parser.parse_args()
    engine = Engine(args)
    server = ThreadingHTTPServer((args.host, args.port), handler_for(engine))
    print(f"local tts sidecar ({engine.kind}) on http://{args.host}:{args.port} "
          f"with {len(engine.voices())} voices", file=sys.stderr)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
