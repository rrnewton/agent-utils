"use strict";
// gent-talk voice page.
//
// The call flow, in order:
//
//   1. ask THIS server for a signed conversation URL, authenticated with the write-scope token;
//   2. open a WebSocket to that URL;
//   3. stream microphone audio up as base64 16 kHz PCM, and play the agent's audio back down.
//
// Four rules hold here, and each one is a failure this page is meant not to have:
//
// * No vendor script, no CDN, no framework, no build step. Everything below is plain browser API,
//   so the page cannot break because a third-party bundle moved, and it loads on a phone with a
//   bad connection. The app-like frame is CSS (see web/voice.css), not a runtime.
// * No silent degradation. If the mint fails, if the socket closes, or if the agent negotiates an
//   audio format this page cannot decode, the page SAYS SO. It never falls back to an unsigned
//   URL, and it never sits there looking connected while nothing works.
// * Agent text and channel text are UNTRUSTED. Nothing from either is ever assigned as markup.
//   Every visible fragment is built as an element whose text is set with textContent.
// * The interface does not imply continuity the session does not have. See `noteConversationEnded`
//   and `clearView`: a hang-up loses the agent's context and says so, and clearing the screen is
//   labelled and described as clearing the screen.

const TOKEN_KEY = "gent-talk.token"; // shared with the main app on purpose.
const MIC_SETTINGS_KEY = "gent-talk.voice.mic";

const el = (id) => document.getElementById(id);
const setStatus = (text) => {
  el("status").textContent = redact(text);
};
const setState = (text) => {
  el("conversation-state").textContent = text;
};

// Nothing this page displays may contain a credential.
//
// The server redacts its own secrets before answering, but this page holds one the server has
// never seen: the API token in this browser. An error message is assembled from whatever came
// back, and a server — or something in between it and here — is free to echo a request back
// verbatim. So the last thing before any text reaches the DOM is this: the token, if it is in
// there, is replaced. It is one line, it costs nothing, and it removes a whole class of "the
// error message leaked the key" from being possible at all.
function redact(text) {
  let out = String(text);
  const secrets = [localStorage.getItem(TOKEN_KEY) || "", el("api-token").value || ""];
  for (const secret of secrets) {
    // Short strings are not credentials, and blanket-replacing one would mangle ordinary words.
    if (secret.length >= 8) {
      out = out.split(secret).join("[redacted]");
    }
  }
  return out;
}

// The whole point of an earlier pass: a failure is SHOWN, in the page, in words the owner can act
// on. The reported bug was that a 502 naming a missing API-key permission appeared only in the dev
// console, so the only visible symptom was a page that did nothing. The panel lives outside the
// three screens so it is visible on whichever one is up.
function showError(text) {
  const box = el("error");
  box.textContent = redact(text);
  box.hidden = false;
  setState("error");
}

function clearError() {
  const box = el("error");
  box.textContent = "";
  box.hidden = true;
}

const session = {
  socket: null,
  connected: false, // true only between `onopen` and teardown.
  muted: false,
  audio: null, // AudioContext
  stream: null, // MediaStream
  node: null, // ScriptProcessorNode capturing the microphone
  source: null,
  playAt: 0, // next start time on the audio clock
  playing: [], // scheduled AudioBufferSourceNodes, so an interruption can cancel them
  outputRate: 16000,
};

const token = () => localStorage.getItem(TOKEN_KEY) || "";

// --- screens ----------------------------------------------------------------------------------
//
// Three screens, no router and no framework: they are three sections stacked in one grid cell and
// `hidden` picks. The explanatory text that earns no space once you know what the page is lives on
// the sign-in screen; the connection details and the knobs live in settings; the main screen is
// the transcript and the two controls, and nothing else.

const SCREENS = ["signin", "main", "settings"];
let screenBeforeSettings = "signin";
let currentScreen = "signin";

function showScreen(name) {
  for (const screen of SCREENS) {
    el(`screen-${screen}`).hidden = screen !== name;
  }
  const main = name === "main";
  // The top-row controls belong to the main screen; on the other two they would act on something
  // that is not on the screen.
  el("tabs").hidden = !main;
  el("clear-view").hidden = !main;
  el("open-settings").hidden = name === "settings";
  // The control pane is a grid ROW. Hiding it collapses the row, so the body grows to fill the
  // frame rather than leaving a band of empty pane under a sign-in form.
  el("control-pane").hidden = !main;
  if (name !== "settings") {
    screenBeforeSettings = name;
  }
  currentScreen = name;
}

let currentTab = "voice";

