Frozen authority bridge handoff

Updated: 2026-09-15T23:01:15.844804+00:00
Branch: see the registered checkout HEAD symbolic reference.
Head:7b8e342b06d6c273eebbc6c0804ba9ca515be259
Tree:226a15b78d49d518d628a0ccb5ce94c1299f6e4d
Base:14589e875c08e458d2f8af5a6f78b12128d5a367

The two committed paths are py/wrkslots/cli.py and py/wrkslots/tests/test_tool_authority.py. Source remains frozen. Native coordinator process2162531 owns this registered slot; no full validation is running here. Do not remove the slot or this handoff.

Native and independent reviews APPROVE this exact checkpoint. The independent dedicated session is cf85433d-a9d2-4ab6-aed5-ccee367abd2b; its first full report completed but the CLI exceeded600s and was terminated. A same-session exact-head attestation then exited normally0 in36.121s. The complete wrkslots subset passed696 cases in1111.564s:43host,555private-namespace lifecycle,98ordinary-environment lifecycle. Strict mypy passed on both changed files;9unchanged chain controls passed. These are focused controls, not ordinary full validation.

The actual published producer verifier was exercised through a nested Git fixture:1qualifying case,9refusals,1old-baseline red control. Permanent consumer-owned protocol/interpreter coverage is assigned in the canonical task tracker as test_published_tool_authority. A real installed full-tree digest took0.766692s under the production system interpreter;0physical read_bytes means that observation was warm, not a cold-storage guarantee. The15s bound remains unchanged.

All generated fixture and review artifacts were preserved outside this checkout before whole-root source gates. Evidence root relative to this slot:../../../ignored/ci-hub/ops-authority-au-20260915
- slot-target/ops-authority holds test logs, actual-verifier integration source/results and historical fixtures.
- slot-ignored-ops-authority holds frozen metadata and complete independent review certificates.
- artifact-relocation.json maps every original relative path and verifies bytes, modes, inode identities and modification times for2361entries,25687615regular-file bytes.
- installed-digest-902fa8cb.json records the exact full installed-tree measurement.
Moved fixture Git absolute paths are historical provenance, not runnable checkouts; retain the mapping.

Next: coordinator waits for the other AU lane to land, composes this unchanged patch onto that current main, obtains exact-head attestations, and authorizes one full ordinary make validate on the combined source. Full suites must be serialized. Do not rebase, publish, start validation, or modify source independently.
