// The keys of the settings the server keeps for every one of the owner's devices (`#457
// settings-server-side`; `#624 voice-page-modularity`, moved from the legacy script). The full
// inventory and its reasons are in `src/client_settings.rs`.

/** Every device-storage key whose setting the server keeps. Keep beside `SYNCED_KEYS` in
 * `src/client_settings.rs` — a test there compares the two lists. */
export const SYNCED_SETTING_KEYS: readonly string[] = [
  "vibe-talk.voice.resume",
  "vibe-talk.voice.relay",
  "vibe-talk.voice.read-audio-source",
  "vibe-talk.voice.read-speed",
  // `#552 wit-tts-provider`. The Wit.ai voice chosen in Settings.
  "vibe-talk.voice.wit-voice",
  "vibe-talk.voice.prompts",
  "vibe-talk.voice.mic",
  "vibe-talk.voice.read-mode",
  "vibe-talk.voice.mark-own-read",
  "vibe-talk.voice.hide-empty",
  "vibe-talk.voice.combine",
  "vibe-talk.voice.rich-headers",
  // `#543 in-app-message-links`. Whether a link to a message opens it here.
  "vibe-talk.voice.in-app-links",
  // `#580 open-in-chat-jump`. The view a sent card's Open in channel lands in.
  "vibe-talk.voice.open-in-view",
  "vibe-talk.voice.collapsed-lines",
  "vibe-talk.voice.hidden-link-kinds",
  // `#620 link-url-chip`. Whether the Links view writes each link as its address.
  "vibe-talk.voice.link-raw-urls",
  // `#564 message-summary-toggle`. Summarize collapsed messages; the server reads it too, and
  // generates no message summary while it is off.
  "vibe-talk.voice.summarize-collapsed",
  "vibe-talk.voice.threads-unpacked",
  "vibe-talk.voice.thread-card-states",
  "vibe-talk.voice.threads-main",
  "vibe-talk.voice.thread-order",
  "vibe-talk.voice.thread-card-folded",
  // `#522 thread-card-max-lines`. K, the most lines a thread card shows of its root; the server
  // reads it too, to name a thread whose root is longer from its first reply.
  "vibe-talk.voice.thread-card-max-lines",
  // `#488 slack-app-agents`. Who is who: which account is the owner, the coding agent, a person or
  // another bot, chosen once for every device. The census of who spoke where stays local: a cache.
  "vibe-talk.voice.identities",
];

/** `#488 slack-app-agents`. Who is who: which account is the owner, the coding agent, a person or
 * another bot, chosen once for every device. Two devices' choices merge on the first read. */
export const IDENTITY_KEY = "vibe-talk.voice.identities";