function showTab(name) {
  // Deliberately touches NOTHING in `session`. Switching tabs during a call must not disturb the
  // call: no socket close, no track stop, no re-acquire, no change to the mute state.
  currentTab = name;
  el("pane-voice").hidden = name !== "voice";
  el("pane-discord").hidden = name !== "discord";
  el("tab-voice").className = name === "voice" ? "tab-button active" : "tab-button";
  el("tab-discord").className = name === "discord" ? "tab-button active" : "tab-button";
}

// --- connection details -------------------------------------------------------------------------
//
// Shown once in a banner that takes itself away, and permanently reachable from settings. Kept in
// a variable rather than read back out of the DOM, so appending to it cannot pick up a redaction
// marker and re-redact it.

const BANNER_DISMISS_MS = 8000;
let connectionDetail = "";
let bannerTimer = null;

function renderDetail() {
  const text = redact(connectionDetail);
  el("connection-detail").textContent = text;
  el("settings-detail").textContent = text;
}

function showDetail(text) {
  connectionDetail = String(text);
  renderDetail();
  el("connection-banner").hidden = false;
  if (bannerTimer !== null) {
    clearTimeout(bannerTimer);
  }
  bannerTimer = setTimeout(dismissBanner, BANNER_DISMISS_MS);
}

function addDetail(text) {
  connectionDetail = `${connectionDetail} · ${text}`;
  renderDetail();
}

function dismissBanner() {
  if (bannerTimer !== null) {
    clearTimeout(bannerTimer);
    bannerTimer = null;
  }
  el("connection-banner").hidden = true;
}

// --- transcript -----------------------------------------------------------------------------

/** Keep the newest line directly above the control pane, which is where the eye already is. */
function scrollToNewest() {
  const area = el("scroll-area");
  area.scrollTop = area.scrollHeight;
}

function line(who, text) {
  const li = document.createElement("li");
  const meta = document.createElement("div");
  meta.className = "meta";
  const author = document.createElement("span");
  author.textContent = who;
  meta.append(author);
  const body = document.createElement("div");
  body.className = "body";
  body.textContent = text; // untrusted text: never innerHTML.
  li.append(meta, body);
  el("transcript").append(li);
  scrollToNewest();
  return li;
}

/**
 * A line the PAGE wrote, not something anybody said.
 *
 * These exist because a single unbroken list of messages is itself a claim — that it is all one
 * conversation — and that claim is sometimes false. Rendered visibly differently so it cannot be
 * mistaken for a turn.
 */
function systemLine(text) {
  const li = document.createElement("li");
  li.className = "system";
  const body = document.createElement("div");
  body.className = "body";
  body.textContent = text;
  li.append(body);
  el("transcript").append(li);
  scrollToNewest();
  return li;
}

// True from the moment a conversation opens until the end has been announced in the transcript.
let conversationOpen = false;

/**
 * Say, in the transcript itself, that the conversation ended.
 *
 * This is the honesty fix for the hang-up behaviour. ElevenLabs' Conversational AI WebSocket has
 * no documented way to resume a conversation after the socket closes: the initiation message and
 * the signed-URL endpoint both take an `agent_id` and neither accepts a `conversation_id`, and the
 * REST conversations API is transcript retrieval, not resumption. (Checked 2026-08-19 against the
 * vendor's Agent WebSocket API reference and get-signed-url reference. The docs do not say resume
 * is impossible; they provide no field through which it could be requested.) So the honest thing
 * is to mark the break rather than to promise a resume, and mute — which never closes the socket —
 * is the control that actually preserves context.
 */
function noteConversationEnded() {
  if (!conversationOpen) {
    return;
  }
  conversationOpen = false;
  systemLine(
    "Call ended. The agent does NOT carry this conversation's context into the next call — the " +
      "vendor documents no way to resume a conversation once the connection closes. The lines " +
      "above stay on screen, but anything below this point is a NEW conversation that has never " +
      "seen them. To pause without losing context, use Mute instead of Hang up."
  );
}

/**
 * Clear the SCREEN. Nothing else.
 *
 * The ambiguous middle — the screen empties and the agent carries on with everything the operator
 * thought they had removed — is the one outcome this must not have. So the control is labelled
 * "Clear view", and when a call is live the very first thing back on the empty screen is a line
 * saying the agent still remembers. Ending the conversation is a different and heavier action, and
 * it has its own button.
 */
