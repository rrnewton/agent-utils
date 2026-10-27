"""Differential: wrkslotsd parse-config against the Python document loader.

#562 rust-primary-wrkslots, first slice: the shared `.wrkslots.yml` document
loader (strict YAML subset or JSON) exists in Rust wrkslotsd and must agree
with wrkslots.yamlconfig on a corpus of valid and refused documents.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
from wrkslots import yamlconfig


AU_ROOT = Path(__file__).resolve().parents[3]
CARGO = "cargo"

VALID = [
    "schema: 2\nmachine: node-a\n",
    "---\nname: 'it''s'\nquoted: \"a\\tb\"\nempty:\nitems: [1, two, {x: null}]\n",
    "slots:\n  - name: a\n    size: 0x10\n  - name: b\nflags: [true, no, 0755, 1.5, .5]\n",
    "nested: {a: {b: [1, 2]}, c: 'x'}\nlist:\n- one\n- -5\n",
    "# comment only\nkey: value # trailing\n",
    '{"json": true, "n": 3}\n',
    "plain: 1:30\nother: yes\n",
]

INVALID = [
    "a: 1\na: 2\n",
    "\ta: 1\n",
    "a: |\n  text\n",
    "a: &anchor 1\n",
    "key: [1,\n 2]\n",
    "a: {x: 1, x: 2}\n",
    "a: 1\n---\n",
    "%YAML 1.2\n",
    '{"a": 1, "a": 2}\n',
    "a: 'unterminated\n",
]


def _rust_parse(tmp_path: Path, text: str) -> subprocess.CompletedProcess[str]:
    document = tmp_path / "config.yml"
    document.write_text(text)
    return subprocess.run(
        [CARGO, "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-config", "--config", str(document)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


@pytest.mark.parametrize("text", VALID)
def test_rust_loader_matches_python(tmp_path: Path, text: str) -> None:
    value, document_format = yamlconfig.load_document(text)
    result = _rust_parse(tmp_path, text)
    assert result.returncode == 0, result.stderr
    parsed = json.loads(result.stdout)
    assert parsed == {"format": document_format, "value": value}


@pytest.mark.parametrize("text", INVALID)
def test_rust_loader_refuses_like_python(tmp_path: Path, text: str) -> None:
    with pytest.raises(yamlconfig.YamlError):
        yamlconfig.load_document(text)
    result = _rust_parse(tmp_path, text)
    assert result.returncode == 1
    assert "line " in result.stderr or "duplicate" in result.stderr
