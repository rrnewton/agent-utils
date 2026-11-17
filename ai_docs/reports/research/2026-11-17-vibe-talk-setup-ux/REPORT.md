# vibe-talk install and first-pairing UX review (2026-11-17)

For #1780 vibe-talk-setup-routes-and-pairing-ux. Method: walked `vibe-talk/QUICKSTART.md`,
`vibe-talk/README.md` ("From zero", "Pairing a phone", "Installing the reader"), the
`agent-utils-setup` skill's vibe-talk entry, `vibe-talk/scripts/run.sh` and
`verify-deployment.sh`, `src/main.rs`, `src/pair_link.rs`, `src/config.rs`, and the page's
pairing words in `web/src/present/settings/pairing.ts`, in the order a new user meets them.

## Answer to the owner's setup-skill question

The `agent-utils-setup` skill (`skills/agent-utils-setup/SKILL.md`) is wizard-style: phases 0-3
only read and ask in batches of at most about five questions, each with a recommended default,
and nothing changes on disk until the owner approves the plan in phase 4. For vibe-talk it
covers only a menu entry: vibe-talk is listed as an optional tool, and "Add a tool later" says
it is a separately deployed service whose QUICKSTART and README are the reference, with a
first-run checklist (native build or container, config and tokens outside synced directories,
`[storage] path` set, `--fake-discord` before a real provider, `[read_aloud] backend =
"browser"` with no voice vendor, TLS in front, user services). It does not ask which
connection route the user wants, does not create vibe-talk configuration, and does not pair a
phone. The new `vibe-talk/SETUP.md` is the hand-off it should point at.

## Friction points, each with a fix

| # | Friction (where a new user hits it) | Fix | State |
|---|---|---|---|
| 1 | No route map: QUICKSTART step 4 offers one worked Cloudflare example and says any ingress is fine, so LAN, SSH, Tailscale and proxy users must invent their route | `SETUP.md` section 2: six routes with steps, security notes and trade-offs, plus a comparison table | Done here |
| 2 | No pre-flight check: a config that loads can still 503 the channel views (no `[storage] path`), refuse summaries (default ElevenLabs backend with no credentials), never update live (poll off), and have nothing for pairing to build on (no `public_url`) — each found minutes later, one at a time | `vibe-talk doctor`: one line per check with its fix, exit 1 on FAIL, no network call, no token printed; unit-tested | Done here |
| 3 | Pairing needs `public_url`, discovered only when `pair-link` refuses | `doctor` warns before first start; SETUP step 1 and route sections set it per route | Done here |
| 4 | First-run default voice is ElevenLabs: with no vendor account every long message is a red failed summary, which reads as a broken install | SETUP recommends `[read_aloud] backend = "browser"` for a first run; `doctor` warns with the same fix | Done here (docs/check); page banner is a follow-up |
| 5 | Two configuration dialects (shell env file for `scripts/run.sh`, TOML for the server) with a precedence rule between them | SETUP names one path (TOML plus `doctor`) and says env wins when both exist | Done here (docs); unifying the launcher is a follow-up |
| 6 | QUICKSTART is Discord-first and interleaves the ElevenLabs voice half, so a Slack or bridge user and a messages-only user must filter six steps themselves | SETUP is provider-neutral and defers voice to QUICKSTART step 5 | Done here (docs) |
| 7 | `pair-link` "could not reach the server" did not say what to check next | Error now points at `vibe-talk doctor` and at the bind address in the configuration | Done here |
| 8 | iOS pairing is a paste-into-the-app flow that no install doc described in sequence | SETUP section 3 gives Android and iOS steps in order, including the unused-link paste | Done here |
| 9 | The site demo page described the quickstart but linked no route or pairing guidance | Site "Running it yourself" now links SETUP first | Done here |
| 10 | No channels after sign-in reads as an app bug; channels exist only in configuration | SETUP "Your first channel" says so and gives the restart-plus-doctor loop | Done here (docs) |

## Follow-up proposals for the coordinator (not done here)

1. **First-run welcome screen (#1781 mu-firstrun):** reuse the SETUP copy listed in the
   hand-off to that lane — "add your first channel" and pairing wording — so the page and the
   guide cannot drift. Include the no-storage and no-voice-vendor banners from findings 2
   and 4, driven by the diagnostics route rather than new state.
2. **One setup command:** fold `doctor`, the startup probe, and `verify-deployment.sh` behind
   one `vibe-talk setup check --url ...` entry point, keeping the existing scripts as its
   stages. Today a user must learn three names to answer "is it working?".
3. **Launcher unification:** make `scripts/run.sh` accept the same TOML the server reads (or
   generate the env file from it), so finding 5 has one dialect instead of a documented
   precedence rule.
4. **Release binaries:** publish a signed release binary per platform so the first install
   does not require a Rust toolchain or a container runtime; SETUP's build section then
   shrinks to a download and a checksum.
5. **Pairing from the page:** let a signed-in desktop browser mint and show a pairing link
   (write scope already mints codes), so `pair-link` is the agent path rather than the only
   path.

## Verification

`cargo test --lib doctor::` passes (6 tests, including the pre-implementation failure run).
`vibe-talk doctor --help` and a run against the example-shaped configuration were exercised
from the built binary during development; the path-selected validation gate is the authority
for the landed tree.