function clearView() {
  el("transcript").replaceChildren();
  el("discord-log").replaceChildren();
  if (session.socket) {
    systemLine(
      "View cleared. This cleared the screen only: the call is still open and the agent still " +
        "has everything said before this point in its context. Hang up to end the conversation."
    );
    setStatus("View cleared — the screen only. The agent has not forgotten anything.");
  } else {
    setStatus("View cleared.");
  }
}

// --- the server ------------------------------------------------------------------------------

/**
 * One request to gent-talk, with the token, and one error taxonomy out of it.
 *
 * The server answers with a real taxonomy — 503 `elevenlabs_not_configured` names the exact
 * setting that is missing, 502 `elevenlabs_error` carries the vendor's status and message,
 * including things only the vendor knows, such as an API key that lacks the `convai_write`
 * permission. Flattening that to "could not start" would throw away the only sentence that says
 * what to fix, so all of it is passed through.
 */
async function api(path) {
  const response = await fetch(path, { headers: { Authorization: `Bearer ${token()}` } });
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch (_error) {
    throw new Error(`gent-talk returned non-JSON (HTTP ${response.status})`);
  }
  if (!response.ok) {
    const detail = payload && payload.detail ? payload.detail : "(no detail)";
    const code = payload && payload.error ? payload.error : "error";
    const error = new Error(`HTTP ${response.status} ${code}: ${detail}`);
    // Only these two mean "your token is wrong". Everything else means the server is unhappy for
    // a reason that signing in again will not fix, and bouncing the owner back to the sign-in
    // screen for those would be a lie about which thing is broken.
    error.refused = response.status === 401 || response.status === 403;
    throw error;
  }
  return payload;
}

async function mintSignedUrl() {
  if (!token()) {
    throw new Error("no API token saved on this phone yet");
  }
  const payload = await api("/api/v1/signed-url");
  if (!payload || !payload.signed_url) {
    throw new Error("gent-talk answered without a signed URL");
  }
  return payload;
}

// --- audio helpers ---------------------------------------------------------------------------

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function bytesToBase64(bytes) {
  let binary = "";
  const chunk = 0x8000; // apply() has an argument-count limit; chunk to stay under it.
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// Linear resample to 16 kHz, which is the input format the agent expects. Doing this explicitly
// beats asking for an AudioContext at 16 kHz and hoping: a browser is allowed to give you a
// different rate, and the failure is silent and sounds like a chipmunk.
function downsampleTo16k(input, inputRate) {
  if (inputRate === 16000) {
    return input;
  }
  const ratio = inputRate / 16000;
  const out = new Float32Array(Math.floor(input.length / ratio));
  for (let i = 0; i < out.length; i += 1) {
    const at = i * ratio;
    const low = Math.floor(at);
    const high = Math.min(low + 1, input.length - 1);
    const frac = at - low;
    out[i] = input[low] * (1 - frac) + input[high] * frac;
  }
  return out;
}

function floatToPcm16(samples) {
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    out[i] = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
  }
  return out;
}

// "pcm_16000" -> 16000. Anything else (ulaw, mp3) is refused rather than played as noise.
function outputRateFrom(format) {
  if (!format) {
    return 16000; // the documented default for this websocket.
  }
  const match = /^pcm_(\d+)$/.exec(format);
  if (!match) {
    throw new Error(
      `the agent negotiated audio format "${format}", which this page cannot decode. ` +
        "Set the agent's output format to PCM (pcm_16000) in the ElevenLabs dashboard."
    );
  }
  return Number(match[1]);
}

function playPcm(b64) {
  const bytes = base64ToBytes(b64);
  const pcm = new Int16Array(bytes.buffer, bytes.byteOffset, Math.floor(bytes.byteLength / 2));
  const buffer = session.audio.createBuffer(1, pcm.length, session.outputRate);
  const channel = buffer.getChannelData(0);
  for (let i = 0; i < pcm.length; i += 1) {
    channel[i] = pcm[i] / 0x8000;
  }
  const node = session.audio.createBufferSource();
  node.buffer = buffer;
  node.connect(session.audio.destination);
  const now = session.audio.currentTime;
  session.playAt = Math.max(session.playAt, now + 0.05);
  node.start(session.playAt);
  session.playAt += buffer.duration;
  session.playing.push(node);
  node.onended = () => {
    session.playing = session.playing.filter((n) => n !== node);
  };
}

function stopPlayback() {
  for (const node of session.playing) {
    try {
      node.stop();
    } catch (_error) {
      // already finished; nothing to do.
    }
  }
  session.playing = [];
  session.playAt = 0;
}

// --- microphone settings ------------------------------------------------------------------------

