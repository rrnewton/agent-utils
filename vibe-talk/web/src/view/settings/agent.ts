// The voice agent's send approval and prompt editors, with the last call's handover report.
// Moved from voice.js for #624 voice-page-modularity; requests and rendering keep their order.

import type { Contract, SessionPrompt, VoicePromptSettings, VoiceSettings } from "../../contract/index.ts";
import {
  VOICE_SEND_HOSTED,
  VOICE_SEND_STATE,
  agentPromptFollowsSwitch,
  promptChars,
  voiceInstructionsLine,
  voicePromptDeliveryLine,
  voicePromptLastCallLine,
  voicePromptLock,
  voiceSendSavedClause,
  voiceSystemPromptLine,
} from "../../present/settings/index.ts";
import type { AgentPromptLastCall } from "../../present/settings/index.ts";

/** The two independently saved fields of the prompt editor. */
export type AgentPromptField = "instructions" | "system_prompt";

/** The part of the opening frame that this owner may extend, retaining every other field. */
export interface AgentPromptInitiation {
  conversation_config_override?: {
    agent?: { prompt?: { prompt: string }; [field: string]: unknown };
    [field: string]: unknown;
  };
}

/** What follows the initiation, and whether it now replaces the hosted agent's prompt. */
export interface AgentPromptHandover {
  readonly note: { readonly type: "contextual_update"; readonly text: string } | null;
  readonly replaced: boolean;
}

type RequestFailure = { readonly detail?: string; readonly message?: string };

/** The page capabilities used by the editors, each read only when the old script read it. */
export interface AgentSettingsDependencies {
  lookup(id: string): HTMLElement;
  decoded<K extends keyof Contract>(type: K, path: string, options?: { method: string; body?: unknown }): Promise<Contract[K]>;
  writable(): boolean;
  showError(text: string): void;
  setStatus(text: string): void;
  addDetail(text: string): void;
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
  /** The existing confirmation window, read when Restore is armed. */
  clearArmedMs(): number;
}

/** The editors' named commands and the call's read surface. */
export interface AgentSettings {
  /** A detached copy of the last server-reported switch, or null for an older server. */
  sendSetting(): Readonly<VoiceSettings> | null;
  applySendSetting(view: VoiceSettings | null | undefined): void;
  renderSendSetting(): void;
  saveSendSetting(): Promise<void>;
  applyPrompt(view: VoicePromptSettings | null | undefined, keep?: readonly AgentPromptField[]): void;
  renderPrompt(): void;
  savePrompt(field: AgentPromptField, text: string): Promise<void>;
  restoreSystemPrompt(): Promise<void>;
  /** Change only the opening frame and the recorded handover; draw nothing before it is sent. */
  handOverPrompt(initiation: AgentPromptInitiation, handed: SessionPrompt | null | undefined): AgentPromptHandover;
  /** A render failure is reported and cannot stop the call from opening. */
  renderAfterHandOver(): void;
  /** Retire the pending Restore confirmation timer without drawing. */
  dispose(): void;
}

interface AgentElements {
  "voice-ask-before-send": HTMLInputElement;
  "voice-ask-before-send-state": HTMLElement;
  "voice-send-editor": HTMLElement;
  "voice-send-untick": HTMLElement;
  "voice-send-hosted": HTMLElement;
  "voice-instructions": HTMLTextAreaElement;
  "voice-system-prompt": HTMLTextAreaElement;
  "voice-prompt-state": HTMLElement;
  "voice-prompt-editor": HTMLElement;
  "save-voice-instructions": HTMLButtonElement;
  "save-voice-system-prompt": HTMLButtonElement;
  "restore-voice-system-prompt": HTMLButtonElement;
  "voice-instructions-state": HTMLElement;
  "voice-system-prompt-state": HTMLElement;
}

