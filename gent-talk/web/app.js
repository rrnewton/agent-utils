"use strict";
// gent-talk phone client.
//
// Two rules hold everywhere in this file:
//
// 1. Channel text is UNTRUSTED. It is inserted with textContent, never innerHTML, so a message
//    can never become markup, script, or a link the owner did not intend to tap. There is no
//    markdown rendering here on purpose.
// 2. The API token lives only in this browser's localStorage and is sent as a bearer token. It is
//    never put in a URL, so it cannot leak through a referrer or a server log.

const TOKEN_KEY = "gent-talk.token";
const state = {
  channels: [],
  digest: [],
  agentId: null,
};

const el = (id) => document.getElementById(id);
const setStatus = (text) => {
  el("status").textContent = text;
};

function token() {
  return localStorage.getItem(TOKEN_KEY) || "";
}

async function api(path, options = {}) {
  const headers = Object.assign({ Authorization: `Bearer ${token()}` }, options.headers || {});
  if (options.body !== undefined) {
    headers["Content-Type"] = "application/json";
  }
  const response = await fetch(path, Object.assign({}, options, { headers }));
  const text = await response.text();
  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch (_error) {
    throw new Error(`server returned non-JSON (HTTP ${response.status})`);
  }
  if (!response.ok) {
    const detail = payload && payload.detail ? payload.detail : `HTTP ${response.status}`;
    throw new Error(detail);
  }
  return payload;
}

// How many messages there are, or an honest refusal to say.
//
// The length of what the server returned is the FETCH WINDOW, not a channel total. Discord gives
// a bot no message count for a guild text channel, so the number is the channel's own only when
// the server reports `complete` -- the fetch came back short, meaning there is nothing older.
// Otherwise no digit is shown at all: a confidently wrong count is worse than no count, and this
// one was wrong in the direction that makes the bridge look like it is losing messages.
//
// `!== true` rather than `=== false`, so a server too old to send the field is treated as unknown.
function messageCount(count, complete) {
  if (complete !== true) {
    return "the most recent messages — older ones are not loaded";
  }
  if (count === 0) {
    return "no messages";
  }
  return `${count} message${count === 1 ? "" : "s"}`;
}

function messageNode(message, opts = {}) {
  const li = document.createElement("li");
  const meta = document.createElement("div");
  meta.className = "meta";

  const author = document.createElement("span");
  author.textContent = message.author;
  const stamp = document.createElement("span");
  // `#52 operator-timezone`. The server converts once, into the operator's configured zone, and
  // hands back a string that is already correct — so prefer it. The ISO slice below is the
  // fallback for a server too old to send `spoken_time`, and it is UTC-as-Discord-reported-it,
  // which is exactly the value the phone and the voice agent must not disagree about.
  stamp.textContent =
    message.spoken_time || (message.timestamp || "").replace("T", " ").slice(0, 16);
  meta.append(author, stamp);

  const body = document.createElement("div");
  body.className = "body";
  body.textContent = opts.summary !== undefined ? opts.summary : message.content;

  li.append(meta, body);
  if (opts.onSelect) {
    li.addEventListener("click", () => opts.onSelect(message));
  }
  return li;
}

function fillChannelSelects() {
  for (const id of ["talk-channel", "text-channel"]) {
    const select = el(id);
    const previous = select.value;
    select.replaceChildren();
    for (const channel of state.channels) {
      const option = document.createElement("option");
      option.value = channel.id;
      option.textContent = channel.writable ? `${channel.label} (postable)` : channel.label;
      select.append(option);
    }
    if (previous) {
      select.value = previous;
    }
  }
}

async function loadConfig() {
  if (!token()) {
    setStatus("no API token yet — open Settings");
    return;
  }
  const config = await api("/api/v1/client-config");
  state.channels = config.channels;
  state.agentId = config.elevenlabs_agent_id;
  fillChannelSelects();
  el("server-info").textContent =
    `server version ${config.version}; ` +
    `${config.channels.length} channel${config.channels.length === 1 ? "" : "s"}; ` +
    `voice agent ${config.elevenlabs_agent_id ? "configured" : "not configured"}`;
  mountVoiceAgent();
  setStatus("connected");
}

// The seam. When an ElevenLabs agent id is configured, mount the vendor's hosted widget; the agent
// itself reaches this server's API over the network, using its own credential. When it is not
// configured, say so plainly rather than pretending there is a voice path.
function mountVoiceAgent() {
  const host = el("voice-agent");
  host.replaceChildren();
  if (!state.agentId) {
    const note = document.createElement("p");
    note.className = "muted";
    note.textContent =
      "Voice agent not configured. Set elevenlabs.agent_id on the server to mount the hosted " +
      "widget here. Until then, 'Read the digest aloud' uses this phone's own speech engine.";
    host.append(note);
    return;
  }
  const widget = document.createElement("elevenlabs-convai");
  widget.setAttribute("agent-id", state.agentId);
  const script = document.createElement("script");
  script.src = "https://unpkg.com/@elevenlabs/convai-widget-embed";
  script.async = true;
  script.type = "text/javascript";
  host.append(widget, script);
}