// Three knobs on the audio the browser hands us. They are exposed because each one has a failure
// mode the owner has actually hit, and none of them is guessable from the outside: the agent
// answering its own voice on speakerphone, background noise reaching the transcriber, and — the
// awkward one — automatic gain amplifying a silent room until a phone being set down crosses the
// speech threshold and gets transcribed as a word.
const MIC_TOGGLES = [
  ["mic-echo-cancellation", "echoCancellation"],
  ["mic-noise-suppression", "noiseSuppression"],
  ["mic-auto-gain", "autoGainControl"],
];

const DEFAULT_MIC_SETTINGS = {
  echoCancellation: true, // stated explicitly today.
  noiseSuppression: true, // stated explicitly today.
  autoGainControl: true, // NOT stated today; see `audioConstraints`.
};

/** What the checkboxes say right now. The DOM is the live truth; storage only carries it across reloads. */
function micSettings() {
  const settings = {};
  for (const [id, key] of MIC_TOGGLES) {
    settings[key] = el(id).checked === true;
  }
  return settings;
}

/** What was stored, defaulted per key. A corrupt or partial entry falls back to today's behaviour. */
function storedMicSettings() {
  const settings = { ...DEFAULT_MIC_SETTINGS };
  let stored = null;
  try {
    stored = JSON.parse(localStorage.getItem(MIC_SETTINGS_KEY) || "null");
  } catch (_error) {
    stored = null;
  }
  if (stored && typeof stored === "object") {
    for (const key of Object.keys(settings)) {
      if (typeof stored[key] === "boolean") {
        settings[key] = stored[key];
      }
    }
  }
  return settings;
}

// The constraint object handed to getUserMedia.
//
// Echo cancellation and noise suppression are already stated explicitly, so a toggle simply
// supplies the value that was hard-coded.
//
// Automatic gain control is the careful one. It is NOT in the constraint object at all by default,
// so whatever the browser's own default is, is what applies — in practice on, but the spec makes
// that the implementation's choice, not a promise. Passing `autoGainControl: true` would convert an
// implicit default into an explicit request, and those are not guaranteed to be the same thing on
// every browser. So the ON state — the default — says nothing at all, and only the OFF state is
// stated. With every toggle in its default position this returns the byte-for-byte constraint
// object the page has always sent.
function audioConstraints(settings) {
  const audio = {
    channelCount: 1,
    echoCancellation: settings.echoCancellation,
    noiseSuppression: settings.noiseSuppression,
  };
  if (!settings.autoGainControl) {
    audio.autoGainControl = false;
  }
  return { audio };
}

function persistMicSettings(settings) {
  const encoded = JSON.stringify(settings);
  try {
    localStorage.setItem(MIC_SETTINGS_KEY, encoded);
  } catch (_error) {
    return false;
  }
  // Read it back rather than assuming, exactly as saving the token does: private browsing accepts
  // setItem and stores nothing, and "saved" would then be a lie found out only after a reload.
  return localStorage.getItem(MIC_SETTINGS_KEY) === encoded;
}

// A toggle flipped mid-call does NOT reach the open microphone — the constraints were read when
// the stream opened. Saying "saved" and stopping there would leave a control that looks like it
// did something; so the line under the toggles states which call the change lands on.
function micSettingsChanged() {
  const saved = persistMicSettings(micSettings());
  const when = session.stream
    ? "The microphone is already open, so this does NOT change the call in progress — hang up " +
      "and start again to apply it."
    : "It applies the next time you start talking.";
  el("mic-settings-state").textContent = saved
    ? `Saved. ${when}`
    : "This browser refused to store the setting, so it will be forgotten when you reload " +
      `(private browsing does this). ${when}`;
}

// --- the call ----------------------------------------------------------------------------------

