# Changelog

User-visible changes to agent-utils, newest first, grouped by week (Sunday to Saturday). Each
line names its tracking issue; `#N` is `.minibeads/issues/agent-utils-N.md` in this repository.
Older changes are in the git history. If your team went down, start with the
[recovery guide](common/docs/agentctl/recovery.md).

## Week of 2026-11-22 (Nov 22-28, in progress)

**docs**
- A team-recovery guide, [`common/docs/agentctl/recovery.md`](common/docs/agentctl/recovery.md), says what kills a team, what to run in order, how to verify it came back, and what to collect when it does not; the `team-restore` skill now covers `team manifest`, `team act` and `run-agent-prompt`. This changelog starts (#2132).

**wrkslots**
- The Rust edition runs `image status`, the image mount commands, the `worktrees-image` commands and `sandbox show-config` natively, with output matching the Python edition (#1095, #1101, #1116, #1141, #1279).

**wrkviz**
- The Rust edition supports `summarize --agent-scope coordinator` and `summarize-project` (#1077).

**vibe-talk**
- Read-aloud retries recoverable failures at the same block, records why each read stopped, and reports daily failure counters on `healthz` (#2056).

## Week of 2026-11-15 (Nov 15-21)

**agentctl**
- `agentctl team manifest` prints a redacted recovery-facts document (desired set, record gaps, queued prompts, units, lost work), and `team act` queues one allowlisted, idempotent recovery action for the team driver (#1559).
- New `agentctl redact NAME` rewrites a literal string, such as a leaked secret, out of one agent's retained history and archives, with `--dry-run` (#1963).
- `send --to-manager` and `send --to-children` address the caller's manager or children without naming them, and skip-level sends are marked (#1904).
- `rename OLD NEW --keep-alias DURATION` keeps the old name resolving for a while, managed with `alias list` and `alias rm` (#1777).
- `adopt --profile` and `bind-profile` attach a launch profile to an adopted agent, so restore and revive launch it correctly (#1993).
- Revive, restart and restore rebuild a lane's launch from its current profile, and every interactive profile must carry its harness's permission-bypass flag (#1822, #1928).
- `status`, `list` and other human-facing output redact launch arguments (#1937).
- Headless agents take console input as urgent, accept `/interrupt MSG`, always get a console tab, and the chat bridge reads their replies from the event log (#1879, #1973, #1974).
- Muse lanes record their native session at start, `restore` resumes them, and a second `restore` in the same boot replays its recorded outcomes (#1132, #1934, #1945, #2013).
- Launches answer Muse's plugin-hooks trust dialog by policy instead of stalling, and Muse placeholder hints no longer read as unsent drafts (#1773, #1888, #1893, #1932).
- Revive brings back a previous-boot lane whose pane is gone, finishes an interrupted revive from its recorded launch, and no longer deadlocks on its own lock (#1723, #1944, #1992).
- Prompts the harness transcript shows as taken count as delivered, typed prompts in narrow panes are submitted, and `send` always stops at its timeout (#1882, #1897, #2036).
- `watch` pages the owner at Claude Code hook confirmations, runs bounded scheduled tab hygiene, and reads background-task counts only from the live footer (#1784, #1918, #1942, #1971).

**run-agent-prompt**
- New `run-agent-prompt` sends one judgement-only, tool-free prompt to the first harness that answers (Muse, then Claude, then Codex) and prints only the answer (#1559).

**agentctl chat**
- The relay poll no longer starves the owner's channel rescan, and `chat health` reports a bridge that has stopped hearing the owner (#1892).

**wrkslots**
- A per-remote SSH command (`remote_ssh_commands`) reaches remotes that need their own SSH setup (#1940).
- `doctor --stray-worktrees` lists worktrees created outside wrkslots without touching them (#1930).
- `doctor --quick` gives a fast snapshot check, and `status` and `doctor` report interrupted mutations with the command that recovers them (#1700, #1706).
- `finish` handles slots holding only ignored files, a finished slot can still publish its handoff, and `remove-agent-batch` archives ignored files (#1686, #1753, #1868).
- Fields written by a newer wrkslots no longer break older readers of the event log or archives (#1582, #1788).
- Long removals no longer hold the registry lock while salvaging and deleting, and `--wait-lock` also waits out a running finish (#1437, #1735, #1755, #1757).
- Eden-backed slots mount outside the project, roll back a failed create, support several backing repositories, and finish and remove cleanly (#1554, #1555, #1558, #1702, #1730, #1804, #1805, #1925).

**dagrun**
- Learned memory caps keep headroom above the observed peak, so light steps are no longer killed under contention (#1763).

**vibe-talk**
- A Documents view lists shared documents newest first and opens them, or document links in messages, in an in-app reader (#1022, #1041, #1047, #1073).
- The Documents list gains file-name search and star and downloaded filters, and keeps its exact place across Back, view switches and downloads (#1767, #1908, #1909, #1999, #2001, #2005).
- The Documents reader gains React and discussion jumps, and Read starts at the first thing on screen (#2003, #2004, #2006).
- The docked Discuss pane resizes by dragging its title bar, Cmd+D docks or floats it, and its state survives a refresh (#1552, #1556, #1821, #1978).
- Discuss summaries follow the open thread, cover long threads whole, start where the last summary ended, and show earlier summaries and questions as a timeline (#1761, #1811, #1842, #1912).
- Search sorts server hits behind toggle chips, recalls earlier terms with Up and Down, closes with a prominent X, and matches inside link addresses in Links mode (#1786, #1797, #1816, #1852, #1895, #1968).
- Typed hashtags become filterable labels with chips and search, and the Pinned button becomes a Labels drop-down (#1825, #1853).
- Settings gain Appearance (System, Light or Dark), Channel is the default view, and a first-run Welcome and setup guide explain the app (#1780, #1781, #1845).
- Composer drafts sync across devices (#1840).
- Transcript threads appear only in a new Transcripts view, which opens at the newest (#1886).
- Read-aloud continues into a thread and back out, and one click on a message link while reading jumps there and reads it (#1838, #1959).
- `:thread:`, `:msg:`, `:email:` and `:issue:` short references render as links (#1565, #1607, #1796, #1889).
- The page never scrolls a view the user has scrolled, and a reload keeps the view, the linked message and the scroll place (#1849, #1880).

**other**
- local-ci-hub gains a host-limits file with live admission and a limp mode, and writes terminal records under a versioned receipt contract (#1754, #1806).

## Week of 2026-11-08 (Nov 8-14)

**agentctl**
- The agent console gains `/effort` and `/model`, and `agentctl set-model` records a per-agent model override applied at the next turn boundary (#1396).
- `run-headless` now supports Muse agents as well as Claude Code (#1234).
- New `agentctl kill-pid` signals a process only when its command line and start time match, and refuses the user manager, Herdr and other teams' processes (#1420).
- `agentctl team verify` says with evidence whether a restored team really came back, and `team kick` runs a per-team driver that restores, verifies, hands off to the lead and escalates to a human through configured commands (#1414, #1416, #1417).
- Restore copes with a user-manager restart without a reboot, waits for a usable certificate, and boot restore units retry instead of giving up (#975, #1137, #1405).
- Revive, restore and restart deliver an agent's queued messages to its new generation and keep the profile's launch environment (#1023, #1139, #1412, #1413).
- Queued messages can carry a supersede key, periodic automated notices expire, and the whole queue is delivered at a turn boundary (#1156, #1157, #1479, #1480).
- Profiles can declare `required_env`, refusing a launch that lacks it, and Codex lanes accept `max` and `ultra` effort (#1319, #1429).
- Prompts drawn but not submitted in Muse, Claude Code and Codex panes are now detected and submitted, and long briefs are no longer quarantined as misroutes (#1009, #1085, #1323, #1427, #1500, #1546).
- `agentctl services turnover` moves every agent-utils daemon on a host to main, only where its code changed, and refuses a downgrade (#1179, #1289, #1493).

**agentctl chat**
- `agentctl chat` refuses a near-duplicate post in the same channel and thread, and deletes the message file once its text is stored (#1239, #1240).
- The owner can send a raw message, typed as keystrokes into the bridged agent, from a Send raw message checkbox in vibe-talk (#1135, #1136, #1223).
- Chat-driven coordinators cannot park on an in-terminal question, a stale prompt delivers or alarms, and the alarm names the blocked prompt (#1158, #1160, #1372).

**wrkslots**
- An optional `identity:` setting applies a git identity privately to every new slot checkout, and `doctor` reports checkouts that differ (#1096, #1097).
- The worktrees image can mount directly at `worktrees_dir` with slot subvolumes as top-level entries, and its mount, migrate and bind handling is hardened (#1147, #1148, #1149, #1150, #1152, #1162).
- Older wrkslots versions accept new optional config keys and journal fields, `machine` may be `auto`, and post-provision hooks can target slot types (#1109, #1122, #1124, #1163).
- Removal refuses to delete gitignored files outside regenerable caches unless archived or discarded, names unpushed submodule history, and records every ref it found (#1255, #1256, #1302).
- TTL teardown no longer deletes live-owned or held slots, and `clean-caches` skips a slot whose gate is running (#1029, #1491).
- `status`, `audit`, `remove` and `retire-pending` hold their locks far more briefly, and salvage writes much smaller bundles (#1153, #1172, #1247, #1287, #1367).

**merge-queue / gate**
- The merge queue validates each root stacked on the candidates ahead of it and lands by checked fast-forward (#1071, #1475).
- Validator runs receive `MQ_BASE`, `MQ_PARENT` and `MQ_PURPOSE`, `submit --no-exact` clears an exact pin, and submit can require an approval command (#1080, #1094, #1377, #1381).
- `gate.py` fast-tracks owner-requested gates, bounds every ticket wait with aging, and queues a full gate only after a local pass (#1180, #1219, #1262).
- Gates defer while host-configured units or measurement holds are active, with exclude patterns and an observe-only mode (#1406, #1489).

**wrkviz**
- Sessions are labelled coordinator, implementor or reviewer with a confidence, and the summarizer model can be chosen per summary kind (#1155, #1324).

**vibe-talk**
- New Discuss panel: a side agent writes a grouped Markdown catch-up of unread messages with linked citations, saved on the server, and answers follow-ups (#1345, #1346, #1348, #1349, #1350, #1353, #1356).
- Discuss opens as an overlay, docked or maximized panel, resizes by drag, docks under the list on phones, and toggles with Mod+8 (#1344, #1354, #1355, #1386, #1397, #1526).
- Discuss summaries fold by workstream, remember their folds, read aloud like messages, and treat the owner's messages as context, never unread (#1384, #1387, #1497, #1501, #1535).
- The Documents view lists newest at the bottom, searches, stars and hides documents, fetches visible or recent ones with progress, and Back walks through documents (#1079, #1082, #1263, #1264, #1265, #1266, #1307, #1308, #1309).
- The document reader gets a pop-up outline, message-style read-aloud with block highlighting, a multi-level scrubber, and clearer headings and tables (#1038, #1039, #1236, #1327, #1330, #1505, #1508).
- New `vibe-talk search` and `vibe-talk prompts --hours K` commands search the conversation history and print the owner's recent messages (#1338, #1529).
- Voice calls are saved on the server and share one transcript across devices (#1303).
- Drag the separator to size the two columns, a chip jumps to the last unread message above, and the Newest chip counts messages below (#1057, #1167, #1186).
- Threads can be pinned in the thread picker, a synced setting chooses what `e` does after Done, and Post to main starts a new top-level thread from a reply (#1052, #1321, #1550).

**other**
- `agent-teams topology check` has lead and subordinate teams check in, probe each other, and kick a downed peer (#1418).
- gh-paced admits admin API calls listed in a scoped host-local allowlist (#1476).

## Week of 2026-11-01 (Nov 1-7)

**agentctl**
- `restore` brings a team back idempotently from a desired-state spec, `install-units` installs its persistent user units, and coordinators get an I'm-back summary (#740, #742, #743, #746, #810).
- Revive reuses idle shells Herdr restored, finds recorded Claude Code and Codex sessions, revives stopped records with `revive --archived`, and abandons dead candidates (#741, #744, #899, #920).
- New `agentctl handover OLD NEW` swaps coordinators atomically, and `coordinator set` refuses a dead target (#721, #747).
- A coordinator's Herdr tab moves to the front, and `watch` keeps a canonical tab order and closes idle bare shells no agent owns (#849, #976, #979).
- `watch-agents` pages the coordinator about requests in agents' status files (`agentctl requests`), and `set NAME --standing` lets an agent rest idle without notices (#862, #901).
- Queued prompts are delivered at the next turn boundary, and drafts agentctl typed but never submitted no longer block later deliveries (#720, #731, #804, #808, #825, #989).
- `start` and `adopt` record a Muse lane's native session, and `start --resume` and revive recognise resumed Muse and Codex lanes as ready (#811, #916, #921, #928, #946, #947).
- Unrecognised terminal programs are adopted as the `unknown` harness, which pins the pane's real foreground program (#852, #913).

**agentctl chat**
- Owner-only `!` commands list sub-agents (`!who`), open a thread with one (`!@NAME`) and close it (`!end`) (#917).
- An opt-in trace mode posts the agent's non-reply transcript text to a daily trace thread, tagged with metadata (#714, #832).
- A reaction shows how many minutes delivery took, on by default, and requests record model, effort and harness for per-profile latency summaries (#801, #802, #848, #897).
- New `chat settle KEY --settle-as delivered|abandoned` clears a delivery-uncertain request so chat health can recover (#784, #888).
- A restarted bridge holds stale unsent replies (`--hold-replies-older-than`, default 15 minutes) for the coordinator to send or drop, and backfills after an outage stream gap (#745, #781).

**wrkslots**
- `transfer-owner --agent OLD --to-agent NEW` hands a slot whose owner is provably dead to a differently named agent (#1018).
- New `freeze set|clear|status` makes a host refuse every slot retirement and removal while the freeze is set (#780).
- New `reap-stray-worktrees` finds Git worktree registrations no slot or process owns and prunes them with `--apply`, keeping rescue refs (#775, #776, #777).
- A failed provision fetch names the `insteadOf` rewrite behind it, and ssh and https spellings of one remote count as the same remote (#868, #882).
- `create` and `recover` verify every checkout before reporting success, and removal also reclaims the slot's state home and root-owned build files (#796, #881, #911).
- Journals left by dead or interrupted actors no longer block unrelated creates, and `recover` rolls a lagging journal forward (#885, #912, #992, #1007).

**dagrun**
- Per-step priority hints decide which ready steps run first when parallelism is short (#860).
- `--skip TAG=REASON` skips steps at runtime with a recorded reason, and their dependents are skipped too (#867).

**merge-queue / gate**
- New `merge-queue` plans conflict-free batches of ready changes at exact heads, validates them, and lands each change only on its own green; a priority label moves a change to the front (#732, #733, #735, #817, #1008).
- Conflicts confined to declared generated paths are resolved by regenerating them, and `submit --exact` lands a merge commit with both parents intact (#922, #945).
- A decision log records every validation, summarised by `merge-queue stats` (#822, #824).
- `donor run --cap-file` changes a donor's job cap without a restart, and an optional pre-claim check hands back jobs that fail it (#918, #1015).
- A validator that cannot run is an infrastructure refusal, never a red verdict, and withdrawn or stale candidates no longer stall a run (#814, #816, #1006, #1010, #1013, #1016).

**wrkviz**
- Summaries default to the Muse backend in both editions, with Anthropic and Codex still selectable by `--backend` (#839, #842).

**vibe-talk**
- Selecting words in a message offers a Quoted reply chip and a floating quick-reply box that never moves the page (#726, #727).
- A reply quoting under 5% of a message leaves it unread, and message times older than today show the weekday or date (#728, #729).
- A hamburger menu holds common actions beside a real Voice/Channel button, lit blue for Voice (#820, #845, #880).
- Read-aloud can pause on a sentence for a voice reply to exactly that sentence, Autoplay reads the next unread message, and tables are read row by row (#821, #847, #886).
- Cmd+L copies the selected message's link, a deep-linked message stays highlighted until dismissed, and Done updates at once (#889, #890, #892).
- The Draft chip finds every unsent draft, ignores blank ones and really discards on confirm (#819, #846).
- New `vibe-talk recap` prints an unread window of chat history grouped by thread (#717).

**other**
- agent-teams can relay inter-team messages through a chat channel, mirroring direct deliveries and taking over when a direct route is down; `relay trust-key` trusts another team's key (#722, #770, #782, #790).

## Week of 2026-10-25 (Oct 25-31)

**agentctl**
- Headless agents run by `run-headless` get a typed event log, a readable coloured `agentctl tail NAME` with log rotation, and `watch-agents` status (#582, #583, #588, #589, #590).
- `agentctl takeover` and `handback` switch an agent between headless and an interactive session, never with two writers (#591).
- Agents can run and be managed entirely under tmux, with no Herdr, chosen per profile or registry (#507, #514).
- `send` delivers to a busy Muse by steering its running turn instead of waiting for it to go idle (#505).
- `send` and drain give up after a configurable deadline (10 minutes by default), hold no lock while waiting, and refuse prompts too large to type (#702, #703, #705).
- Every `agentctl send` and every tick-hub notice now carries a mandatory header naming its sender (#609).
- Per-agent registry entries move under `agents/` and `locks/`, apart from the registry's fixed files (#593).
- `watch-agents` tells the coordinator the moment a child shows a DONE or BLOCKED verdict, and `agentctl hold NAME --after OTHER` queues landings (#509).
- `watch-agents` tells a stuck agent from a progressing one, shows queued input, classifies blocked harnesses, and reports approval prompts on its next pass (#570, #574, #585, #675).
- `watch-agents` stays fast on large registries and marks a notice reported only after it is delivered (#571, #669).
- `agentctl services` guides a rebuild and restart of a team's long-running services from release binaries (#701).

**agentctl chat**
- The bridge reads agent replies from the harness transcript instead of scraping the screen (#710).
- A chat request prompt is one header line, a fenced message, and short reply commands that find their bridge (#707).
- `chat repoint` retargets a running bridge to another agent or pane live, with health checks (#677).
- A headless coordinator can be bridged to a chat channel, and its permission prompts go to the owner in chat as approve or deny (#594, #601, #671).
- Owner messages are never batched, reach a working coordinator or Muse promptly with receipts, and messages with screenshots arrive again (#556, #615, #678, #690, #698, #700).
- The bridge verifies the target pane before typing, bounds uncertain deliveries, keeps reply order, and survives a slow git check (#581, #587, #681, #682, #683).
- A selectable converted Slack outbound path keeps Markdown headings, tables and line spacing (#530, #534, #550, #551).

**herdr-run**
- herdr-run keeps only a few idle command tabs and reuses the least recently used one (#527).

**wrkviz**
- `summarize` never leaves a partial tree silently, and ingest reads compressed upstream sources at months-long scale (#535, #568).

**vibe-talk**
- The server gathers each channel's whole history into its cache, and page reads and read-aloud are answered from it with Slack behind (#595, #611, #614, #617, #622, #650).
- Settings show each channel's history mode and crawl progress, with a prioritise-now button, and the crawler honours provider rate limits (#602).
- Search runs on the server: an All channels checkbox, results that follow the view, a deeper reach, and an honest scope line (#578, #603, #604, #605, #606, #607, #608).
- The Rust server supervises the Slack tail itself, retiring the Python adapter (#709).
- Read-aloud gains a local CPU voice, another cloud voice and a per-block fallback chain; a silent or failed cloud read stops and says why (#548, #552, #561, #563).
- The voice socket is served through vibe-talk with its own authentication, and call audio can route to the system or Bluetooth device (#553, #619).
- The voice agent reads from the cache, keeps every spoken turn's text, and sends to chat only when explicitly asked (#511, #512, #579, #649).
- Chat permalinks open the message inside vibe-talk, and Back returns (#543, #580).
- Fenced code blocks are syntax-highlighted on the server (#531).
- A page running an older bundle than the server serves says so and offers a reload (#529).
- Two small ticks after Sending mark server and upstream delivery (#621).
- On phones the message box sits flush on the keyboard, pasting on Android no longer sends, and the read strip spans the screen (#566, #573, #575, #610).
- Whatever the reader is looking at no longer jumps away when a thread re-sorts (#612).

**other**
- New `agent-teams` tool delivers exactly-once messages between teams' coordinators across hosts, including over ssh (#504, #557).
- Paired tools such as agentctl, dagrun, herdr-run and tick-hub now run their Rust editions by default; `AGENT_UTILS_ENGINE=python` opts back (#503).

## Week of 2026-10-18 (Oct 18-24)

**agentctl**
- New `watch-agents` tells the coordinator when an agent is stuck at an approval prompt, idle without a verdict, stalled, or dead (#381, #393, #397).
- `answer --text TEXT --replace-draft` safely replaces an inspected, unsent free-text answer to a Codex question (#446).
- Muse lanes launched with launch-flag overrides or `--approval-mode never` now reach a verified idle prompt and accept typed input (#382, #395).

**agentctl chat**
- Chat messages with attached files, such as screenshots, now reach the agent as requests, rescued by a rescan when the event stream omits them (#415, #485).
- The bridge prompt asks agents to answer separate topics in separate chat replies (#435).
- `chat reply` wakes a running `chat run` without `--offer-reply-command`, and replies to requests typed mid-turn into Claude Code are captured (#391, #392).
- A quiet channel no longer lets the Slack cursor fall out of the event stream's retention (#399).
- The Slack outbound helper works with a bot-run client, and the guide documents separate inbound and outbound identities (#383, #390).

**wrkslots**
- `create`, `adopt` and `transfer-owner` refuse agent names the liveness probe cannot judge (#413).
- `remove --coordinator-authorized --attest-agent-stopped AGENT` removes a dead-owner slot whose agent was never registered (#341).
- Slot image mounts copied into unrelated mount namespaces no longer block removal or get mounted twice (#350).

**herdr-run**
- Finished command tabs are retired after an idle TTL (`idle_tab_ttl_seconds`), with a sweep that can run from a timer (#412).

**wrkviz**
- Extracted transcripts are stored as seekable multi-member gzip with an index; older archives must re-run `extract-transcripts` (#329).

**vibe-talk**
- A one-time, short-lived pairing link signs a phone in (#369).
- With `[message_cache] serve` on, page loads are answered from the message cache at once, fed by live Slack ingest and a history backfill (#384, #452, #453).
- The page URL selects the view, and a link can name a single message (#462).
- A message's ⋯ menu copies its link or opens it in Slack, Google Chat or Discord, optionally in the Slack app (#408, #409, #461, #481).
- Reply opens an inline editor in the stream under the message, one box at a time (#410, #467).
- Read-aloud goes block by block, lights the sentence being said, shows a tappable timeline, and its strip gains Play/Pause and Stop (#376, #379, #380, #434, #442, #451).
- Reactions added in the native app show in vibe-talk within seconds (#378).
- Settings sections fold and tile into columns on wide screens, prose becomes tooltips, and All channels sets Clear unread everywhere (#345, #346, #347, #348, #349).
- Every setting outside Screen and keys is stored on the server and shared by all devices (#457).
- With several chat services in one server, channel pickers group channels by service (#355).
- The Threads view shows the main channel and its message box, unpacks threads from their cards, and sorts by update or creation (#404, #419, #426, #440).
- One read mode (Show, Fold, Hide read) applies to every view, thread cards included (#432).
- Search shows unmatched runs as expandable separators and loads older pages in every layout (#262, #439).
- Thread colours are chosen by the server, survive reloads, and neighbouring threads get clearly different hues (#396, #414, #427).
- The voice agent gets the full text of recent messages, recognises the owner's own messages, and stops claiming abilities it lacks (#437, #438, #441).
- Calls can resume on the websocket backend and survive a bridge error, and long spoken summaries and relayed messages are no longer dropped (#330, #331, #337, #352).
- A Settings choice and Ctrl/Cmd+Shift+9 and 0 set how many lines a collapsed message shows (#425).
- `z` and Ctrl/Cmd+Z undo the most recent undoable action of any kind (#443).
- Cmd+/ opens search from anywhere, Ctrl/Cmd+[ and ] go Back and Forward, and Ctrl/Cmd+S reaches the message box from any focus (#344, #403, #422, #423).
- A Settings switch, on by default, hides empty messages everywhere (#386).
- A refused send says why, a slow multi-part send is not posted twice on Retry, and Google Chat mentions are defused instead of refused (#444, #445, #448).
- Each deployment can name its installed app, and vibe-talk is tagged version 0.1.0 (#343, #456).
