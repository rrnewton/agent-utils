// Applying synced Settings and listening for missed updates (#624 voice-page-modularity).
// Feature commands are injected; Settings does not take ownership of row, link or thread state.
import type { SettingsSync } from "../../core/settings/index.ts";
import { IDENTITY_KEY } from "../../core/settings/index.ts";
import { CANNED_PROMPTS, PROMPTS_KEY } from "./canned.ts";
import type { CannedPromptsEditor } from "./canned.ts";
import { MIC_SETTINGS_KEY, MIC_TOGGLES } from "./microphone.ts";
import type { MicrophoneSettingsEditor } from "./microphone.ts";

export interface SettingsSyncViewDependencies {
  lookup(id: string): HTMLElement;
  sync(): SettingsSync;
  guard(run: () => Promise<void>): () => void;
  readonly keys: {
    readonly READ_MODE_KEY: string;
    readonly MARK_OWN_KEY: string;
    readonly HIDE_EMPTY_KEY: string;
    readonly COMBINE_KEY: string;
    readonly RICH_HEADERS_KEY: string;
    readonly SUMMARIZE_COLLAPSED_KEY: string;
    readonly IN_APP_LINKS_KEY: string;
    readonly OPEN_IN_VIEW_KEY: string;
    readonly AFTER_DONE_KEY: string;
    readonly COLLAPSED_LINES_KEY: string;
    readonly CARD_MAX_LINES_KEY: string;
    readonly READ_SPEED_KEY: string;
    readonly READ_AUDIO_SOURCE_KEY: string;
    readonly WIT_VOICE_KEY: string;
    readonly QUOTE_READ_PERCENT_KEY: string;
    readonly RELAY_KEY: string;
    readonly LINK_KINDS_KEY: string;
    readonly LINK_RAW_KEY: string;
    readonly THREADS_UNPACKED_KEY: string;
    readonly THREAD_CARD_STATES_KEY: string;
    readonly DOC_STATES_KEY: string;
    readonly THREADS_MAIN_KEY: string;
    readonly THREAD_ORDER_KEY: string;
    readonly THREAD_CARD_FOLD_KEY: string;
    readonly RESUME_KEY: string;
  };
  readonly microphone: MicrophoneSettingsEditor;
  readonly prompts: CannedPromptsEditor;
  resumeWanted(): boolean;
  replaceIdentityChoices(choices: Record<string, unknown>): void;
  renderChannelRows(): void;
  renderIdentities(): void;
  readMode(value: string): void;
  markOwnRead(): void;
  hideEmptyMessages(): void;
  combineMessages(): void;
  richHeaders(): void;
  summarizeCollapsed(): void;
  inAppLinks(): void;
  openInView(): void;
  afterDone(): void;
  collapsedLines(): void;
  cardMaxLines(): void;
  readSpeed(): void;
  readAudioSource(): void;
  witVoice(): void;
  quoteReadPercent(): void;
  relay(): void;
  linkKinds(): void;
  linkRaw(): void;
  threadsUnpacked(): void;
  threadCardStates(): void;
  /** `#1265 documents-star-hide`. Another device starred or hid a document. */
  docStates(): void;
  threadsMain(): void;
  threadOrder(): void;
  threadCardFold(): void;
}