async function start() {
  if (session.socket) {
    setStatus("already connected");
    return;
  }
  clearError();
  setState("minting…");
  setStatus("asking gent-talk for a signed URL…");
  const minted = await mintSignedUrl();
  showDetail(
    `agent ${minted.agent_id}; signed URL valid for about ${Math.round(
      (minted.valid_for_seconds || 900) / 60
    )} minutes`
  );

  setStatus("asking for the microphone…");
  // Read HERE, at the moment the stream opens — which is why a toggle flipped mid-call cannot
  // reach this one, and why `micSettingsChanged` says so out loud.
  session.stream = await navigator.mediaDevices.getUserMedia(audioConstraints(micSettings()));
  session.audio = new (window.AudioContext || window.webkitAudioContext)();
  await session.audio.resume(); // iOS starts it suspended until a gesture.

  setState("connecting…");
  const socket = new WebSocket(minted.signed_url);
  session.socket = socket;
  session.muted = false;
  renderTalk();

  socket.onopen = () => {
    socket.send(JSON.stringify({ type: "conversation_initiation_client_data" }));
    session.connected = true;
    conversationOpen = true;
    setState("connected");
    setStatus("Connected — say something.");
    renderTalk();
    startCapture(socket);
  };

  socket.onmessage = (event) => {
    let message = null;
    try {
      message = JSON.parse(event.data);
    } catch (_error) {
      return; // a frame we do not understand is not a reason to tear the call down.
    }
    handle(socket, message);
  };

  socket.onerror = () => {
    // Not "see the console": the console is exactly where this used to die.
    showError(
      "The connection to the voice agent failed. The signed URL was minted, so gent-talk and " +
        "your token are fine; the failure is between this browser and ElevenLabs."
    );
    setStatus("websocket error");
  };

  socket.onclose = (event) => {
    // A signed-URL failure shows up HERE, as an immediate close, so say the code out loud rather
    // than leaving the page looking idle for no visible reason.
    setState("closed");
    setStatus(
      `conversation closed (code ${event.code}${event.reason ? `: ${event.reason}` : ""})`
    );
    teardown();
    noteConversationEnded();
  };
}

function handle(socket, message) {
  switch (message.type) {
    case "conversation_initiation_metadata": {
      const meta = message.conversation_initiation_metadata_event || {};
      session.outputRate = outputRateFrom(meta.agent_output_audio_format);
      addDetail(
        `conversation ${meta.conversation_id || "?"} · agent audio ${
          meta.agent_output_audio_format || "pcm_16000"
        }`
      );
      break;
    }
    case "audio":
      playPcm((message.audio_event || {}).audio_base_64);
      break;
    case "agent_response":
      line("agent", (message.agent_response_event || {}).agent_response || "");
      break;
    case "user_transcript":
      line("you", (message.user_transcription_event || {}).user_transcript || "");
      break;
    case "interruption":
      stopPlayback();
      break;
    case "ping":
      socket.send(
        JSON.stringify({ type: "pong", event_id: (message.ping_event || {}).event_id })
      );
      break;
    default:
      break;
  }
}

function startCapture(socket) {
  const rate = session.audio.sampleRate;
  // ScriptProcessorNode is deprecated in favour of AudioWorklet, and is still the only capture
  // path that needs no separate module file. A worklet would mean a second asset or a blob URL,
  // for no behavioural gain on a page this small.
  session.source = session.audio.createMediaStreamSource(session.stream);
  session.node = session.audio.createScriptProcessor(4096, 1, 1);
  session.node.onaudioprocess = (event) => {
    if (socket.readyState !== WebSocket.OPEN) {
      return;
    }
    // MUTE LIVES HERE, and nowhere else.
    //
    // It withholds frames. It does not stop the microphone track, it does not close the socket,
    // and it does not tear down the audio graph. That is the entire mechanism, and it is what the
    // control is FOR:
    //
    //   * The conversation stays open, so the AGENT KEEPS ITS CONTEXT. This is the whole reason
    //     mute exists rather than a hang-up-and-redial: hanging up loses the context, and the
    //     vendor documents no way to resume a conversation once the socket has closed. Mute is
    //     therefore the only pause this page can offer that the agent survives.
    //   * Nothing is torn down, so nothing has to be rebuilt. Stopping the track and re-acquiring
    //     it would rebuild the capture graph in the middle of a conversation, which is disruptive
    //     to no purpose when the goal is simply to stop being heard for a while.
    //
    // Anyone tidying `stop()` later: `track.stop()` MUST NOT become reachable from here, not even
    // conditionally. It ends the conversation, and with it everything the agent knows.
    if (session.muted) {
      return;
    }
    const mono = event.inputBuffer.getChannelData(0);
    const pcm = floatToPcm16(downsampleTo16k(mono, rate));
    socket.send(
      JSON.stringify({
        user_audio_chunk: bytesToBase64(new Uint8Array(pcm.buffer)),
      })
    );
  };
  session.source.connect(session.node);
  // A ScriptProcessorNode only fires while it is connected to the graph. Routing it at zero gain
  // keeps it running without echoing the microphone into the speaker.
  const silence = session.audio.createGain();
  silence.gain.value = 0;
  session.node.connect(silence);
  silence.connect(session.audio.destination);
}

