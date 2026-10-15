#!/usr/bin/env python3
"""Write the table of Slack's standard emoji names that Slack reactions are shown with.

`#312 slack-reactions`. Slack names a reaction by its emoji's short name -- `+1`, `eyes`,
`white_check_mark`, `+1::skin-tone-2` -- never by the emoji itself, so the server needs the
character each standard name stands for. Those names come from iamcal's emoji-data, the
MIT-licensed emoji table Slack's own picker uses, which is published on npm as
`emoji-datasource`. This reads that package's `emoji.json` and writes
`vibe-talk/src/slack/emoji_names.tsv`, which `src/slack/reactions.rs` compiles in. A name not
in the table is a workspace's custom emoji, and is shown as its name.

One line per name, sorted: the name, a tab, the emoji's code points in hex joined by `-`, and,
for an emoji a skin tone can be applied to, a tab and `s`. The server applies a tone by putting
its modifier (U+1F3FB to U+1F3FF) after the first code point, dropping a U+FE0F that followed it.
An emoji is marked `s` only when that rule gives exactly every one-tone variant the data lists for
it, so the rule is checked here, against the data, rather than assumed.

USAGE

    npm pack emoji-datasource@16.0.0          # or download the tarball by any other means
    tar -xzf emoji-datasource-16.0.0.tgz package/emoji.json package/package.json package/LICENSE
    python3 vibe-talk/scripts/slack_emoji_names.py package \\
        --output vibe-talk/src/slack/emoji_names.tsv

`--check` compares with an existing output instead of writing it, and exits 1 when they differ.
The tarball is large (it carries image sheets); only the three files above are read.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

# Slack's `skin-tone-2` to `skin-tone-6`, as the data names their modifiers.
TONES = ("1F3FB", "1F3FC", "1F3FD", "1F3FE", "1F3FF")
VARIATION_SELECTOR = "FE0F"


def toned(unified: str, tone: str) -> str:
    """The rule the server applies: the tone after the first code point, without a U+FE0F there."""
    first, *rest = unified.split("-")
    if rest and rest[0] == VARIATION_SELECTOR:
        rest = rest[1:]
    return "-".join([first, tone, *rest])


def text_field(entry: dict[str, object], key: str) -> str:
    value = entry.get(key)
    if not isinstance(value, str) or not value:
        raise SystemExit(f"an emoji entry has no string {key!r}: {entry.get('short_name')!r}")
    return value


def takes_tones(entry: dict[str, object], unified: str) -> bool:
    """Whether the data lists one-tone variants for this emoji, each exactly as the rule writes it."""
    variations = entry.get("skin_variations")
    if not isinstance(variations, dict):
        return False
    single: dict[str, str] = {}
    for key, variant in variations.items():
        if not isinstance(key, str) or "-" in key:
            continue  # a two-person emoji's pair of tones; the server applies one tone only
        if not isinstance(variant, dict):
            raise SystemExit(f"a skin variation of {unified} is not an object")
        single[key] = text_field(variant, "unified")
    if not single:
        return False
    return set(single) == set(TONES) and all(
        single[tone] == toned(unified, tone) for tone in TONES
    )


def table(package: Path) -> str:
    manifest: object = json.loads((package / "package.json").read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise SystemExit("package.json is not an object")
    name = manifest.get("name")
    version = manifest.get("version")
    if name != "emoji-datasource" or not isinstance(version, str):
        raise SystemExit(f"{package} is not the emoji-datasource package: {name!r}")
    license_text = (package / "LICENSE").read_text(encoding="utf-8").strip()
    data: object = json.loads((package / "emoji.json").read_text(encoding="utf-8"))
    if not isinstance(data, list):
        raise SystemExit("emoji.json is not a list")
    rows: dict[str, str] = {}
    for item in data:
        if not isinstance(item, dict):
            raise SystemExit("an emoji entry is not an object")
        entry: dict[str, object] = {str(key): value for key, value in item.items()}
        unified = text_field(entry, "unified").upper()
        if not all(part and all(c in "0123456789ABCDEF" for c in part) for part in unified.split("-")):
            raise SystemExit(f"an emoji's code points are not hex: {unified!r}")
        names = entry.get("short_names")
        if not isinstance(names, list) or not names:
            raise SystemExit(f"{unified} has no short names")
        flag = "\ts" if takes_tones(entry, unified) else ""
        for short in names:
            if not isinstance(short, str) or not short or any(c in short for c in "\t\n:"):
                raise SystemExit(f"{unified} has an unusable short name: {short!r}")
            if short in rows:
                raise SystemExit(f"the short name {short!r} names two emoji")
            rows[short] = f"{short}\t{unified}{flag}"
    header = [
        f"# Slack's standard emoji names, from {name} {version} (iamcal/emoji-data).",
        "# Generated by vibe-talk/scripts/slack_emoji_names.py; do not edit by hand.",
        "# name<TAB>code points[<TAB>s, when a skin tone applies]",
        "#",
        *[f"# {line}".rstrip() for line in license_text.splitlines()],
    ]
    return "\n".join([*header, *(rows[key] for key in sorted(rows))]) + "\n"


def main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Write the Slack emoji-name table from an extracted emoji-datasource package.",
        epilog="Example: slack_emoji_names.py package --output vibe-talk/src/slack/emoji_names.tsv",
    )
    parser.add_argument(
        "package",
        type=Path,
        help="the extracted npm package directory, holding emoji.json, package.json and LICENSE",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="the table to write (or, with --check, to compare); standard output when omitted",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare with --output instead of writing it; exit 1 when they differ",
    )
    args = parser.parse_args(argv)
    package: Path = args.package
    output: Path | None = args.output
    check: bool = args.check
    text = table(package)
    if check:
        if output is None:
            parser.error("--check needs --output")
        if output.read_text(encoding="utf-8") != text:
            print(f"{output} differs from what {package} generates", file=sys.stderr)
            return 1
        return 0
    if output is None:
        sys.stdout.write(text)
    else:
        output.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