export function mountSettingsSyncView(deps: SettingsSyncViewDependencies) {
  function applySyncedSetting(key: string): void {
    try {
      switch (key) {
        case deps.keys.READ_MODE_KEY: {
          // None kept is the shipped default's turn, as everywhere: Show read.
          const keptMode = deps.sync().local(key) || "show";
          deps.readMode(keptMode === "hide" ? "fold" : keptMode);
          break;
        }
        // `#549 own-read-stored`. Only the switch: nothing drawn depends on it.
        case deps.keys.MARK_OWN_KEY:
          deps.markOwnRead();
          break;
        case deps.keys.HIDE_EMPTY_KEY:
          deps.hideEmptyMessages();
          break;
        case deps.keys.COMBINE_KEY:
          deps.combineMessages();
          break;
        case deps.keys.RICH_HEADERS_KEY:
          deps.richHeaders();
          break;
        case deps.keys.SUMMARIZE_COLLAPSED_KEY:
          deps.summarizeCollapsed();
          break;
        case deps.keys.IN_APP_LINKS_KEY:
          deps.inAppLinks();
          break;
        case deps.keys.OPEN_IN_VIEW_KEY:
          deps.openInView();
          break;
        case deps.keys.AFTER_DONE_KEY:
          deps.afterDone();
          break;
        case deps.keys.COLLAPSED_LINES_KEY:
          deps.collapsedLines();
          break;
        case deps.keys.CARD_MAX_LINES_KEY:
          deps.cardMaxLines();
          break;
        case deps.keys.READ_SPEED_KEY:
          deps.readSpeed();
          break;
        case deps.keys.READ_AUDIO_SOURCE_KEY:
          deps.readAudioSource();
          break;
        case deps.keys.WIT_VOICE_KEY:
          deps.witVoice();
          break;
        case deps.keys.QUOTE_READ_PERCENT_KEY:
          deps.quoteReadPercent();
          break;
        case MIC_SETTINGS_KEY: {
          const held = deps.microphone.stored();
          for (const [id, name] of MIC_TOGGLES) {
            (deps.lookup(id) as HTMLInputElement).checked = held[name];
          }
          break;
        }
        case PROMPTS_KEY: {
          const held = deps.prompts.stored();
          for (const entry of CANNED_PROMPTS) {
            const field = deps.lookup(entry.field);
            // A field being typed in is its typist's replica: the next keystroke writes it back
            // and pushes it, which is last-writer-wins with the writer still writing.
            if (document.activeElement !== field) {
              (field as HTMLTextAreaElement).value = held[entry.key]!;
            }
          }
          break;
        }
        case deps.keys.RESUME_KEY:
          (deps.lookup("resume-toggle") as HTMLInputElement).checked = deps.resumeWanted();
          break;
        case deps.keys.RELAY_KEY:
          deps.relay();
          break;
        case deps.keys.LINK_KINDS_KEY:
          deps.linkKinds();
          break;
        case deps.keys.LINK_RAW_KEY:
          // As a press here does: the reader's message kept where it is while every link's height
          // changes under it.
          deps.linkRaw();
          break;
        case deps.keys.THREADS_UNPACKED_KEY:
          // Not `setUnpackMode`: that is the reader's press, which also gives every thread of the
          // channel back to Unpack — a remote change of the default must not throw away the
          // per-thread choices that sync beside it.
          deps.threadsUnpacked();
          break;
        case deps.keys.THREAD_CARD_STATES_KEY: {
          deps.threadCardStates();
          break;
        }
        case deps.keys.DOC_STATES_KEY: {
          deps.docStates();
          break;
        }
        case deps.keys.THREADS_MAIN_KEY:
          deps.threadsMain();
          break;
        case deps.keys.THREAD_ORDER_KEY:
          deps.threadOrder();
          break;
        case deps.keys.THREAD_CARD_FOLD_KEY:
          deps.threadCardFold();
          break;
        case IDENTITY_KEY: {
          // `#488 slack-app-agents`. Another device said who an account is: every row it wrote is
          // drawn that way here too, and the list in Settings says so.
          let held: Record<string, unknown> = {};
          try {
            const parsed = JSON.parse(deps.sync().local(key) || "null") as unknown;
            if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) held = parsed as Record<string, unknown>;
          } catch (_error) {
            // A value this page cannot read is no choice at all.
          }
          deps.replaceIdentityChoices(held);
          deps.renderChannelRows();
          deps.renderIdentities();
          break;
        }
        default: break;
      }
    } catch (_error) {
      // The value is kept regardless; the next load applies it through the ordinary restore.
    }
  }


  function renderSettingsSyncState(text: string): void {
    const line = deps.lookup("settings-sync-state");
    if (!line) return;
    line.hidden = !text;
    if (text && line.textContent !== text) line.textContent = text;
  }
  const onVisibility = (): void => {
    if (!document.hidden) deps.sync().refresh().catch(() => {});
  };
  const onFocus = (): void => { deps.sync().refresh().catch(() => {}); };
  const onOnline = (): void => { deps.sync().syncOnce().catch(() => {}); };
  function start(): void {
    if (!deps.sync().start()) return;
    deps.guard(() => deps.sync().syncOnce())();
    deps.sync().keepPassing();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onFocus);
    window.addEventListener("online", onOnline);
  }
  return {
    apply: applySyncedSetting,
    status: renderSettingsSyncState,
    start,
    dispose(): void {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("online", onOnline);
    },
  };
}
export type SettingsSyncView = ReturnType<typeof mountSettingsSyncView>;
