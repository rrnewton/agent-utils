// The Settings editors for the canned prompts (`#60 canned-prompt-buttons`; `#624 voice-page-modularity`).
// A button sends the current field, with the shipped wording when it is empty. The tray and sending
// remain the voice view's; keeping an edit and saying whether it was kept belong here.

import type { StoragePort } from "../../ports/index.ts";

export interface CannedPrompt {
  readonly key: string;
  readonly button: string;
  readonly field: string;
  readonly title: string;
  readonly text: string;
  readonly said: string;
}

export const PROMPTS_KEY = "vibe-talk.voice.prompts";

export const CANNED_PROMPTS: readonly CannedPrompt[] = [
  {
    key: "summary",
    button: "canned-summary",
    field: "prompt-summary",
    // The resting tooltip, HERE rather than in web/voice.html, for the same reason the default
    // prompt text is here: `renderCannedPrompts` swaps the title for "Start a call first" and back,
    // so a copy in the markup would be a second answer to "what does this button say" that goes
    // stale the first time the swap runs.
    title:
      "Ask the agent to summarize the recent messages from the coding agent in this channel. " +
      "Editable in Settings.",
    // WEAKER THAN THE ISSUE FILED, ON PURPOSE. The wording asked for was "Summarize my unread
    // messages from the coding agent since I last messaged them", and `#61 unread-status` reported
    // that both halves of that scoping are impossible here: Discord gives a bot no read state, and
    // the fallback is not computable either — the owner has no identity in this server, his own
    // replies are posted AS the bot, and the only author signal is `global_name`, which anyone can
    // set to anything. A button whose text claims a scoping the data cannot support produces
    // confident, wrong summaries, which is a failure this project has already paid for once. So
    // this asks for what the digest genuinely provides. The full argument, with code anchors, is
    // in ai_docs/UNREAD_STATUS_20260819.md; building the capability is a separate feature.
    text: "Summarize the recent messages from the coding agent in this channel.",
    said: "Asked for a summary of the recent messages in the channel.",
  },
  {
    key: "blockers",
    button: "canned-blockers",
    field: "prompt-blockers",
    title:
      "Ask the agent to make the CODING AGENT report its progress and anything waiting on you. " +
      "This spends coding-agent work. Editable in Settings.",
    // Unaffected by the above: it makes no claim about read state, it asks the coding agent to
    // report on itself.
    text:
      "Tell the coding agent to summarize what it has accomplished since last interacting with " +
      "the user, not assuming the user has read any updates in the intervening time, and list " +
      "out any blockers or items that are waiting on the human user.",
    said: "Asked the coding agent for progress and blockers.",
  },
];

/** Storage is resolved by the page on use, preserving the original first-read order. */
export interface CannedPromptsDependencies {
  lookup(id: string): HTMLElement;
  readonly storage: StoragePort;
}

export interface CannedPromptsEditor {
  stored(): Record<string, string>;
  promptFor(entry: CannedPrompt): string;
  changed(): void;
}

/** Construction reads no setting and changes no field. */
export function mountCannedPrompts(deps: CannedPromptsDependencies): CannedPromptsEditor {
  /** What was stored, defaulted PER KEY, so a corrupt or partial entry cannot empty a button. */
  function storedPrompts(): Record<string, string> {
    let stored: Record<string, unknown> | null = null;
    try {
      stored = JSON.parse(deps.storage.getItem(PROMPTS_KEY) || "null");
    } catch (_error) {
      stored = null;
    }
    const prompts: Record<string, string> = {};
    for (const entry of CANNED_PROMPTS) {
      const held = stored && typeof stored === "object" ? stored[entry.key] : null;
      prompts[entry.key] = typeof held === "string" && held.trim() !== "" ? held : entry.text;
    }
    return prompts;
  }

  function persistPrompts(): boolean {
    const prompts: Record<string, string> = {};
    for (const entry of CANNED_PROMPTS) {
      prompts[entry.key] = promptFor(entry);
    }
    const encoded = JSON.stringify(prompts);
    try {
      deps.storage.setItem(PROMPTS_KEY, encoded);
    } catch (_error) {
      return false;
    }
    // Read back rather than assume, for the third time in this file and for the same reason: private
    // browsing accepts setItem and stores nothing.
    return deps.storage.getItem(PROMPTS_KEY) === encoded;
  }

  /**
   * What this button will actually send.
   *
   * An emptied field falls back to the default rather than sending nothing. A canned button that has
   * been cleared out is otherwise a control that is present, looks live, and does nothing — the
   * exact failure this page keeps removing.
   */
  function promptFor(entry: CannedPrompt): string {
    return (deps.lookup(entry.field) as HTMLTextAreaElement).value.trim() || entry.text;
  }

  function promptsChanged(): void {
    deps.lookup("prompt-state").textContent = persistPrompts()
      ? "Saved. The buttons send this from now on."
      : "This browser refused to store the prompts, so they will be back to the defaults when you " +
        "reload (private browsing does this). The buttons send what is in the boxes for now.";
  }

  return { stored: storedPrompts, promptFor, changed: promptsChanged };
}
