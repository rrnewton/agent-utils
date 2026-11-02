// The prefix the server puts on the voice agent's posts: editor, writes and fresh reads.
// Moved from voice.js for #624 voice-page-modularity.

import type { Contract, VoicePostPrefix } from "../../contract/index.ts";
import { cleanPostPrefix, voicePostPrefixLine } from "../../present/settings/index.ts";

type RequestFailure = { readonly detail?: string; readonly message?: string };

/** The page capabilities the prefix editor uses. */
export interface AgentPrefixDependencies {
  lookup(id: string): HTMLElement;
  decoded<K extends keyof Contract>(type: K, path: string, options?: { method: string; body?: unknown }): Promise<Contract[K]>;
  writable(): boolean;
  showError(text: string): void;
  setStatus(text: string): void;
}

/** The prefix editor's named commands. */
export interface AgentPrefix {
  apply(view: VoicePostPrefix | null | undefined): void;
  render(): void;
  save(reset: boolean): Promise<void>;
  refresh(): Promise<void>;
  /** Drop a read still on its way, without drawing. */
  dispose(): void;
}

interface PrefixElements {
  "voice-post-prefix": HTMLInputElement;
  "voice-post-prefix-editor": HTMLElement;
  "save-voice-post-prefix": HTMLButtonElement;
  "reset-voice-post-prefix": HTMLButtonElement;
  "voice-post-prefix-state": HTMLElement;
}

