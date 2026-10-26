# car-mode-design: reading, pausing, replying and resuming with big targets (proposal)

[opus 5.5] `#547 car-mode-design`. A proposal for the owner, written 2026-10-26. Nothing in it is implemented except the bug
fix in "Where we are". It follows `#554 read-pause-reply-resume`. It is meant to sit beside the bottom-bar
proposal, [`2026-10-22-channel-bars-layout.md`](../2026-10-22-channel-bars-layout.md) (group I of
[`2026-10-23-vibe-talk-next`](../2026-10-23-vibe-talk-next/README.md)).

## What the owner asked

> When reading a long message I hear something I want to reply to in the middle. I cannot always save
> all my replies to the very end without losing something. So I press pause on the audio and then I
> press the tiny little reply button. And then I press the microphone button and then I speak my reply
> and press send. First of all, that was all too many button presses and too small for a good car mode.
> In the future, I think pressing the read button should switch us into a full car mode and possibly
> update other things on the UI accordingly. Zoom for instance has a way to swipe left to the car view
> which just has a big tap to speak button. [...] I want to click on the message that I want to read.
> I want to get this nice overlay interface but I also want it to be very easy to pause dictate my reply
> and then resume the reading of the text message where I was originally at. I don't actually care if it
> sends my reply because I'm fine with making multiple small replies as a batch to the original message
> that's being read out.

And later the same day (request 219, an idea the owner wants thought through, not built yet):

> On mobile, it might make sense to go into a kind of full-screen view on the message when we're reading
> it chunk by chunk. If we scroll up and down the message and it's a big one, then maybe our scrolls
> should be limited to that message and not accidentally go further up or down while reading is on. If
> reading is paused or stopped, maybe the other messages should come back.

## Where we are

`#554 read-pause-reply-resume` fixed the bug that made the owner's test fail. Chrome on Android has no
speech pause. Its `speechSynthesis.pause()` stops the voice and its `resume()` does nothing, so with
the device voice Resume was silent. Now the device voice stops at the sentence on Pause, and Resume says
that sentence again from its start. The device voice now speaks one sentence per utterance. The agent's
voice already resumed correctly. So the flow the owner tried now works, but it is still the flow they
described:

| Step | Today | Target size |
|---|---|---|
| 1 | Read (bar), then tap the message | bar button, a row |
| 2 | Pause (the Read button, now labelled Pause) | 87px bar button |
| 3 | Reply on the message | small chip in the row's meta line (about 50x24px) |
| 4 | the keyboard's microphone | the keyboard's own key |
| 5 | speak, then Send reply | 110x44px |
| 6 | Resume (the Read button) | 87px bar button |

That is five taps after the read starts, two of them on small targets. Most of them need the driver to
look at the phone.

Other things already on the page that car mode can reuse:

- **The read engine, with one pause path** (`#434 read-aloud-pause`). The Read button, the Media
  Session keys (a steering wheel's or headset's play, pause, next and previous) and the keys all use it.
- **The read strip**: Previous, k of n, Next for a message read in blocks (`#376`, `#442`).
- **The sentence highlight** in the drawn row for the agent's voice (`#379 read-aloud-sentences`). The
  device voice now speaks one sentence per utterance, so it could light its sentence the same way. That
  is not done yet.
- **The inline reply box** under the message (`#410 inline-reply-editor`). A reply marks the message it
  answers read (`#300 reply-marks-read`), and several replies queue in the outbox in order.
- **`#429 keyboard-hides-bar`**: the bars go while the keyboard is up, and a slim Stop row stays while
  a read is going.

There is one gap. The device voice does not claim the Media Session. Only a multi-block read by the
agent's voice does, so a steering wheel's pause does nothing to a device-voice read today.

## The idea in one picture

A **car view** is one screen in portrait. It shows the message being read in large type, and the lower
half of the screen holds a few very large buttons. The list, the pickers, the chips and the composers
are all out of sight until the view is left.

