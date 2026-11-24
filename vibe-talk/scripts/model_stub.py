#!/usr/bin/env python3
"""Loopback model stub for the screenshot harness: the external model, stubbed.

WHY THIS EXISTS
===============

The dynamic-testing rule (owner thread 185) allows stubbing only external
upstreams. For Discuss that upstream is the model behind
`[summaries.llm]`: the harness points the throwaway server's Messages API
backend at this stub on 127.0.0.1, so `GET .../catchup/preview` and
`POST .../catchup` run through the REAL server router and the page's exact
requests, and only the model's answer is invented here. A shot taken this
way shows Discuss really answering; a fixture that intercepts the catchup
route at the network edge shows a 404 the moment the page asks for anything
the fixture did not invent, which is the failure this stub replaces.

The answer is fixed prose with a marker line, so a scene can assert that
the summary on screen is the one this stub served (through the server),
not text the page made up.

USAGE
=====

    model_stub.py --port PORT        (binds 127.0.0.1 only; 0 asks the kernel)
    model_stub.py --port 0 --port-file PATH   (also writes the bound port to PATH)
"""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ANSWER = (
    "## Progress\n"
    "The model stub read the channel window the server sent. "
    "Ada shipped the fix and Ben is reviewing it.\n\n"
    "## Review\n"
    "Nothing is blocked. The next step is the review pass.\n\n"
    "Model stub summary: this answer came from the loopback model stub "
    "through the real Discuss route.\n\n"
    # `#1924` (c): the trailing Terms section is the server's own split
    # (`split_unknown_terms`): it leaves the answer body and stands in
    # the pane's Terms block, so a shot can show Terms rendering.
    "## Terms I didn't understand\n"
    "- cache-key rewrite\n"
    "- flaky ordering test"
)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:  # keep the run's log clean
        pass

    def _json(self, status: int, body: dict[str, object]) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802 - the handler's own name
        self._json(200, {"ok": True, "stub": "model"})

    def do_POST(self) -> None:  # noqa: N802 - the handler's own name
        if self.path.rstrip("/") not in ("/v1/messages", ""):
            self._json(404, {"error": "not_found", "detail": "the model stub serves /v1/messages"})
            return
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        answer = ANSWER
        # `#2156`: a WTF glossary lookup asks in its own shape; answer it as
        # the strict JSON the lookup route parses, citing the first evidence
        # message the server fenced into the prompt. The prompt arrives
        # JSON-wrapped, so decode before reading its lines.
        try:
            payload = json.loads(body.decode("utf-8", "replace"))
            text = json.dumps(payload)
            for message in payload.get("messages", []):
                content = message.get("content", "")
                if isinstance(content, str) and content:
                    text = content
                elif isinstance(content, list):
                    for block in content:
                        if isinstance(block, dict) and block.get("type") == "text":
                            text = str(block.get("text", ""))
        except ValueError:
            text = body.decode("utf-8", "replace")
        if "You maintain a project glossary" in text or "Selected words:" in text:
            import re

            selected = re.search(r"Selected words: (.+)", text)
            evidence = re.search(r"\[msg id=(\S+?) ", text)
            term = (selected.group(1).strip() if selected else "the term").removeprefix("the ")
            # `#2189`: evidence that is only the selection's own use-site
            # defines nothing; refuse as a real glossary model must.
            bodies = re.findall(r"\[msg id=\S+? [^\]]*\] (.*)", text)
            if len(set(bodies)) <= 1:
                self._json(
                    200,
                    {
                        "id": "msg_model_stub",
                        "type": "message",
                        "role": "assistant",
                        "model": "model-stub",
                        "content": [{"type": "text", "text": json.dumps({"term": None})}],
                        "stop_reason": "end_turn",
                        "usage": {"input_tokens": 100, "output_tokens": 40},
                    },
                )
                return
            answer = json.dumps(
                {
                    "term": term,
                    "definition": f"{term}: the stub's glossary definition, through the real lookup route.",
                    "source_kind": "message",
                    "source_id": evidence.group(1) if evidence else "",
                }
            )
        self._json(
            200,
            {
                "id": "msg_model_stub",
                "type": "message",
                "role": "assistant",
                "model": "model-stub",
                "content": [{"type": "text", "text": answer}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 100, "output_tokens": 40},
            },
        )


def main() -> int:
    parser = argparse.ArgumentParser(description="Loopback Messages API stub for screenshot runs.")
    parser.add_argument("--port", type=int, required=True, help="loopback port to bind (0 asks the kernel)")
    parser.add_argument("--port-file", default="", help="write the bound port to this file once listening")
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    bound = server.server_address[1]
    if args.port_file:
        with open(args.port_file, "w", encoding="utf-8") as handle:
            handle.write(f"{bound}\n")
    print(f"model stub on 127.0.0.1:{bound}", flush=True)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
