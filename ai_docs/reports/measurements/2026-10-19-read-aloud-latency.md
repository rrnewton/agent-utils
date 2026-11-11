# Read-aloud latency: tap to first audio, and block boundaries

2026-10-19. `#375 read-aloud-latency`, `#376 read-aloud-blocks`. Working note; numbers are
content-free and every measurement below ran on a loopback test instance (its own port and tokens,
the in-memory chat backend, and a fake `vibe-talk-v1` voice bridge). Nothing here touched a deployed
instance or a real voice provider.

## The report

The owner taps a long message to hear it in the car and waits "a very significant lag" before it
starts. The server log at the time showed ten `POST /api/v1/channels/{id}/speech/prepare` requests
for one channel in flight together, each taking 46 to 53 s and all failing with 502, followed by the
read-aloud session ending on a provider-side stream error.

## Why ten preparations at once

`prepareSpeech` in `web/voice.js` runs when Read is turned on and again on every redraw of the
channel list. It collected every loaded row's unprepared ids and sent **one request per thread,
all at once** (`Promise.all`). The server prepares one thread per request; a threaded preparation
looks its messages up through the chat adapter, and that adapter serves lookups that go upstream
through one slot. So ten threads on screen were ten lookups queued behind each other, each request
waiting for the ones ahead of it: the last answered after roughly ten lookup times, and an adapter
time-out failed them together.

A tap on a row with no ticket yet prepares that row on its own (`#191 read-aloud-first-lag`), but
that request reached the same server-side lookup queue behind the ten, so the tap waited as long as
the slowest of them, and when they failed it fell back to the older route, which looks the message
up again.

## What a preparation does

No language model is involved anywhere on the read-aloud path.

1. Allowlist check, then the provider warm-up is *started* (never awaited): it opens or touches the
   one shared voice session.
2. The messages: from the 30 s read-ahead memory when the page fetched them recently, otherwise one
   window fetch (main channel) or one lookup by id (a thread) at the chat adapter.
3. Speech normalisation (`speakable::for_speech`): deterministic code, linear in the text.
4. A ticket per message: 256 random bits naming the already-normalised text.