async function loadDigest() {
  const channel = el("talk-channel").value;
  if (!channel) {
    return;
  }
  setStatus("fetching…");
  const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/digest`);
  state.digest = payload.entries;
  const list = el("digest");
  list.replaceChildren();
  for (const entry of payload.entries) {
    list.append(
      messageNode(
        {
          author: entry.author,
          timestamp: entry.timestamp,
          spoken_time: entry.spoken_time,
          content: entry.summary,
        },
        { onSelect: () => readFull(channel, entry.id) }
      )
    );
  }
  el("channel-name").textContent = payload.channel.label;
  setStatus(messageCount(payload.entries.length, payload.complete));
}

async function readFull(channel, messageId) {
  const payload = await api(
    `/api/v1/channels/${encodeURIComponent(channel)}/messages/${encodeURIComponent(messageId)}`
  );
  showResult(payload.message, null);
  speak(`${payload.message.author} said: ${payload.message.content}`);
}

function showResult(message, note) {
  const host = el("find-result");
  host.replaceChildren();
  if (note) {
    const warn = document.createElement("p");
    warn.className = "ambiguous";
    warn.textContent = note;
    host.append(warn);
  }
  if (message) {
    const list = document.createElement("ol");
    list.className = "messages";
    list.append(messageNode(message));
    host.append(list);
  }
}

async function findMessage(event) {
  event.preventDefault();
  const channel = el("talk-channel").value;
  const query = el("find-query").value.trim();
  if (!channel || !query) {
    return;
  }
  setStatus("searching…");
  const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/resolve`, {
    method: "POST",
    body: JSON.stringify({ query }),
  });
  if (!payload.best) {
    showResult(null, `nothing in the last ${payload.searched} messages matched that.`);
    setStatus("no match");
    speak("Nothing in the recent messages matched that.");
    return;
  }
  const note = payload.ambiguous
    ? "More than one message fits that description; this is the closest."
    : null;
  showResult(payload.best.message, note);
  speak(`${payload.best.message.author} said: ${payload.best.message.content}`);
  setStatus(`matched on: ${payload.best.matched_terms.join(", ")}`);
}

async function loadScrollback() {
  const channel = el("text-channel").value;
  if (!channel) {
    return;
  }
  setStatus("fetching…");
  const payload = await api(`/api/v1/channels/${encodeURIComponent(channel)}/messages`);
  const list = el("scrollback");
  list.replaceChildren();
  for (const message of payload.messages) {
    list.append(messageNode(message));
  }
  // The header names the channel you are looking at. Only the digest path used to set it, so
  // arriving on the Text tab first left the header reading "not connected" over a full,
  // freshly-fetched scrollback -- the one place the app lied about its own state.
  el("channel-name").textContent = payload.channel.label;
  setStatus(messageCount(payload.messages.length, payload.complete));
  list.lastElementChild?.scrollIntoView({ block: "end" });
}

// Local speech: no vendor, no key, no cost. It is the fallback while the hosted agent is unwired,
// and it is also the fastest way to sanity-check that a digest is actually speakable.
function speak(text) {
  if (!("speechSynthesis" in window)) {
    setStatus("this browser has no speech engine");
    return;
  }
  window.speechSynthesis.cancel();
  window.speechSynthesis.speak(new SpeechSynthesisUtterance(text));
}

function speakDigest() {
  if (state.digest.length === 0) {
    speak("Nothing fetched yet.");
    return;
  }
  const lines = state.digest.map((entry) => `${entry.author}: ${entry.summary}`);
  speak(`${state.digest.length} recent messages. ${lines.join(". ")}`);
}

function guard(fn) {
  return (...args) =>
    Promise.resolve(fn(...args)).catch((error) => setStatus(`error: ${error.message}`));
}

function wire() {
  for (const button of document.querySelectorAll("nav button")) {
    button.addEventListener("click", () => {
      for (const other of document.querySelectorAll("nav button")) {
        other.classList.toggle("active", other === button);
      }
      for (const section of document.querySelectorAll(".tab")) {
        section.classList.toggle("active", section.id === `tab-${button.dataset.tab}`);
      }
    });
  }

  el("refresh-digest").addEventListener("click", guard(loadDigest));
  el("refresh-text").addEventListener("click", guard(loadScrollback));
  el("find-form").addEventListener("submit", guard(findMessage));
  el("speak-digest").addEventListener("click", speakDigest);
  el("stop-speaking").addEventListener("click", () => window.speechSynthesis?.cancel());
  el("talk-channel").addEventListener("change", guard(loadDigest));
  el("text-channel").addEventListener("change", guard(loadScrollback));

  el("api-token").value = token();
  el("save-token").addEventListener(
    "click",
    guard(async () => {
      localStorage.setItem(TOKEN_KEY, el("api-token").value.trim());
      await loadConfig();
    })
  );
  el("forget-token").addEventListener("click", () => {
    localStorage.removeItem(TOKEN_KEY);
    el("api-token").value = "";
    setStatus("token forgotten");
  });
}

wire();
guard(loadConfig)();