/** No server report, write or restore confirmation. Construction has no page effects. */
export function mountAgentSettings(deps: AgentSettingsDependencies): AgentSettings {
  const el = <K extends keyof AgentElements>(id: K): AgentElements[K] => deps.lookup(id) as AgentElements[K];

  // --- asking before the voice agent sends -----------------------------------------------------------
  //
  // `#264 voice-send-approval`. The SERVER's switch over what the voice agent's `post_reply` does: on
  // (the shipped default) it proposes a draft this page shows on the card and the owner confirms; off,
  // it posts at once. It is drawn from client-config and redrawn from what the PUT answered — never
  // from the click — because the server is what the agent obeys, for every device and every voice
  // provider, and a box that showed a value the server did not hold would be a lie about the agent.

  /** The setting as the server last reported it, or null when it does not say (an older server). */
  let voiceSendSetting: VoiceSettings | null = null;

  /** True while a change is on its way to the server, so a second click cannot race it. */
  let voiceSendSaving = false;

  /** Take down what the server said — from client-config, or from a save. */
  function applyVoiceSendSetting(view: VoiceSettings | null | undefined): void {
    voiceSendSetting =
      view && typeof view.ask_before_send === "boolean"
        ? { ask_before_send: view.ask_before_send, changeable: view.changeable === true }
        : null;
    renderVoiceSendSetting();
  }

  /** Draw the checkbox and its line from `voiceSendSetting` and the token's scope. */
  function renderVoiceSendSetting(): void {
    const box = el("voice-ask-before-send");
    const state = el("voice-ask-before-send-state");
    el("voice-send-editor").hidden = voiceSendSetting === null;
    if (voiceSendSetting === null) {
      state.textContent =
        "This server does not report this setting, so its voice agent always asks before it sends.";
      return;
    }
    box.checked = voiceSendSetting.ask_before_send;
    // The way to send without asking, said only while asking is on: unticked, it would describe the
    // state the box is not in.
    el("voice-send-untick").hidden = !voiceSendSetting.ask_before_send;
    renderVoiceSendHosted();
    const line = voiceSendSetting.ask_before_send ? VOICE_SEND_STATE.on : VOICE_SEND_STATE.off;
    if (!deps.writable()) {
      box.disabled = true;
      state.textContent = `${line} Only the write token can change this.`;
    } else if (!voiceSendSetting.changeable) {
      box.disabled = true;
      state.textContent = `${line} This server keeps no settings, so this cannot be changed here.`;
    } else {
      box.disabled = voiceSendSaving;
      state.textContent = line;
    }
  }

  /** Send the box's new position, then draw what the server STORED; on failure, put it back. */
  async function saveVoiceSendSetting(): Promise<void> {
    const wanted = el("voice-ask-before-send").checked;
    voiceSendSaving = true;
    renderVoiceSendSettingWhileSaving(wanted);
    let stored = null;
    try {
      stored = await deps.decoded("VoiceSettings", "/api/v1/voice-settings", {
        method: "PUT",
        body: { ask_before_send: wanted },
      });
    } catch (error) {
      voiceSendSaving = false;
      // Back to what the server holds: the click did not change the agent, so the box must not say so.
      renderVoiceSendSetting();
      deps.showError(`The voice send setting was not changed: ${(error as RequestFailure).detail || (error as RequestFailure).message}`);
      return;
    }
    voiceSendSaving = false;
    applyVoiceSendSetting(stored);
    // The server obeys the new setting at once. The agent's instructions follow only where something
    // hands them over, which the clause says.
    deps.setStatus(
      stored.ask_before_send
        ? `Saved. The server now holds what the voice agent sends for your yes; ${voiceSendSavedClause(agentPromptFollowsSwitch(voicePromptSettings, voicePromptLastCall))}`
        : `Saved. The server now posts as soon as the voice agent sends; ${voiceSendSavedClause(agentPromptFollowsSwitch(voicePromptSettings, voicePromptLastCall))}`
    );
  }

  /** Show the hosted-agent line beside the switch, worded from what is known; hidden for a bridge. */
  function renderVoiceSendHosted(): void {
    const follows = agentPromptFollowsSwitch(voicePromptSettings, voicePromptLastCall);
    const hosted = el("voice-send-hosted");
    hosted.hidden = follows === "bridge";
    if (follows !== "bridge") hosted.textContent = VOICE_SEND_HOSTED[follows];
  }

  /** The box as clicked, held still until the server answers. */
  function renderVoiceSendSettingWhileSaving(wanted: boolean): void {
    el("voice-ask-before-send").checked = wanted;
    el("voice-ask-before-send").disabled = true;
  }

  // --- the voice agent's instructions ----------------------------------------------------------------
  //
  // `#272 voice-agent-instructions`. The owner's own words for the agent — additional instructions,
  // and his edited copy of the base system prompt — are the SERVER's, like the send switch above: it
  // puts them into the prompt when a call opens, for every device. So both boxes are filled from
  // client-config and refilled from what the PUT stored, and each has its own Save, because a
  // half-typed instruction must not reach the agent.
  //
  // How the words reach the agent is the server's to say (`delivery`). A bridge fetches the prompt as
  // each call opens. A hosted agent is handed them by THIS page as its socket opens, in the form the
  // voice session names (`handOverAgentPrompt`) — the whole prompt only when the agent allows it to be
  // replaced. What the last call was actually given is kept, so the line under the boxes can say it:
  // an edited prompt a hosted agent would not take is exactly what the owner needs to be told.

  /** The owner's words as the server last reported them, or null when it does not say (an older server). */
  let voicePromptSettings: VoicePromptSettings | null = null;

  /** True while a save is on its way, so a second click cannot race it. */
  let voicePromptSaving = false;

  /**
   * What the last call this page opened on a hosted agent was handed, or null before one, with
   * whether the owner's own copy of the prompt was saved as it opened (`editedWhenOpened`; null when
   * this server did not say). The line under Instructions says why such a call got nothing from
   * that, so a save after the call cannot rewrite what the call was given.
   */
  let voicePromptLastCall: AgentPromptLastCall | null = null;

  /**
   * While Restore is armed, the timer that disarms it; null when a tap would only arm it.
   *
   * Restore deletes the owner's own copy of the prompt — perhaps hours of his wording — so it asks
   * twice, in the armed idiom Clear and the backlog's Clear use, with their window: the first tap
   * changes its word and colour, and only a second tap within that window acts.
   */
  let restoreArmedTimer: ReturnType<typeof setTimeout> | null = null;

  /** Which box holds text the server does not. */
  function voicePromptDirty(): Record<AgentPromptField, boolean> {
    if (voicePromptSettings === null) return { instructions: false, system_prompt: false };
    return {
      instructions: el("voice-instructions").value.trim() !== voicePromptSettings.instructions,
      system_prompt: el("voice-system-prompt").value.trim() !== voicePromptSettings.system_prompt.trim(),
    };
  }

  /**
   * Take down what the server said — from client-config, or from a save — and fill the boxes from it.
   * A box named in `keep` keeps text the reader typed and has not saved: saving one box must not
   * throw away an edit in the other.
   */
  function applyVoicePrompt(view: VoicePromptSettings | null | undefined, keep: readonly AgentPromptField[] = []): void {
    const dirty = voicePromptDirty();
    const typed = { instructions: el("voice-instructions").value, system_prompt: el("voice-system-prompt").value };
    voicePromptSettings =
      view && typeof view.instructions === "string" && typeof view.system_prompt === "string" ? view : null;
    if (voicePromptSettings !== null) {
      for (const field of ["instructions", "system_prompt"] as const) {
        const box = el(field === "instructions" ? "voice-instructions" : "voice-system-prompt");
        box.value = keep.includes(field) && dirty[field] ? typed[field] : voicePromptSettings[field];
      }
    }
    renderVoicePrompt();
  }

  /** Draw both boxes, their Saves and the lines under them from `voicePromptSettings`. */
  function renderVoicePrompt(): void {
    // The line beside the send switch is worded from the same knowledge: how the prompt is delivered,
    // and what the last call was given.
    renderVoiceSendHosted();
    const state = el("voice-prompt-state");
    el("voice-prompt-editor").hidden = voicePromptSettings === null;
    if (voicePromptSettings === null) {
      state.textContent = "This server does not take instructions for its voice agent.";
      return;
    }
    const lock = voicePromptLock(voicePromptSettings, deps.writable());
    const dirty = voicePromptDirty();
    const held = Boolean(lock) || voicePromptSaving;
    el("voice-instructions").disabled = Boolean(lock);
    el("voice-system-prompt").disabled = Boolean(lock);
    el("save-voice-instructions").disabled =
      held || !dirty.instructions ||
      promptChars(el("voice-instructions").value) > voicePromptSettings.max_instructions_chars;
    el("save-voice-system-prompt").disabled =
      held || !dirty.system_prompt ||
      promptChars(el("voice-system-prompt").value) > voicePromptSettings.max_system_prompt_chars;
    const restore = el("restore-voice-system-prompt");
    restore.disabled = held || (!voicePromptSettings.system_prompt_edited && !dirty.system_prompt);
    if (restore.disabled && restoreArmedTimer !== null) {
      deps.clearTimeout(restoreArmedTimer);
      restoreArmedTimer = null;
    }
    const armed = restoreArmedTimer !== null;
    restore.className = armed ? "secondary armed" : "secondary";
    restore.textContent = !armed
      ? "Restore the shipped prompt"
      : voicePromptSettings.system_prompt_edited
        ? "Delete your copy?"
        : "Discard your edit?";
    el("voice-instructions-state").textContent = voiceInstructionsLine(voicePromptSettings, el("voice-instructions").value, dirty.instructions);
    el("voice-system-prompt-state").textContent = voiceSystemPromptLine(voicePromptSettings, el("voice-system-prompt").value, dirty.system_prompt);
    state.textContent = [lock, voicePromptDeliveryLine(voicePromptSettings), voicePromptLastCallLine(voicePromptSettings, voicePromptLastCall)].filter(Boolean).join(" ");
  }

  /** Save one box, then draw what the server STORED; on failure, keep the typed text and say so. */
  async function saveVoicePrompt(field: AgentPromptField, text: string): Promise<void> {
    voicePromptSaving = true;
    renderVoicePrompt();
    let stored = null;
    try {
      stored = await deps.decoded("VoicePromptSettings", "/api/v1/voice-prompt", {
        method: "PUT",
        body: { [field]: text },
      });
    } catch (error) {
      voicePromptSaving = false;
      renderVoicePrompt();
      const what = field === "instructions" ? "The instructions were" : "The system prompt was";
      deps.showError(`${what} not saved: ${(error as RequestFailure).detail || (error as RequestFailure).message}`);
      return;
    }
    voicePromptSaving = false;
    applyVoicePrompt(stored, [field === "instructions" ? "system_prompt" : "instructions"]);
    if (field === "instructions") {
      deps.setStatus(
        stored.instructions
          ? "Saved. The voice agent gets these instructions from its next call."
          : "Saved. The voice agent has no additional instructions from its next call."
      );
    } else {
      // His copy — or after Restore, the shipped prompt — reaches a hosted agent only if the agent
      // lets this page replace its prompt; one that does not keeps the prompt in its console.
      const which = stored.system_prompt_edited ? "your prompt" : "the shipped prompt";
      const follows = agentPromptFollowsSwitch(voicePromptSettings, voicePromptLastCall);
      deps.setStatus(
        follows === "kept"
          ? `Saved, but the last call's agent does not let this page replace its prompt, so ${which} will not reach it; ` +
              (stored.system_prompt_edited ? "your instructions still do." : "it keeps the one in its provider's console.")
          : follows === "unknown"
            ? `Saved. The voice agent gets ${which} from its next call if its provider lets this page replace its prompt.`
            : `Saved. The voice agent gets ${which} from its next call.`
      );
    }
  }

  function disarmRestore(): void {
    restoreArmedTimer = null;
    renderVoicePrompt();
  }

  /** Back to the shipped prompt: a save of an empty box, or just the box put back when nothing was saved. */
  async function restoreVoiceSystemPrompt(): Promise<void> {
    if (voicePromptSettings === null) return;
    if (restoreArmedTimer === null) {
      restoreArmedTimer = deps.setTimeout(disarmRestore, deps.clearArmedMs());
      renderVoicePrompt();
      deps.setStatus(
        voicePromptSettings.system_prompt_edited
          ? "Tap again to delete your copy of the prompt and go back to the shipped one."
          : "Tap again to throw away your edit and put the shipped prompt back."
      );
      return;
    }
    deps.clearTimeout(restoreArmedTimer);
    restoreArmedTimer = null;
    if (!voicePromptSettings.system_prompt_edited) {
      el("voice-system-prompt").value = voicePromptSettings.system_prompt;
      renderVoicePrompt();
      return;
    }
    await saveVoicePrompt("system_prompt", "");
  }

  /**
   * Put the owner's words on a hosted agent's opening frames, in the form the server chose, and
   * remember what this call was given. Answers `note`, the frame to send right after the initiation
   * or null, and `replaced`, whether the initiation now replaces the agent's own prompt.
   *
   * `prompt_override` rides on the initiation itself, beside a typed call's text-only request; the
   * server names it only when the agent's own configuration allows it, because one it does not allow
   * is believed to end the conversation. `contextual_update` is a note sent straight after, as
   * resuming's record is. Anything else sends nothing, and an absent `handed` leaves the frame exactly
   * as it always was.
   *
   * It draws nothing. It runs inside the socket's open handler before the initiation is sent, and
   * Settings is redrawn only after the frames are (`renderAfterHandOver`), so drawing Settings can
   * never stand between the owner and his call.
   */
  function handOverAgentPrompt(initiation: AgentPromptInitiation, handed: SessionPrompt | null | undefined): AgentPromptHandover {
    voicePromptLastCall =
      handed && typeof handed.transport === "string"
        ? {
            ...handed,
            editedWhenOpened: voicePromptSettings === null ? null : voicePromptSettings.system_prompt_edited === true,
          }
        : null;
    const nothing = { note: null, replaced: false };
    if (voicePromptLastCall === null || typeof handed!.text !== "string" || handed!.text === "") return nothing;
    if (handed!.transport === "prompt_override") {
      const override = initiation.conversation_config_override || {};
      initiation.conversation_config_override = {
        ...override,
        agent: { ...(override.agent || {}), prompt: { prompt: handed!.text } },
      };
      return { note: null, replaced: true };
    }
    if (handed!.transport === "contextual_update") {
      return { note: { type: "contextual_update", text: handed!.text }, replaced: false };
    }
    return nothing;
  }

  /**
   * Redraw Settings' lines about what the call was given, once its opening frames are sent. A
   * failure here is caught and noted rather than allowed out: the rest of the open handler — the
   * call going live, the microphone starting — must not depend on a settings screen drawing.
   */
  function renderAfterHandOver(): void {
    try {
      renderVoicePrompt();
    } catch (error) {
      deps.addDetail(`Settings could not show what this call was given: ${String((error && (error as RequestFailure).message) || error)}`);
    }
  }

  return {
    sendSetting: () => voiceSendSetting === null ? null : { ...voiceSendSetting },
    applySendSetting: applyVoiceSendSetting,
    renderSendSetting: renderVoiceSendSetting,
    saveSendSetting: saveVoiceSendSetting,
    applyPrompt: applyVoicePrompt,
    renderPrompt: renderVoicePrompt,
    savePrompt: saveVoicePrompt,
    restoreSystemPrompt: restoreVoiceSystemPrompt,
    handOverPrompt: handOverAgentPrompt,
    renderAfterHandOver,
    dispose() {
      if (restoreArmedTimer !== null) deps.clearTimeout(restoreArmedTimer);
      restoreArmedTimer = null;
    },
  };
}