The tap then opens `GET /api/v1/speech/{ticket}`: the server prompts the shared voice session with
the fixed verbatim-read framing (the provider's direct path, no model) and streams the WAV as it
arrives. The page plays it through Web Audio with a 0.5 s jitter cushion.

## Where the time goes, tap to first audible sample

Loopback, Chromium at a phone viewport, the fake bridge answering its first audio 600 ms after a
prompt and streaming at three times speaking pace. The page's own timing report and the server's
`read-aloud reached audible playback` line agree.

| Case | Tap to audible | Of which |
|---|---|---|
| Prepared row, warm session | 1.14 s | 0.60 provider first audio, 0.50 jitter cushion, 0.04 page and server |
| First read after a fresh server (joins the warm-up's session) | 1.34 s | plus 0.17 s waiting for the session the preparation started |
| Preparation (15 ids, from read-ahead memory) | 13 ms | |

So with a prepared row the page and the server add about 40 ms; the rest is the provider's first
audio and the deliberate cushion. The earlier deployed measurement (`#191`) had reads after the
first starting about 0.3 s after the tap (before the cushion was raised from 0.1 s to 0.5 s). The
owner's tens of seconds were the preparation queue above, not synthesis.

## What changed (`#375`)

- **One background preparation at a time.** Threads are prepared one after another, the rows on
  screen first, and each thread's tickets are usable as soon as its request answers.
- **A tap takes the batch's place.** A tap that needs a preparation cancels the batch request in
  flight (closing it frees its server-side lookup slot), prepares its own row, and starts reading;
  the batch resumes behind it. At most two preparations from one page are ever in flight.
- **A pace change cancels the batch** so tickets at the old pace cannot come back.
- **The provider's first-audio wait is logged with the length of what it was asked to say**
  (`said_chars`, a count), so a deployed log can tell whether first audio grows with text length.

Modelled on the incident (one upstream slot, about 5 s a lookup, ten threads on screen): the tap
used to wait behind up to ten lookups (about 50 s, then fail); now it waits behind at most the one
lookup already at the adapter plus its own (about 10 s at that degraded rate, one lookup time
normally). The page suite counts preparations in flight; the new tests fail on the old page.

## Session reuse

Already in place and confirmed on loopback: every read after the first reports
`session_reused=true`, and the preparation's warm-up opens the session before the tap. A session
the provider ends (as in the report) is replaced on the next read, which then pays one connection
and readiness handshake.

## Small first chunk

Not added. The deployed provider streams at speaking pace (`#47 read-aloud-overlong`) and a warm
read started about 0.3 s after the tap (`#191`); nothing measured so far suggests that wait grows
with the length of the message (an inference, not a measurement), so splitting the first block
would add a turn without shortening it. Blocks (`#376`) make the first request one block of the message anyway, and the
new `said_chars` field will show if that assumption stops holding.

## Block boundaries (`#376 read-aloud-blocks`)

A long message is now read one block at a time (paragraph, heading, list item, block quote; a code
block or table as a short description), one request per block under one selection, so the server
queues each block behind the one before it on the same voice session. The page asks for the next
block as soon as the current one has audio, and schedules it on the same audio clock to begin
where the current one ends, or half a second (the jitter cushion) after its first audio arrives,
whichever is later.

What a boundary costs therefore depends on how fast the voice generates compared with how fast it
speaks:

- **A voice that generates faster than it speaks** has finished the current block before it is
  heard, starts the next at once, and the next block is waiting when the current one ends: **no
  gap**.
- **A voice that streams at speaking pace**, as the deployed provider does (`#47`), cannot start the
  next block until the current one has been generated, which is about when it finishes playing; the
  next block's first audio then takes the provider's per-turn wait. That wait is **a pause at each
  boundary, about as long as the provider's first audio for a warm turn**. The page spends none of
  its jitter cushion on it, so a boundary never turns a later network stall into a longer gap. The
  deployed warm first-audio time was about 0.2 to 0.3 s (`#191`), a short paragraph pause.

Measured (loopback, real server, fake voice bridge with 300 ms first audio, six-block message):

| Voice | Boundary gaps | Tap to audible |
|---|---|---|
| Streams at speaking pace | 325, 330, 330, 330, 335 ms | 1.04 s |
| Generates 3x faster than it speaks | 0 ms at every boundary | 1.04 s |

The Chromium walk in the validation suite (`tests/read_aloud_blocks_browser.py`, fake page API with
a one-session voice) measures the same: at speaking pace each boundary equals the voice's first
audio (300 ms), generating twice as fast the boundaries are 0 ms, and a negative control with
1.2 s first audio shows 1.2 s gaps.

To remove the pause on a speaking-pace voice the next block would have to be generated while the
current one plays: a second voice session used only for the block after next, or a provider that
accepts the next prompt before the current turn ends. Neither is done here; the first doubles the
provider sessions read-aloud holds, and the second is not in the `vibe-talk-v1` protocol.

## Sentences (`#379 read-aloud-sentences`)

Sentence highlighting inside the current block adds no requests and so no pauses: another boundary
per sentence would add the per-turn pause above at every full stop. The page lights the sentence
whose share of the block's characters the reader has reached in the block's audio. The block's
length is exact once its audio has all arrived; before that (always, on a speaking-pace voice) it is
the block's spoken characters times the voice's rate, 0.068 s a character until a block of this
voice of 40 characters or more has arrived whole and been measured. The error is confined to one
block: every block starts its light on its first sentence when its audio starts.

## Timeline (`#380 read-aloud-timeline`)

The steering strip draws the read as a line of its blocks, each as wide as its spoken characters,
the heard ones tinted and the current one filled; a tap on one is a jump, so it costs what any jump
costs: the interrupt of the block playing and the next block's first audio plus the cushion.