function teardown() {
  stopPlayback();
  if (session.node) {
    session.node.disconnect();
    session.node.onaudioprocess = null;
  }
  if (session.source) {
    session.source.disconnect();
  }
  if (session.stream) {
    // Hang up — and ONLY hang up — releases the microphone. `teardown()` runs when the
    // conversation is over, and the conversation being over is precisely what makes releasing the
    // stream correct here. Mute must never reach this line: it would end the call and lose the
    // agent's context, which is the one thing mute exists to preserve.
    for (const track of session.stream.getTracks()) {
      track.stop();
    }
  }
  if (session.audio) {
    session.audio.close();
  }
  session.node = null;
  session.source = null;
  session.stream = null;
  session.audio = null;
  session.socket = null;
  session.connected = false;
  session.muted = false;
  renderTalk();
}

function stop() {
  if (session.socket) {
    session.socket.close();
    setStatus("Hung up.");
  } else {
    setStatus("Not connected.");
  }
  setState("idle");
  teardown();
  noteConversationEnded();
}

// --- the two controls ---------------------------------------------------------------------------

function renderTalk() {
  const talk = el("talk");
  const label = el("talk-label");
  if (!session.socket) {
    talk.className = "control control-talk";
    label.textContent = "Talk";
    return;
  }
  if (!session.connected) {
    talk.className = "control control-talk";
    label.textContent = "Connecting…";
    return;
  }
  if (session.muted) {
    // The slash across the microphone comes from this class; see web/voice.css.
    talk.className = "control control-talk muted";
    label.textContent = "Muted";
    return;
  }
  // `live` is what runs the pulse animation.
  talk.className = "control control-talk live";
  label.textContent = "Listening";
}

function setMuted(muted) {
  session.muted = muted;
  renderTalk();
  setStatus(
    muted
      ? "Muted. The call is still open and the agent still has everything said so far; it just " +
          "cannot hear you. Your phone still shows the microphone as in use, because it really " +
          "is still open — unmute and it hears you again mid-conversation."
      : "Listening — say something."
  );
}

/** The one large control: start a call when idle, toggle mute when one is live. */
function onTalk() {
  if (!session.socket) {
    return guard(start)();
  }
  setMuted(!session.muted);
  return Promise.resolve();
}

function guard(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      // Both places: the panel is what the owner reads, the status line is the one-liner. Neither
      // is the console.
      showError(error.message);
      setStatus("could not start the conversation");
      teardown();
    });
}

// --- raw Discord, rendered ------------------------------------------------------------------
//
// The value of this view is being able to point at a specific real message — the agent has
// described messages that did not exist. So every line carries its author and its message id.
//
// Channel text is third-party data written by whoever is in the channel. NOTHING below turns it
// into markup: there is no innerHTML, no insertAdjacentHTML, no template string that becomes a
// document. Every fragment of a message becomes an element created HERE whose text is assigned
// with textContent, which is escaping by construction rather than escaping by remembering. The
// markdown subset is deliberately small, and the only sink that is not plain text — a link's href —
// is scheme-checked before anything is written to it.

/** Only these become clickable. Everything else is shown as the text it is. */
const SAFE_LINK = /^https?:\/\//i;

/**
 * One inline construct.
 *
 * Groups, in the order they are tried: code span, bold, strikethrough, italic (asterisk), italic
 * (underscore), link, user mention, channel mention. Code is first so that backticked text is
 * taken verbatim; a construct is not parsed across a line break.
 */
