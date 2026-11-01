// Signing a device in from a pairing link (`#369 pairing-link`; `#624 voice-page-modularity`, moved from
// the legacy script; plan step 16, view/settings).
//
// A pairing link is the app's address with `#pair=CODE`: a single-use code, good for a few minutes, that
// the server trades once for the write token. The page takes the code out of the address before anything
// else runs (its `takePairingCode`) and hands it here. It is redeemed only when the person taps Sign this
// device in (`redeem`), never on load: a link scanner that runs pages would otherwise spend it, and hold
// the token. The request carries NO bearer token, even on a browser that has one: the code is the
// credential. A failure is said plainly and leaves whatever this browser already held alone, so a stale
// link opened on a phone that was signed in leaves it signed in.
//
// Signing in, the token, the screens and the request itself are the page's; the words are
// web/src/present/settings's.

import { deepLinkParams } from "../../core/navigation/index.ts";
import {
  PAIRING_PARTIAL,
  PAIRING_UNREACHED,
  PAIRING_WRONG_LINK,
  pairingCodeShaped,
  pairingRefusedWords,
  pairingTransferAddress,
} from "../../present/settings/index.ts";

/** The server's answer to a redemption, as far as the offer reads it. */
interface RedeemAnswer {
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
}

/** What the offer asks of the page. */
export interface PairingDependencies {
  /** The code the page was opened with, already out of the address, or null (`takePairingCode`). */
  readonly code: string | null;
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** Trade `code` for the write token, with no bearer token on the request (`POST /api/v1/pair/redeem`). */
  redeem(code: string): Promise<RedeemAnswer>;
  /** Save the token in the sign-in box and sign in with it, as Save does (`saveToken`). */
  saveToken(): Promise<void>;
  /** Sign in with the saved token, saying whether the server took it (`signIn`). */
  signIn(): Promise<boolean>;
  /** Whether this browser holds a token (`token`). */
  hasToken(): boolean;
  /** Show the saved channel, if this device kept one for the token, saying whether it did (`hydrateFromCache`). */
  hydrateFromCache(): boolean;
  /** Put the reader back where they were (`reopenSavedView`). */
  reopenSavedView(): void;
  /** Show one of the page's screens (`showScreen`). */
  showScreen(name: string): void;
  /** The page's error panel and status line (`showError`, `clearError`, `setStatus`). */
  showError(text: string): void;
  clearError(): void;
  setStatus(text: string): void;
  /** A call whose failure is reported rather than thrown (`guardQuietly`). */
  guard(run: () => Promise<unknown>): () => void;
}

/** The offer's read surface and named commands. */
export interface Pairing {
  /** The code waiting to be redeemed, or null. */
  code(): string | null;
  /** The sign-in screen, offering to redeem the pairing link this page was opened from. */
  offer(): void;
  /** Not now: the link is left unspent, and the page carries on as it would have without it. */
  decline(): void;
  /** Trade the code for the write token, then sign in with it exactly as Save does. */
  redeem(): Promise<boolean>;
  /** Redeem a pasted link or code inside the installed app's own storage, without navigating back to Safari. */
  redeemInput(): Promise<void>;
  /** A second link opened in a tab already showing the app: its code, taken from the address as at load. */
  opened(code: string): void;
}

