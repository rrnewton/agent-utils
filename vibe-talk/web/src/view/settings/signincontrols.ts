// Signing in, and pairing a device by its link (`#616 composer-no-autofill`, installed-app pairing; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/settings for the plan's step 17): the form
// that is never submitted, Save and Forget for the token, the pairing code's Sign in, the link's offer confirmed or
// declined, and a second link opened in a tab already showing the app. What each does is the page's; mounted once,
// in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface SignInControlsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The token saved (an empty field tries the saved one again), and forgotten. */
  saveToken(): unknown;
  forgetToken(): unknown;
  /** A pasted pairing code or link redeemed; the link's offer confirmed, or declined. */
  redeemPairingInput(): unknown;
  redeemPairing(): unknown;
  declinePairing(): unknown;
  /** The pairing code in the address, taken out of it, if there is one; and that code offered. */
  takePairingCode(): string | null;
  pairingOpened(code: string): unknown;
}

/** The listeners, mounted. */
export interface SignInControls {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount sign-in's and pairing's listeners in the page's order. */
export function mountSignInControls(deps: SignInControlsDependencies): SignInControls {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  // The form is never submitted; should something submit it, nothing happens.
  listen<Event>(el("signin-form"), "submit", (event) => event.preventDefault());
  listen<Event>(el("save-token"), "click", () => deps.saveToken());
  listen<Event>(el("forget-token"), "click", () => deps.forgetToken());
  listen<Event>(el("pair-submit"), "click", () => deps.redeemPairingInput());
  listen<Event>(el("pair-confirm"), "click", () => deps.redeemPairing());
  listen<Event>(el("pair-dismiss"), "click", () => deps.declinePairing());
  // A second link opened in a tab already showing the app is a navigation within the page: the script does not run
  // again, so the fragment is taken here, as at load, and offered.
  listen<Event>(window, "hashchange", () => {
    const code = deps.takePairingCode();
    if (code === null) return;
    deps.pairingOpened(code);
  });
  return { unmount };
}
