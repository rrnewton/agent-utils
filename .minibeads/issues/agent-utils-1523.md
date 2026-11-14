---
title: 'voice-main-strict-ts: convert the entry module to strict TypeScript'
status: open
priority: 2
issue_type: task
created_at: 2026-11-14T16:03:30.000000000+00:00
updated_at: 2026-11-14T16:03:30.000000000+00:00
---

# Description

[muse watermelon] The final entry of `#624 voice-page-modularity` lands
`vibe-talk/web/src/app/main.js` checked exactly as `web/voice.js` was
(`checkJs`, `strict: false` in `vibe-talk/tsconfig.json`), because that step is a
move and must change behaviour by zero. Every other handwritten module below
`web/src` is strict TypeScript under `tsconfig.web.json`. This issue is the
follow-up conversion of the entry itself, to be done after `#624` closes,
split by view area, never as one giant commit.

Measured baseline on the final-entry candidate tree (reproduced
2026-11-14), 652 errors, all in the renamed entry:

```bash
mv vibe-talk/web/src/app/main.js vibe-talk/web/src/app/main.ts
vibe-talk/contract/node_modules/.bin/tsc -p vibe-talk/tsconfig.web.json --pretty false
// 652 errors, all in src/app/main.ts:
//   263 TS6133 (declared but never read: unused imports, locals, parameters)
//   208 TS2345, 117 TS2322 (argument/assignment mismatches, mostly dep-object
//     contravariance where the entry hands a narrower record than an owner's
//     port declares), 18 TS2339, 14 TS7006, the rest under 10 each
mv vibe-talk/web/src/app/main.ts vibe-talk/web/src/app/main.js
```

So roughly 390 errors remain after the unused declarations are pruned. No
`@ts-nocheck`, `@ts-ignore` or `@ts-expect-error` may be used to pay this
down (the architecture checker refuses them anyway).

# Design

- Convert area by area (shell, channel, composer/voice, settings, documents,
  app wiring), each step behaviour-neutral: types and annotations only, no
  interface redesign in the same commit. Where a dep object's type is the
  obstacle, widen or narrow the owner's declared port to what the entry
  already passes, and say so in the commit.
- Prune the 263 unused declarations first as their own step; each removal is
  an unused import or local, never a behaviour change.
- The end state: `web/src/app/main.ts` checked by `tsconfig.web.json`
  (strict), `vibe-talk/tsconfig.json`'s non-strict `checkJs` coverage of the
  entry retired, the generated `entry-imports.ts` regenerated from the `.ts`
  entry, and the page_build "every handwritten module but the entry is
  TypeScript" exception removed.
- The architecture checker's exemption of `app/main.js` (platform globals and
  its unseen JSDoc `any` casts) goes away with the rename; the platform
  globals the entry names then reach it through its ports as everywhere else,
  or the file exemption is re-justified by name for `main.ts`.

# Acceptance Criteria

- `web/src/app/main.ts` passes `tsc -p tsconfig.web.json` clean (strict,
  `noUnusedLocals`, `noUnusedParameters`), with no `@ts-*` suppression
  comments added.
- `make -C vibe-talk typecheck` passes with the entry under the strict
  config, and the page suite, module suites and page_build suite pass at each
  step.
- The emitted page asset differs from the last pre-conversion build only in
  wrapper/module shape (the same asset-diff evidence as the final entry).
- `entry-imports.ts` is regenerated from the converted entry and held fresh
  by the existing page_build check.