```
┌──────────────────────────────────────────┐
│ ‹ Exit      #lead-team · thread "deploy" │  ← where this message is; Exit = Back gesture
│ ci-bot · 01:12 · 3 of 7 unread           │
│                                          │
│  Before we cut the release, is the       │
│  migration on the reporting replica      │  ← the message, ~1.4x type, scroll
│  ▌finished? Whether the dashboards▐      │    confined to it; sentence lit
│  picked up the new partitions matters    │
│  too, because last time …                │
│                                          │
├──────────────────────────────────────────┤
│   [ ⏮ ]        [  ⏸  Pause  ]      [ ⏭ ] │  ← 72px row; ⏮/⏭ = sentence/block,
│                                          │    long-press = message
│ ┌──────────────────────────────────────┐ │
│ │                                      │ │
│ │          🎤  Tap to reply            │ │  ← half the lower area, ~180px tall
│ │                                      │ │
│ └──────────────────────────────────────┘ │
│  2 replies queued to this message        │
└──────────────────────────────────────────┘
```

**Tap to reply** pauses the read at its sentence and starts dictation, showing the words large as they
are recognised. A second tap, or a pause in speech, finishes the reply. The reply goes to the outbox as a
reply to the message being read, and the read resumes on its own from the sentence it was paused in. In
the owner's words: "pause, dictate, reply, resume", in one tap or two. Each tap is one reply, and several
in a row queue in order ("I'm fine with making multiple small replies as a batch").

## Decisions to make, with my recommendation for each

### 1. How you get into car mode

| Option | Tradeoff |
|---|---|
| **a. Read enters car mode on a phone** (the owner's suggestion) | One way in, and the obvious one. Reading on a phone without car mode (in bed, on a desk) loses the list around the message unless there is a way back, so a setting "Read opens the car view" (default on for phones) keeps that open. |
| b. A swipe to a car view (the Zoom precedent: swipe left from the meeting to a view with one big talk button) | Discoverable only if you know it. It conflicts with the row swipes the list already uses (swipe to Done), so it would have to start at the screen's edge, where Android's back gesture already lives. |
| c. A separate "Car" button in the read panel of the bars proposal (`[source] [Pace] [Read] [🚗]`) | Explicit. One more button, and the most likely entry point (Read) still does the old thing. |

**Recommendation: a**, inside the bar redesign. In Option B of channel-bars-layout, `🔊 Read ˄` opens the
panel and the panel's Read arms reading mode. On a phone that Read opens the car view, with a setting to
keep inline reading. On a desk, Read keeps today's inline mode, and there is no car view.

What "tap the message that I want to read" means then: the car view opens on the message the reader
taps. If Read is pressed with no message tapped, it opens on the first unread message of the view on
screen. Inside the car view, ⏭ long-press moves to the next message.

### 2. How dictation is captured

| Option | One tap? | Privacy and dependence | Notes |
|---|---|---|---|
| a. The keyboard's dictation (works today) | No: the page can open the box and raise the keyboard, but cannot press the keyboard's microphone | Whatever the keyboard does | Cheapest. Cuts the flow to Reply, then mic, then Send, still with the small key. |
| **b. The browser's `SpeechRecognition`** (Chrome on Android) | Yes | Chrome sends audio to its speech service unless on-device recognition is offered and chosen (newer Chrome versions can recognise on the device, depending on the device's language packs) | The page controls start and stop and sees the words as they come. Device read-aloud already insists on local voices, so this needs the same "local only, or ask" rule. |
| c. The server's own speech path (record with the microphone, transcribe on the server through the voice stack) | Yes | The deployment's own voice service | Heaviest. It depends on the same cloud audio that is not working for the owner today. |

**Recommendation: b, with a as the fallback**, and on-device recognition whenever Chrome offers it. In
phase 2 (below), Tap to reply uses option a. That lets us ship the big target and auto-resume before
any speech-recognition work.

### 3. Send immediately, or confirm?

