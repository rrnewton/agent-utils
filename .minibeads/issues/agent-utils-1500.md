---
title: 'codex-idle-animation-reads-as-draft: Codex idle braille animation is read as unsubmitted composer text'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T00:00:00.000000000+00:00
updated_at: 2026-11-13T00:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] `agentctl send` and drain refuse to deliver to an idle Codex pane. The Codex composer draws an idle particle animation in braille cells (U+2800-U+28FF) over its placeholder line ("Ask Codex to do anything"). agentctl's composer-empty check reads those cells as unsubmitted text, so drain reports blocked "composer already holds unsubmitted text ...; refusing to append". Every first brief to a fresh Codex lane stalled as pending (send rc 124). The reporter's workaround was to cancel the queued items and type a one-line pointer by hand, because long pastes get garbled.

Related, closed: #808, #884, #989 (stranded/own-draft composer handling). Their designs must not regress: a draft agentctl itself typed is still recovered, a human draft is still refused, and a paste placeholder left in the composer still holds later prompts.

# Design

In both editions (Python `submission` and the Rust delivery path), the Codex composer reader treats a composer whose content is only the placeholder plus braille-pattern cells (U+2800-U+28FF) and whitespace as empty. Keep refusing real text, including text that mixes braille with letters. Scope it to the Codex harness's composer parsing where the check is per harness; otherwise use a shared decorative-glyph filter with a unit-tested character set. Add fixtures for an idle Codex screen with animation frames (several frames, including a braille-only line and a placeholder overlaid with braille), a real draft, and a mixed line. Rust and Python must agree; add a differential/fixture case. No wall-clock assertions. Tests use tmp registries only.

# Acceptance Criteria

- An idle Codex screen whose composer holds only the placeholder, braille-pattern cells (U+2800-U+28FF) and whitespace is read as empty in both editions: `codex_idle_composer` / idle recognition succeeds, the draft check does not refuse, and a first brief is typed and submitted.
- Several animation frames pass, including a braille-only composer line and a placeholder overlaid with braille.
- A real draft is still refused before typing in both editions, including a line that mixes braille cells with letters.
- A faint paste placeholder left in a Codex composer still counts as a draft and holds later prompts (#808/#884 behaviour unchanged); stranded/own-draft recovery (#808, #884, #989) is unchanged.
- Rust and Python agree on the same shared fixture cases.

# Notes

Reported from a consumer team as verbatim facts summarised in the description. Number assigned by the coordinator for this lane. No consumer names, host names or local paths are recorded here.

## Resolution

[watermelon-full-preview] Fixed in both editions. The Codex composer reader now treats a composer holding only a Codex placeholder, braille-pattern cells (U+2800-U+28FF) and whitespace as empty: `py/agentctl/submission.py` (`_codex_only_placeholder_and_braille`, scoped in `_codex_view`, `submit_verified` and `composer_is_empty`) and Rust `harness/codex.rs` (`only_placeholder_and_braille`, scoped in its composer `view` and `idle_composer`), `submission.rs` (`composer_is_empty`, `composer_draft` and the `submit_verified` draft check) and `turn_state.rs` (`composer_draft`). Braille cells overlaid on a solid-drawn placeholder leave a long in-order fragment of the placeholder, which is also read as empty. Real text still refuses, including a line mixing braille with letters and a non-braille glyph, and a faint paste placeholder still counts as a draft, so #808/#884/#989 stranded and own-draft handling is unchanged.

Fixtures: shared `rs/agentctl/tests/fixtures/composer/codex-braille-cases.json` (braille-only frames, braille-range boundaries U+2800/U+28FF, braille over a faint placeholder, a placeholder overlaid and fragmented by braille, a braille-only line, a real draft, a mixed line, a non-braille glyph and a faint paste placeholder) is read by both editions. Python: `test_codex_braille_idle_fixtures_agree_on_empty_and_idle`, `test_codex_idle_animation_frames_do_not_block_a_first_brief`, `test_codex_mixed_braille_and_letters_still_blocks_a_first_brief`. Rust: `codex_braille_idle_fixtures_agree_on_empty_and_idle`, `codex_braille_character_set_is_exactly_u2800_to_u28ff`, `codex_idle_animation_frames_do_not_block_a_first_brief`, `codex_mixed_braille_and_letters_still_blocks_a_first_brief`, `turn_state` `a_codex_braille_idle_animation_is_not_a_composer_draft`.
