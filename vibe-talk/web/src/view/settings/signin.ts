// Sign-in: the token proved, saved and forgotten (`#63 status-line-placement`, `#616
// composer-no-autofill`, `#625 signin-flash`, `#18 offline-message-cache`, `#189 restore-ui-state`,
// `#457 settings-server-side`, `#462 webapp-deep-links`, `#543 in-app-message-links`, `#468
// send-dedupe`; `#624 voice-page-modularity`, moved from the legacy script into view/settings for
// the plan's step 17): what the sign-in screen says with nothing stored, the proof of the saved
// token against client-config, Save reading the token back, and Forget closing the stream before
// the credential goes. Every owner it touches is handed in; the words are the page's as they were.

import type { ClientConfigLike } from "./clientconfig.ts";

/** The Save button's resting words. */
export const SAVE_LABEL = "Save token";

export interface SignInDependencies {
  el(id: string): HTMLElement;
  field(id: string): { value: string };
  token(): string;
  signedOutByRename(): boolean;
  config(): Promise<ClientConfigLike>;
  applyClientConfig(config: ClientConfigLike): void;
  forgetMessages(): void;
  showError(text: string): void;
  setStatus(text: string): void;
  refusalSentence(error: { message: string; status?: unknown }): string;
  showScreen(name: string): void;
  loadFreshAt(): unknown;
  setChannelFreshness(state: string): void;
  payReopenRead(): void;
  channelContextKey(): string;
  threading(): boolean;
  reconcileChannelSnapshot(drawn: { key: string; threading: boolean }): void;
  clearError(): void;
  reopenSavedView(): void;
  setState(state: string): void;
  voiceTranscript(): { loadStoredConversation(): void };
  startSettingsSync(): void;
  settleUnconfirmedSends(): void;
  credentialStore(): { keep(value: string): void; forget(): void };
  stopOutgoingSends(): void;
  markSave(label: string, state: string, busy: boolean): void;
  revertSaveLater(run: () => void, ms: number): void;
  settingsSync(): { forget(): void };
  deepLinks: { signOut(): void; noteWritten(address: string): void };
  clearLinkedMark(): void;
  forgetLinkJumps(): void;
  backHistory(): { state: unknown; replaceState(state: unknown, title: string, url: string): void } | null;
  location(): { pathname?: string } | null;
  stopChannelStream(): void;
}

/** Sign-in, as the page calls it. */
export interface SignIn {
  signIn(): Promise<boolean>;
  saveToken(): Promise<void>;
  forgetToken(): void;
  setTokenState(text: string): void;
  noTokenSentence(): string;
}