A misheard reply posted to a team channel is the main risk of one-tap replies. The options:

- confirm on screen (a Send button: one more look and one more tap, which is what car mode is trying to
  remove);
- **send after a short spoken readback with an Undo**: the device voice says "Reply: … Sending", and a
  large Undo fills the reply area for about 4 seconds. Then the reply goes to the outbox, and the read
  resumes;
- send at once and rely on edit and delete afterwards (vibe-talk can edit a sent reply on some services,
  not all).

**Recommendation: readback plus Undo.** The owner said "I don't actually care if it sends my reply", so
the default should be to send, with a way to catch a bad transcription without looking at the screen.

### 4. Where the reply goes

The reply goes to the **message being read**, threaded as Reply threads it today (`replyRoute`). The
first reply marks that message read, as `#300 reply-marks-read` already does. That changes nothing
audible: the read goes on, and the message is archived again when the read ends. A batch of replies
queues in the outbox in order. The count under the button ("2 replies queued to this message") is the
only on-screen sign, and the readback says each one.

### 5. Resume after a reply: automatic

The read resumes on its own once the reply is sent, or once Undo's window closes. It resumes from the
start of the sentence it was paused in (device voice) or from where its clock stopped (agent's voice),
as `#554` now guarantees. An explicit Pause before replying keeps the read paused after the reply.

### 6. The steering wheel and headset

Car mode claims the Media Session for **both** voices. Today only a multi-block read by the agent's voice
does, and that is the gap noted above. Play and pause hold and resume the read. Next and Previous step by
sentence or block. A double "next" could mean the next message, if the owner wants that (question 6).
A car's own voice button is not something a web page can receive.

### 7. The full-screen read view (request 219)

The owner's idea: on a phone, while a message is read chunk by chunk, show that message full screen,
keep scrolling inside it, and bring the other messages back when reading is paused or stopped.

**What it gives.** A long message can be followed and scrolled without flinging the list into other
messages. The highlighted sentence has the whole screen. A tap anywhere means this message, which matters
in a car. It is also the top half of the car view sketched above. The car view is "the full-screen read
view, plus big controls".

**How to build it:** as an overlay sheet that holds one copy of the message, not by clamping the list's
own scroll. Clamping the list fights the scroll anchoring, live arrivals, folds and gathered replies that
the list already juggles (`#280 scroll-stability`). A sheet has only one message in it, so it cannot
scroll past it, and the list under it keeps its place untouched. Then "the other messages come back" is
just the sheet closing, and the list is where it was.

**Where it applies:**

| Option | Tradeoff |
|---|---|
| Car view only | Smallest change. Reading on a phone without car mode keeps the list and its context (earlier messages, the thread around it). |
| All phone read-aloud | One behaviour to learn. But a short message read in full screen is a flash of a sheet for three seconds, and reading down a list one message after another (tap, listen, tap the next) becomes open, close, open. |
| All phone read-aloud, only for messages taller than the screen | Fits the problem: scrolling "a big one". But the screen then changes depending on the message, which is less predictable. |

**Paused and stopped are different, and I suggest they act differently.** The owner suggested both bring
the list back. I would keep the sheet on **pause**. In a car a pause is almost always "I'm about to
reply", and the reply belongs to this message. If the list came back under the thumb at that moment, the
layout would jump and the place in the message would be lost. Pausing leaves the sheet, dims the lit
sentence, and puts Resume and Reply where Pause was. **Stop** and **the end of the message** close the
sheet, and the list comes back with the message in its place, marked as heard. (In car mode, the end of
a message could instead move on to the next unread message: question 7.) If the owner prefers that pause
bring the list back outside car mode, that works too, because the sheet is only an overlay. It just
should not happen inside the car view.

**Getting out:** the Android back gesture or browser Back (the sheet pushes a history entry, as in-app
message links do since `#543`), an Exit at the sheet's top left, Stop, or a swipe down on the sheet's
header. All four close the sheet and leave the read stopped, except a swipe down while playing, which
could leave the sheet and keep reading inline (question 8).

**The highlight and auto-scroll.** The sheet follows the lit sentence: when the sentence leaves the
middle third of the sheet, the sheet scrolls it back. If the reader scrolls by hand, following stops,
and a "Follow" chip appears. Following resumes on the chip, on the next block or Resume, or after 8
seconds without touching. Without this rule, a hand scroll is undone at the next sentence. For the device
voice this needs the sentence highlight first, which is now simple because each utterance is one
sentence.

**On a desk:** no sheet. A desk has room for the message and its context at once, so it keeps the inline
highlight in the list. The two-column layout can show the message being read in the right column if
that helps. A sheet over a 1280px window would hide the context for no gain.

**Recommendation: build it as part of the car view first** (it is the car view's top half). Ship it with
pause keeping the sheet and stop closing it. Outside car mode, offer it on phones only for messages
taller than the screen, behind a setting that starts off. Turn it on by default only after the owner
has tried it in the car view.

## How it fits the bottom-bar overhaul

- Channel-bars-layout's Option B folds read-aloud behind `🔊 Read ˄` and a one-row panel
  `[source] [− Pace +] [Read]`. Car mode adds no bar control. The panel's Read is the way in on a phone
  (decision 1).
- The car view hides both bar rows, as `#429 keyboard-hides-bar` hides them for the keyboard. The bars'
  controls (channel, view, pace, source) are not needed while driving. Pace could be a long-press on
  Pause if the owner wants it.
- The read strip (Previous, k of n, Next) becomes the car view's ⏮ / ⏭ row. The page keeps one pause
  path: the car view's Pause is the same `setReadingPaused` as the bar's Read and the Media Session.
- The overhaul should land first, or at least its panel should, so car mode has an entry to hang on.

## Phases

1. **Done:** `#554 read-pause-reply-resume`. Pause, reply and resume works on both voices.
2. **Car view shell with keyboard dictation.** This is the full-screen sheet with the big ⏮ / Pause / ⏭
   row, Exit and Back, and a big Reply. Reply pauses the read, opens the reply box in the sheet and raises
   the keyboard; Send resumes the read on its own. It also claims the Media Session for the device voice
   and lights the device voice's sentence. Taps per reply: 3 (Reply, the keyboard's mic, Send), down from
   5, and only the mic key is small.
3. **One-tap dictation.** Tap to reply uses `SpeechRecognition` (on-device where offered), with readback,
   Undo and auto-resume. Taps per reply: 1, or 2 to finish early.
4. **Polish**: auto-advance to the next unread message, landscape (a car mount is often landscape), and
   the optional sheet outside car mode.

Each phase gets a Chromium walk on a 393x852 touch screen, with device voices modelled on Chrome for
Android, as `tests/read_pause_reply_browser.py` does. Each also needs the owner's real phone, because
neither Android's audio focus (what the keyboard's microphone does to the read) nor its speech services
can be faked faithfully in a headless browser.

## Questions for the owner

1. **Entry:** should Read on a phone open the car view (my recommendation), or should car mode be its own
   button or swipe?
2. **Dictation:** is Chrome's speech recognition acceptable when on-device recognition is not offered, or
   should car mode fall back to the keyboard's dictation (option a) in that case?
3. **Sending:** readback plus a 4-second Undo, or a Send tap after all?
4. **Resume:** should the read always resume after a reply, even if you pressed Pause yourself before
   Reply? (I suggest: only if Reply did the pausing.)
5. **Full-screen view outside car mode:** off, on for long messages only, or on for every read?
6. **Steering wheel:** should a double press of next skip to the next message?
7. **End of a message in car mode:** stop and show the list, or go on to the next unread message, with a
   short "Next: from ci-bot in deploy"?
8. **Pause outside car mode:** keep the full-screen sheet (my suggestion), or bring the list back as you
   first proposed?