const INLINE =
  /`([^`\n]+)`|\*\*([^*\n]+)\*\*|~~([^~\n]+)~~|\*([^*\n]+)\*|_([^_\n]+)_|\[([^\]\n]*)\]\(([^)\s]*)\)|<@!?(\d+)>|<#(\d+)>/g;

function styled(tag, className, text) {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  node.textContent = text;
  return node;
}

/**
 * A run of plain message text.
 *
 * A span rather than a text node so that every fragment is built the same way, through the one
 * function that assigns textContent — there is no second path to audit.
 */
function plain(text) {
  return styled("span", "", text);
}

function mdLink(label, href) {
  // A URL is a sink. `javascript:` and `data:` execute; a scheme-less string resolves against this
  // origin and can be made to look like somewhere else entirely. None of those becomes a tappable
  // link — the message is shown with its URL as visible text instead, which is strictly more
  // informative than a link the operator cannot inspect on a phone.
  if (!SAFE_LINK.test(href)) {
    return plain(`${label} (${href})`);
  }
  const anchor = document.createElement("a");
  anchor.textContent = label;
  anchor.setAttribute("href", href);
  anchor.setAttribute("rel", "noopener noreferrer nofollow");
  anchor.setAttribute("target", "_blank");
  return anchor;
}

function renderInline(parent, text) {
  INLINE.lastIndex = 0;
  let at = 0;
  let match = INLINE.exec(text);
  while (match !== null) {
    if (match.index > at) {
      parent.append(plain(text.slice(at, match.index)));
    }
    at = match.index + match[0].length;
    if (match[1] !== undefined) {
      parent.append(styled("code", "md-code", match[1]));
    } else if (match[2] !== undefined) {
      parent.append(styled("strong", "", match[2]));
    } else if (match[3] !== undefined) {
      parent.append(styled("s", "", match[3]));
    } else if (match[4] !== undefined) {
      parent.append(styled("em", "", match[4]));
    } else if (match[5] !== undefined) {
      parent.append(styled("em", "", match[5]));
    } else if (match[6] !== undefined) {
      parent.append(mdLink(match[6], match[7]));
    } else if (match[8] !== undefined) {
      // Rendered as the id, not as a name: this page has no user directory, and inventing a
      // display name here is exactly the kind of thing this view exists to catch.
      parent.append(styled("span", "md-mention", `@${match[8]}`));
    } else if (match[9] !== undefined) {
      parent.append(styled("span", "md-mention", `#${match[9]}`));
    }
    match = INLINE.exec(text);
  }
  if (at < text.length) {
    parent.append(plain(text.slice(at)));
  }
}

const FENCE = /^\s*```/;
const QUOTE = /^\s*>\s?/;

function renderMarkdownInto(parent, raw) {
  const lines = String(raw === null || raw === undefined ? "" : raw).split("\n");
  let i = 0;
  while (i < lines.length) {
    if (FENCE.test(lines[i])) {
      const body = [];
      i += 1;
      while (i < lines.length && !FENCE.test(lines[i])) {
        body.push(lines[i]);
        i += 1;
      }
      i += 1; // the closing fence, or the end of an unclosed block.
      parent.append(styled("pre", "md-pre", body.join("\n")));
      continue;
    }
    if (QUOTE.test(lines[i])) {
      const quote = document.createElement("div");
      quote.className = "md-quote";
      const body = [];
      while (i < lines.length && QUOTE.test(lines[i])) {
        body.push(lines[i].replace(QUOTE, ""));
        i += 1;
      }
      renderInline(quote, body.join("\n"));
      parent.append(quote);
      continue;
    }
    const paragraph = document.createElement("div");
    renderInline(paragraph, lines[i]);
    parent.append(paragraph);
    i += 1;
  }
  return parent;
}

function discordNode(message) {
  const li = document.createElement("li");
  const meta = document.createElement("div");
  meta.className = "meta";
  const author = document.createElement("span");
  // An author name is channel data too, and a display name can be anything at all.
  author.textContent = message.author_is_bot ? `${message.author} (bot)` : String(message.author);
  const stamp = document.createElement("span");
  stamp.textContent = String(message.timestamp || "")
    .replace("T", " ")
    .slice(0, 16);
  const id = document.createElement("span");
  id.className = "msg-id";
  id.textContent = `id ${message.id}`;
  meta.append(author, stamp, id);
  const body = document.createElement("div");
  body.className = "body";
  renderMarkdownInto(body, message.content);
  li.append(meta, body);
  return li;
}

async function loadDiscord() {
  const channel = el("discord-channel").value;
  if (!channel) {
    setStatus("no channel to read — this server has none configured.");
    return;
  }
  setStatus("fetching the channel…");
  const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/messages`);
  const list = el("discord-log");
  list.replaceChildren();
  for (const message of payload.messages) {
    list.append(discordNode(message));
  }
  setStatus(`${payload.messages.length} message(s) from ${payload.channel.label}`);
  scrollToNewest();
}

// --- sign-in ---------------------------------------------------------------------------------

function setTokenState(text) {
  el("token-state").textContent = text;
}

function applyClientConfig(config) {
  const select = el("discord-channel");
  select.replaceChildren();
  for (const channel of config.channels || []) {
    const option = document.createElement("option");
    option.value = channel.id;
    option.textContent = channel.label;
    select.append(option);
  }
  if ((config.channels || []).length > 0) {
    select.value = config.channels[0].id;
  }
}

/**
 * Prove the saved token actually works, then show the main interface.
 *
 * A refusal (401/403) is the one failure that means "sign in again", and only that one sends the
 * owner back to the sign-in screen. Anything else — server down, network gone — leaves the token
 * alone and lands on the main screen with the error visible, because bouncing to a sign-in form
 * would blame the wrong thing.
 */