/** No report, read or write. Construction has no page effects. */
export function mountAgentPrefix(deps: AgentPrefixDependencies): AgentPrefix {
  const el = <K extends keyof PrefixElements>(id: K): PrefixElements[K] => deps.lookup(id) as PrefixElements[K];

  // --- the prefix on the voice agent's posts ---------------------------------------------------------
  //
  // `#303 voice-post-prefix`. The tag the SERVER puts before every message the voice agent posts, so
  // the channel can tell a relayed message from one the owner typed. Like the send switch it is
  // the server's — kept in its store, the same for every device and every call — so the box is filled
  // from client-config and refilled from what the PUT or DELETE answered. Save is its own, because a
  // half-typed tag must not go out on the agent's next post. An empty box saved is the owner's choice
  // of no tag, which the server keeps apart from never having chosen; Reset to default is the way back
  // to the shipped one.
  //
  // client-config is read once, at sign-in, and another device may save or reset the tag after that.
  // So opening Settings reads the tag again (`refreshVoicePostPrefix`): the box shows what is in force
  // now, and Reset takes the path the server's state calls for, not the one this page last heard of.

  /** The tag as the server last reported it, or null when it does not say (an older server). */
  let voicePostPrefix: VoicePostPrefix | null = null;

  /** True while a save or a reset is on its way, so a second click cannot race it. */
  let voicePostPrefixSaving = false;

  /**
   * Counts the reads and writes of the tag this page has sent, so a read that answers after a newer
   * one, or after a save or reset began, is not drawn over it.
   */
  let voicePostPrefixReads = 0;

  /** Take down what the server said — from client-config, or from a save — and fill the box from it. */
  function applyVoicePostPrefix(view: VoicePostPrefix | null | undefined): void {
    voicePostPrefix =
      view && typeof view.prefix === "string" && typeof view.default_prefix === "string" ? view : null;
    if (voicePostPrefix !== null) el("voice-post-prefix").value = voicePostPrefix.prefix;
    renderVoicePostPrefix();
  }

  /** Whether the box holds a tag the server does not. */
  function voicePostPrefixDirty(): boolean {
    return voicePostPrefix !== null && cleanPostPrefix(el("voice-post-prefix").value) !== voicePostPrefix.prefix;
  }

  /** Draw the box, its two buttons and its line from `voicePostPrefix` and the token's scope. */
  function renderVoicePostPrefix(): void {
    el("voice-post-prefix-editor").hidden = voicePostPrefix === null;
    if (voicePostPrefix === null) return;
    const lock = !deps.writable()
      ? "Only the write token can change this."
      : voicePostPrefix.changeable ? "" : "This server keeps no settings, so this cannot be changed here.";
    const box = el("voice-post-prefix");
    const dirty = voicePostPrefixDirty();
    const length = [...cleanPostPrefix(box.value)].length;
    const held = Boolean(lock) || voicePostPrefixSaving;
    box.disabled = Boolean(lock);
    el("save-voice-post-prefix").disabled = held || !dirty || length > voicePostPrefix.max_chars;
    // Nothing to reset when the default is in force and in the box.
    el("reset-voice-post-prefix").disabled = held || (!voicePostPrefix.custom && !dirty);
    el("voice-post-prefix-state").textContent = [lock, voicePostPrefixLine(voicePostPrefix, dirty, length)].filter(Boolean).join(" ");
  }

  /**
   * Save the box (`reset` false) or go back to the default (`reset` true), then draw what the server
   * STORED. On failure the typed tag stays in the box, and the error says it was not saved.
   */
  async function saveVoicePostPrefix(reset: boolean): Promise<void> {
    if (voicePostPrefix === null) return;
    if (reset && !voicePostPrefix.custom) {
      // Nothing of his is saved: put the default back in the box and write nothing.
      el("voice-post-prefix").value = voicePostPrefix.default_prefix;
      renderVoicePostPrefix();
      return;
    }
    voicePostPrefixSaving = true;
    // A read still on its way is older than what this write will answer.
    voicePostPrefixReads += 1;
    renderVoicePostPrefix();
    let stored = null;
    try {
      stored = await deps.decoded(
        "VoicePostPrefix",
        "/api/v1/voice-post-prefix",
        reset ? { method: "DELETE" } : { method: "PUT", body: { prefix: el("voice-post-prefix").value } }
      );
    } catch (error) {
      voicePostPrefixSaving = false;
      renderVoicePostPrefix();
      deps.showError(`The prefix was not ${reset ? "reset" : "saved"}: ${(error as RequestFailure).detail || (error as RequestFailure).message}`);
      return;
    }
    voicePostPrefixSaving = false;
    applyVoicePostPrefix(stored);
    deps.setStatus(
      !stored.custom
        ? "Reset. The voice agent's messages start with the default prefix again."
        : stored.prefix
          ? "Saved. Every message the voice agent posts starts with this prefix."
          : "Saved. The voice agent's messages carry no prefix."
    );
  }

  /**
   * Read the tag again, as Settings opens: another device may have saved or reset it since this page
   * read client-config. What the server answers is taken as the truth — the line, the buttons, and the
   * path Reset takes — but a tag typed here and not yet saved stays in the box, still marked unsaved.
   * Not for an older server that reports no tag, nor while a save or reset is on its way; a read that
   * fails leaves what is shown, and the next save or reset answers with the truth anyway.
   */
  async function refreshVoicePostPrefix(): Promise<void> {
    if (voicePostPrefix === null || voicePostPrefixSaving) return;
    const read = ++voicePostPrefixReads;
    let view;
    try {
      view = await deps.decoded("VoicePostPrefix", "/api/v1/voice-post-prefix");
    } catch (_error) {
      return;
    }
    if (read !== voicePostPrefixReads || voicePostPrefixSaving) return;
    const typed = voicePostPrefixDirty() ? el("voice-post-prefix").value : null;
    applyVoicePostPrefix(view);
    if (typed !== null && voicePostPrefix !== null) {
      el("voice-post-prefix").value = typed;
      renderVoicePostPrefix();
    }
  }

  return {
    apply: applyVoicePostPrefix,
    render: renderVoicePostPrefix,
    save: saveVoicePostPrefix,
    refresh: refreshVoicePostPrefix,
    dispose() { voicePostPrefixReads += 1; },
  };
}
