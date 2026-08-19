"use strict";
// gent-talk voice page.
//
// The whole flow, in order:
//
//   1. ask THIS server for a signed conversation URL, authenticated with the write-scope token;
//   2. open a WebSocket to that URL;
//   3. stream microphone audio up as base64 16 kHz PCM, and play the agent's audio back down.
//
// Three rules hold here, and each one is a failure this page is meant not to have:
//
// * No vendor script and no CDN. Everything below is plain browser API, so the page cannot break
//   because a third-party bundle moved, and it loads on a phone with a bad connection.
// * No silent degradation. If the mint fails, if the socket closes, or if the agent negotiates an
//   audio format this page cannot decode, the page SAYS SO. It never falls back to an unsigned
//   URL, and it never sits there looking connected while nothing works.
// * Agent text is untrusted, exactly as channel text is in app.js: it is inserted with
//   textContent, never innerHTML.

const TOKEN_KEY = "gent-talk.token"; // shared with the main app on purpose.

const el = (id) => document.getElementById(id);
const setStatus = (text) => {
  el("status").textContent = redact(text);
};
const setState = (text) => {
  el("conversation-state").textContent = text;
};
const setDetail = (text) => {
  el("detail").textContent = redact(text);
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

// The whole point of this pass: a failure is SHOWN, in the page, in words the owner can act on.
// The reported bug was that a 502 naming a missing API-key permission appeared only in the dev
// console, so the only visible symptom was a page that did nothing.
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
  audio: null, // AudioContext
  stream: null, // MediaStream
  node: null, // ScriptProcessorNode capturing the microphone
  source: null,
  playAt: 0, // next start time on the audio clock
  playing: [], // scheduled AudioBufferSourceNodes, so an interruption can cancel them
  outputRate: 16000,
};

const token = () => localStorage.getItem(TOKEN_KEY) || "";

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
  li.scrollIntoView({ block: "end" });
}

// --- step 1: mint ----------------------------------------------------------------------------

async function mintSignedUrl() {
  if (!token()) {
    throw new Error("no API token saved on this phone yet");
  }
  const response = await fetch("/api/v1/signed-url", {
    headers: { Authorization: `Bearer ${token()}` },
  });
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch (_error) {
    throw new Error(`gent-talk returned non-JSON (HTTP ${response.status})`);
  }
  if (!response.ok) {
    // Pass the server's own words through, all of them. gent-talk answers with a real taxonomy —
    // 503 `elevenlabs_not_configured` names the exact setting that is missing, 502
    // `elevenlabs_error` carries the vendor's status and message, including things only the
    // vendor knows, such as an API key that lacks the `convai_write` permission. Flattening that
    // to "could not start" would throw away the only sentence that says what to fix.
    const detail = payload && payload.detail ? payload.detail : "(no detail)";
    const code = payload && payload.error ? payload.error : "error";
    throw new Error(`HTTP ${response.status} ${code}: ${detail}`);
  }
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

// --- step 2 and 3: connect, stream -------------------------------------------------------------

async function start() {
  if (session.socket) {
    setStatus("already connected");
    return;
  }
  clearError();
  setState("minting…");
  setStatus("asking gent-talk for a signed URL…");
  const minted = await mintSignedUrl();
  setDetail(
    `agent ${minted.agent_id}; signed URL valid for about ${Math.round(
      (minted.valid_for_seconds || 900) / 60
    )} minutes`
  );

  setStatus("asking for the microphone…");
  session.stream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
  });
  session.audio = new (window.AudioContext || window.webkitAudioContext)();
  await session.audio.resume(); // iOS starts it suspended until a gesture.

  setState("connecting…");
  const socket = new WebSocket(minted.signed_url);
  session.socket = socket;

  socket.onopen = () => {
    socket.send(JSON.stringify({ type: "conversation_initiation_client_data" }));
    setState("connected");
    setStatus("connected — say something");
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
  };
}

function handle(socket, message) {
  switch (message.type) {
    case "conversation_initiation_metadata": {
      const meta = message.conversation_initiation_metadata_event || {};
      session.outputRate = outputRateFrom(meta.agent_output_audio_format);
      setDetail(
        `${el("detail").textContent} · conversation ${meta.conversation_id || "?"} · agent audio ${
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
  const mute = session.audio.createGain();
  mute.gain.value = 0;
  session.node.connect(mute);
  mute.connect(session.audio.destination);
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
}

function stop() {
  if (session.socket) {
    session.socket.close();
    setStatus("hung up");
  } else {
    setStatus("not connected");
  }
  setState("idle");
  teardown();
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

// --- token, with a button that visibly reacts ---------------------------------------------------

// A banner that says "saved" while the button looks untouched leaves two questions open at once:
// did the click register, and did the save work? So the button itself changes — immediately on
// click, and again on the result — and the line under it states what is stored right now.
const SAVE_LABEL = "Save token";
let saveRevert = null;

function setTokenState(text) {
  el("token-state").textContent = text;
}

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

function saveToken() {
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
}

el("api-token").value = token();
el("save-token").addEventListener("click", saveToken);
el("forget-token").addEventListener("click", () => {
  clearError();
  localStorage.removeItem(TOKEN_KEY);
  el("api-token").value = "";
  markSave(SAVE_LABEL, "", false);
  setTokenState("no token saved in this browser");
  setStatus("token forgotten");
});
el("start").addEventListener("click", guard(start));
el("stop").addEventListener("click", stop);
setTokenState(token() ? "token saved in this browser" : "no token saved in this browser");
setStatus(token() ? "ready" : "paste your write-scope API token first");