async function signIn() {
  if (!token()) {
    showScreen("signin");
    return false;
  }
  let config = null;
  try {
    config = await api("/api/v1/client-config");
  } catch (error) {
    if (error.refused) {
      showError(`This server refused that token: ${error.message}`);
      setStatus("that token was refused — paste the write-scope token");
      showScreen("signin");
      return false;
    }
    showError(`Signed in, but gent-talk did not answer: ${error.message}`);
    showScreen("main");
    return true;
  }
  applyClientConfig(config);
  clearError();
  showScreen("main");
  setStatus("Ready. Tap the microphone to start talking.");
  setState("idle");
  return true;
}

// A banner that says "saved" while the button looks untouched leaves two questions open at once:
// did the click register, and did the save work? So the button itself changes — immediately on
// click, and again on the result — and the line under it states what is stored right now.
const SAVE_LABEL = "Save token";
let saveRevert = null;

function markSave(label, className, disabled) {
  const button = el("save-token");
  button.textContent = label;
  button.className = className;
  button.disabled = disabled;
  if (saveRevert !== null) {
    clearTimeout(saveRevert);
    saveRevert = null;
  }
}

async function saveToken() {
  clearError();
  const value = el("api-token").value.trim();
  markSave("Saving…", "", true);
  if (!value) {
    markSave(SAVE_LABEL, "", false);
    setTokenState("no token saved in this browser");
    showError("There is nothing to save — paste your write-scope API token first.");
    setStatus("nothing to save");
    return;
  }
  localStorage.setItem(TOKEN_KEY, value);
  // Read it back rather than assuming: private browsing and a full quota both make setItem throw
  // or silently do nothing, and "saved" would then be a lie the owner only discovers later.
  if (localStorage.getItem(TOKEN_KEY) !== value) {
    markSave(SAVE_LABEL, "", false);
    setTokenState("no token saved in this browser");
    showError(
      "This browser refused to store the token. Private browsing and a full storage quota both " +
        "do this; the token was NOT saved."
    );
    return;
  }
  markSave("Saved ✓", "ok", false);
  setTokenState("token saved in this browser");
  setStatus("ready");
  saveRevert = setTimeout(() => {
    markSave(SAVE_LABEL, "", false);
  }, 2500);
  await signIn();
}

function forgetToken() {
  clearError();
  localStorage.removeItem(TOKEN_KEY);
  el("api-token").value = "";
  markSave(SAVE_LABEL, "", false);
  setTokenState("no token saved in this browser");
  setStatus("token forgotten");
  showScreen("signin");
}

// --- wiring ------------------------------------------------------------------------------------

el("api-token").value = token();
el("save-token").addEventListener("click", guardQuietly(saveToken));
el("forget-token").addEventListener("click", forgetToken);

// The checkboxes are the live truth `start()` reads, so they are what gets restored on load.
const restoredMicSettings = storedMicSettings();
for (const [id, key] of MIC_TOGGLES) {
  el(id).checked = restoredMicSettings[key];
  el(id).addEventListener("change", micSettingsChanged);
}

el("talk").addEventListener("click", onTalk);
el("hang-up").addEventListener("click", stop);
el("clear-view").addEventListener("click", clearView);
el("dismiss-banner").addEventListener("click", dismissBanner);
el("open-settings").addEventListener("click", () => showScreen("settings"));
el("close-settings").addEventListener("click", () => showScreen(screenBeforeSettings));
el("tab-voice").addEventListener("click", () => showTab("voice"));
el("tab-discord").addEventListener("click", () => {
  showTab("discord");
  if (el("discord-log").children.length === 0) {
    guardQuietly(loadDiscord)();
  }
});
el("refresh-discord").addEventListener("click", guardQuietly(loadDiscord));
el("discord-channel").addEventListener("change", guardQuietly(loadDiscord));

/**
 * Like `guard`, but for things that are not a call: it reports, and it does NOT tear down a live
 * conversation. Reading the channel list must never be able to hang up on the owner.
 */
function guardQuietly(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => {
      showError(error.message);
    });
}

setTokenState(token() ? "token saved in this browser" : "no token saved in this browser");
setStatus(token() ? "checking your token…" : "paste your write-scope API token first");
showTab("voice");
renderTalk();
// No token means no interface to show yet: the sign-in screen is the whole page until there is
// one. With a token, prove it before showing the main screen.
if (token()) {
  guardQuietly(signIn)();
} else {
  showScreen("signin");
}