/** Make sign-in from the page's owners, handed in as they stand. */
export function createSignIn(deps: SignInDependencies): SignIn {
  function setTokenState(text: string): void {
    deps.el("token-state").textContent = text;
  }

  // What the sign-in screen says when there is nothing stored. It carries the INSTRUCTION, not just
  // the fact, because this is now the only place a first-time visitor is told what to do — the
  // load-time status toast that used to say it was a message over a screen that was already
  // asking. `#63 status-line-placement`.
  const NO_TOKEN_YET = "no token saved in this browser — paste your write-scope token above.";

  // The same fact, for the one browser that can explain WHY it is being asked. `#63
  // status-line-placement` put the instruction here rather than in a toast; this keeps it one
  // sentence and adds the cause, because "you were signed out" and "this deployment is broken" look
  // identical from the sign-in screen and only one of them is worth a message to the owner.
  const NO_TOKEN_AFTER_RENAME =
    "this deployment was renamed, so the token saved under its old name is no longer read — " +
    "paste your write-scope token above once and it will stick.";

  /** Whichever of the two sentences this browser has earned. */
  const noTokenSentence = (): string => (deps.signedOutByRename() ? NO_TOKEN_AFTER_RENAME : NO_TOKEN_YET);

  async function signIn(): Promise<boolean> {
    if (!deps.token()) {
      deps.showScreen("signin");
      return false;
    }
    let config: ClientConfigLike | null = null;
    try {
      config = await deps.config();
    } catch (error) {
      const failure = error as { refused?: boolean; status?: number; message: string; network?: boolean };
      if (failure.refused) {
        // Whatever this credential read before, it can no longer read it.
        deps.forgetMessages();
        deps.showError(
          failure.status === 403
            ? `That token may read but not post, and this page needs the write-scope one — ` +
                `VIBE_TALK_WRITE_TOKEN rather than VIBE_TALK_READ_TOKEN. The server said: ` +
                `${failure.message}`
            : `This server refused that token: ${failure.message}`
        );
        deps.setStatus(deps.refusalSentence(failure));
        deps.showScreen("signin");
        return false;
      }
      deps.showError(`Signed in, but vibe-talk did not answer: ${failure.message}`);
      deps.showScreen("main");
      // Saved rows stay up, and say they are saved rather than current.
      if (deps.loadFreshAt()) deps.setChannelFreshness(failure.network ? "offline" : "failed");
      // The token was not refused, and no stream will attach until `/client-config` answers: the
      // reopen's read goes now, and the poll it arms is what asks again. `#189 restore-ui-state`.
      deps.payReopenRead();
      return true;
    }
    const drawn = { key: deps.channelContextKey(), threading: deps.threading() };
    deps.applyClientConfig(config as ClientConfigLike);
    deps.reconcileChannelSnapshot(drawn);
    deps.clearError();
    deps.showScreen("main");
    // With no saved shell there was no channel to reopen on until now; with one, the channel went up
    // at load and has waited for this to be read. `#189 restore-ui-state`.
    deps.reopenSavedView();
    deps.payReopenRead();
    // Short, because the invitation itself now lives in the empty transcript — the largest thing on
    // an idle screen — rather than competing for the one strip where a phone shows text worst.
    deps.setStatus("Ready.");
    deps.setState("idle");
    // After the screen is up, never before: restoring is a convenience, and a store that is slow or
    // absent must not hold the interface hostage. Not awaited for the same reason.
    deps.voiceTranscript().loadStoredConversation();
    // `#457 settings-server-side`. The token is proven, so the settings sync may read with it:
    // it could not start at load, where a saved token is only a claim `/client-config` tests.
    deps.startSettingsSync();
    // `#468 send-dedupe`. What this page left unconfirmed, asked of the server once per load.
    deps.settleUnconfirmedSends();
    return true;
  }

  async function saveToken(): Promise<void> {
    deps.clearError();
    // The box is empty once a token has signed in (`#616 composer-no-autofill`), so an empty box with a
    // token saved is a retry of that token, as Save on the box it used to be shown in was.
    const value = deps.field("api-token").value.trim() || deps.token();
    deps.markSave("Saving…", "", true);
    if (!value) {
      deps.markSave(SAVE_LABEL, "", false);
      setTokenState(NO_TOKEN_YET);
      deps.showError("There is nothing to save — paste your write-scope API token first.");
      deps.setStatus("nothing to save");
      return;
    }
    if (value !== deps.token()) {
      deps.stopOutgoingSends();
      // Another credential may not be able to read what this one did.
      deps.forgetMessages();
    }
    deps.credentialStore().keep(value);
    // Read it back rather than assuming: private browsing and a full quota both make setItem throw
    // or silently do nothing, and "saved" would then be a lie the owner only discovers later.
    if (deps.token() !== value) {
      deps.markSave(SAVE_LABEL, "", false);
      setTokenState(NO_TOKEN_YET);
      deps.showError(
        "This browser refused to store the token. Private browsing and a full storage quota both " +
          "do this; the token was NOT saved."
      );
      return;
    }
    deps.markSave("Saved ✓", "ok", false);
    setTokenState("token saved in this browser");
    deps.setStatus("ready");
    deps.revertSaveLater(() => {
      deps.markSave(SAVE_LABEL, "", false);
    }, 2500);
    // Only once the server took it: a refused token stays in the box, to be corrected, not pasted again.
    if (await signIn()) deps.field("api-token").value = "";
  }

  function forgetToken(): void {
    // `#457 settings-server-side`. The next credential is unproven until `signIn` proves it,
    // and the archive was agreed under this one: both go, and the settings read fresh.
    deps.settingsSync().forget();
    deps.clearError();
    // `#462 webapp-deep-links`: signing out forgets where the reader was, address included — a
    // reload after it must not reopen the signed-out reader's place from the query alone.
    deps.deepLinks.signOut();
    deps.clearLinkedMark();
    // `#543 in-app-message-links`: and the jumps taken by links, whose spots are the signed-out reader's.
    deps.forgetLinkJumps();
    try {
      const history = deps.backHistory();
      const loc = deps.location();
      if (history && typeof window === "object" && loc) {
        history.replaceState(history.state, "", loc.pathname || "/");
        deps.deepLinks.noteWritten(loc.pathname || "/");
      }
    } catch (_error) {
      // The address keeps its query; the token is still forgotten.
    }
    // Before the token goes, not after: the stream holds a bearer credential open, and a signed-out
    // page that is still receiving one channel's messages is the leak this control exists to close.
    deps.stopChannelStream();
    deps.stopOutgoingSends();
    deps.forgetMessages();
    deps.credentialStore().forget();
    deps.field("api-token").value = "";
    deps.markSave(SAVE_LABEL, "", false);
    setTokenState(NO_TOKEN_YET);
    deps.setStatus("token forgotten");
    deps.showScreen("signin");
  }

  return { signIn, saveToken, forgetToken, setTokenState, noTokenSentence };
}
