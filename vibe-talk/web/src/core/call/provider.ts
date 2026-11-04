// The conversational voice as the selected provider describes it (`#624 voice-page-modularity`, moved from the
// legacy script into core/call). Everything about HOW THE AGENT BEHAVES is owned by that provider: its trait
// supplies the human name and, where applicable, a direct settings URL and instance id, which the server passes
// on in `/api/v1/client-config`. The page neither recognizes vendor names nor constructs vendor URLs. Which
// screens say it, and where the call takes its name from it, are the page's.

/** What the provider says of its conversational voice. */
export interface ConversationalVoice {
  name: string;
  settings_url: string | null;
  instance_id: string | null;
}

/** The description's commands and questions. */
export interface VoiceProvider {
  /** The description standing now: a new object each time the server describes it again. */
  current(): ConversationalVoice;
  /**
   * The server described the voice as `described` (client-config's `conversational_voice`): a missing or
   * blank name is "voice provider", and a missing settings URL or instance id is null.
   */
  learn(described: unknown): void;
}

/** What the page says before the server has described anything. */
export function createVoiceProvider(): VoiceProvider {
  let conversationalVoice: ConversationalVoice = {
    name: "voice provider",
    settings_url: null,
    instance_id: null,
  };
  return {
    current: () => conversationalVoice,
    learn(described) {
      const describedVoice = described as { name?: unknown; settings_url?: unknown; instance_id?: unknown } | null | undefined;
      conversationalVoice = {
        name:
          describedVoice && typeof describedVoice.name === "string" && describedVoice.name.trim()
            ? describedVoice.name.trim()
            : "voice provider",
        settings_url:
          describedVoice && typeof describedVoice.settings_url === "string"
            ? describedVoice.settings_url
            : null,
        instance_id:
          describedVoice && typeof describedVoice.instance_id === "string"
            ? describedVoice.instance_id
            : null,
      };
    },
  };
}