/** Holding the code the page was opened with, nothing on its way. Made where the page takes it. */
export function mountPairing(deps: PairingDependencies): Pairing {
  let pairingCode: string | null = deps.code;
  /** Whether a redemption is waiting for the server's answer. */
  let pairingInFlight = false;

  function offer(): void {
    deps.showScreen("signin");
    deps.lookup("pair-offer").hidden = false;
    (deps.lookup("pair-transfer") as HTMLInputElement).value = pairingTransferAddress(window.location.origin, pairingCode);
    (deps.lookup("pair-confirm") as HTMLButtonElement).disabled = false;
    (deps.lookup("pair-dismiss") as HTMLButtonElement).disabled = false;
    deps.lookup("pair-confirm").focus();
  }

  /** Say why pairing failed, and carry on as this browser would have without the link. */
  async function failed(sentence: string): Promise<boolean> {
    deps.setStatus("pairing failed");
    if (deps.hasToken()) {
      // Signed in before: still signed in. The sentence goes up after, because signing in clears
      // the error panel on its way to the main screen.
      await deps.signIn();
    } else {
      deps.showScreen("signin");
    }
    deps.showError(sentence);
    return false;
  }

  async function redeem(): Promise<boolean> {
    const code = pairingCode;
    if (code === null) return false;
    deps.clearError();
    deps.setStatus("Pairing this device…");
    // Both, until the server answers: a second tap would be refused as used, and Not now would hide
    // an offer whose answer is about to sign the browser in anyway.
    (deps.lookup("pair-confirm") as HTMLButtonElement).disabled = true;
    (deps.lookup("pair-dismiss") as HTMLButtonElement).disabled = true;
    pairingInFlight = true;
    let response: RedeemAnswer | null = null;
    try {
      response = await deps.redeem(code);
    } catch (_error) {
      pairingInFlight = false;
      // The code was not spent: the offer stays, to be tapped again once the connection is back.
      (deps.lookup("pair-confirm") as HTMLButtonElement).disabled = false;
      (deps.lookup("pair-dismiss") as HTMLButtonElement).disabled = false;
      deps.showError(PAIRING_UNREACHED);
      deps.setStatus("pairing failed");
      return false;
    }
    pairingInFlight = false;
    // Answered: spent or refused, the code is done with either way. A newer link opened meanwhile
    // (`opened`) is not: it is kept, and offered again if this one did not sign the browser in.
    const newer = pairingCode !== code ? pairingCode : null;
    pairingCode = null;
    deps.lookup("pair-offer").hidden = true;
    (deps.lookup("pair-transfer") as HTMLInputElement).value = "";
    let payload: { readonly token?: unknown } | null = null;
    try {
      payload = JSON.parse(await response.text());
    } catch (_error) {
      payload = null;
    }
    if (response.ok && payload && typeof payload.token === "string" && payload.token) {
      (deps.lookup("api-token") as HTMLInputElement).value = payload.token;
      await deps.saveToken();
      return true;
    }
    const result = await failed(pairingRefusedWords(response.status));
    if (newer !== null) {
      pairingCode = newer;
      offer();
    }
    return result;
  }

  return {
    code: () => pairingCode,
    offer,
    decline() {
      pairingCode = null;
      (deps.lookup("pair-transfer") as HTMLInputElement).value = "";
      deps.lookup("pair-offer").hidden = true;
      if (deps.hasToken()) {
        if (deps.hydrateFromCache()) {
          deps.showScreen("main");
          deps.reopenSavedView();
        }
        deps.guard(() => deps.signIn())();
      }
    },
    redeem,
    async redeemInput() {
      if (pairingInFlight) return;
      const value = (deps.lookup("pair-input") as HTMLInputElement).value.trim();
      let code = value;
      if (value.includes("://")) {
        try {
          const link = new URL(value);
          if (link.origin !== window.location.origin) throw new Error("different server");
          code = deepLinkParams(link.hash.slice(1)).get("pair") || "";
        } catch (_error) {
          deps.showError(PAIRING_WRONG_LINK);
          return;
        }
      }
      if (!pairingCodeShaped(code)) {
        deps.showError(PAIRING_PARTIAL);
        return;
      }
      (deps.lookup("pair-input") as HTMLInputElement).value = "";
      pairingCode = code;
      offer();
      await redeem();
    },
    opened(code) {
      pairingCode = code;
      // While a redemption is answering, the buttons stay as they are; its answer decides what next.
      if (pairingInFlight) return;
      deps.clearError();
      offer();
    },
  };
}
