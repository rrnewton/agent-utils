# Grammars for the code-block highlighter

`#531 server-side-syntax-highlight`. `build.rs` compiles one syntax set for
`src/render/highlight.rs` from two sources:

- **two-face** 0.5 ([CosmicHorrorDev/two-face](https://github.com/CosmicHorrorDev/two-face), MIT or
  Apache-2.0), which packages the grammars [bat](https://github.com/sharkdp/bat) ships, most of
  them from Sublime Text's [Packages](https://github.com/sublimehq/Packages). Kept: bash, C, C++,
  Diff, Go, Haskell, JSON, JavaScript, Lean 4, Markdown, OCaml, Python, Rust, SQL, TOML, TypeScript,
  TypeScriptReact and YAML, with the regular-expression and bash-builtin grammars they include.
  Each grammar's own licence is listed by two-face's `acknowledgement` module and in bat's
  `assets/syntaxes`.
- **This directory**, for the languages neither has. Each file is a Sublime Text grammar
  (`.sublime-syntax`) converted by hand, rule for rule, from a TextMate grammar its language's own
  community maintains, and each is MIT-licensed; the licence texts are beside them.

| File | Converted from | Licence |
|---|---|---|
| `Agda.sublime-syntax` | [agda/agda-github-syntax-highlighting](https://github.com/agda/agda-github-syntax-highlighting) `Agda.yml` at `ba5005841ff9bdc5186af68befd58ed456fc7a8c` (2025-10-07), the grammar GitHub highlights Agda with | MIT, `LICENSE.agda-github-syntax-highlighting` |
| `Rocq.sublime-syntax` | [rocq-prover/vsrocq](https://github.com/rocq-prover/vsrocq) `client/syntax/rocq.tmLanguage.json` at `bc21058b0bb3cab9211f41a623cd1fa30dc42420` (2026-10-26), the Rocq prover's own editor grammar | MIT, `LICENSE.vsrocq` |

For Agda, the grammar of the Agda mode for VS Code
([banacorn/agda-mode-vscode](https://github.com/banacorn/agda-mode-vscode)) was looked at first and
not used: it covers only the `FOREIGN` and `COMPILE` pragmas, because that editor colours
everything else from Agda's own type checker.

## What changed in conversion

The rules, their order and their scope names are upstream's. A TextMate `begin`/`end` rule is a
Sublime `push` of a context whose `meta_scope` is the rule's `name` and whose `pop` is its `end`;
a rule with `applyEndPatternLast` lists its `pop` last. Beyond that:

- **Both:** `[[:space:]]` is written `\s`. A lookbehind with alternatives of different lengths,
  `(?<=^|[…])`, is written `(?:^|(?<=[…]))`, the same match in a form every regex engine syntect
  can use accepts. A lookahead that needed a following space also accepts the end of the line.
- **Agda:** pragmas are matched before block comments rather than excluded from them with
  `{-[^#]`; the same text is matched either way. `constant.char` is the standard
  `constant.character`.
- **Rocq:** the identifier pattern repeated in six rules is one variable, `ident`, and its
  no-break space is written `\x{00A0}`. Comments come before the operator rule, which changes
  nothing (no operator starts with `(`) but says why `(*` is never read as `(` and `*`. A comment's
  nested strings and comments are named contexts rather than repository includes. One scope moved:
  upstream's Ltac2 rule names the optional `rec` as the function's name (capture 3); here the name
  is the identifier after it (capture 4), and `rec` is a keyword.

`src/render/highlight.rs` has a sample of each language in its tests, which fail if a grammar
stops producing the kinds of token it should.
