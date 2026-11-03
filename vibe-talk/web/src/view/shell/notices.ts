// What the page says over every screen (`#63 status-line-placement`, `#33 error-banner-scroll-shift`, `#268
// voice-call-drops`; `#624 voice-page-modularity`, moved from the legacy script into view/shell): the one
// status line, which says a thing that was true a moment ago and takes itself away; the state dot beside it;
// the error panel, which shows a failure in words until it is fixed or for twelve seconds; and the
// connection details, shown once in a banner that takes itself away and kept for Settings. Taking the
// credential out of anything said, and holding the reader as the panel comes and goes, are the page's.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long the status line stays before it takes itself away, in ms. */
const STATUS_DISMISS_MS = 6000;

/** How long the connection banner stays before it takes itself away, in ms. */
const BANNER_DISMISS_MS = 8000;

/** What the notices ask of the page. */
export interface ShellNoticesDependencies {
  readonly clock: ClockPort;
  /** The page's element by id (`el`). */
  el(id: string): HTMLElement;
  /** `text` with any credential this page holds taken out (`redact`). */
  redact(text: unknown): string;
  /** What the status line falls back to while the call's service is not answering, or null while it is. */
  unresponsive(): string | null;
  /** Run `mutate`, the reader held where they are (`holdingReader`). */
  holdingReader(mutate: () => void): void;
  /** The connection details changed: Settings draws them as a readout (`connectionReadouts.detail`). */
  detailChanged(text: string): void;
}

/** The notices' commands. */
export interface ShellNotices {
  /** Say `text` on the status line, until it takes itself away. */
  setStatus(text: unknown): void;
  /** Take the status line away: a hide, not an erase. */
  dismissStatus(): void;
  /** Keep the status line up as it is, its timer let go, until it is said again or taken away: for a picture of it. */
  holdStatus(): void;
  /** The state dot beside the line: idle, working, live, unresponsive, ended, suspended, error. */
  setState(name: string): void;
  /** Show a failure in the error panel: for twelve seconds, or until the reader acts with `stay`. */
  showError(text: unknown, options?: { readonly stay?: boolean }): void;
  /** Take the error panel away. */
  clearError(): void;
  /** Show the connection details in the banner, which takes itself away. */
  showDetail(text: unknown): void;
  /** Add a part to the connection details. */
  addDetail(text: unknown): void;
  /** Take the banner away. */
  dismissBanner(): void;
}

/** Nothing said. */
export function createShellNotices(deps: ShellNoticesDependencies): ShellNotices {
  const el = deps.el;
  let statusTimer: TimerHandle | null = null;
  let errorTimer: TimerHandle | null = null;
  /**
   * The connection details: kept here rather than read back out of the DOM, so appending to them cannot pick
   * up a redaction marker and re-redact it.
   */
  let connectionDetail = "";
  let bannerTimer: TimerHandle | null = null;

  function setStatus(text: unknown): void {
    if (statusTimer !== null) {
      deps.clock.clearTimeout(statusTimer);
    }
    // UN-HIDE FIRST, then write. An `aria-live` region whose element is `display: none` when its
    // text changes announces nothing at all; the order here is what makes it speak.
    el("status-line").hidden = false;
    el("status").textContent = deps.redact(text);
    statusTimer = deps.clock.setTimeout(dismissStatus, STATUS_DISMISS_MS);
  }

  /**
   * A HIDE, NOT AN ERASE, and that is deliberate twice over: the text is still there for anything
   * that wants to know what was last said, and a dozen assertions in the page suite read
   * `#status`'s textContent after the moment it was set.
   */
  function dismissStatus(): void {
    if (statusTimer !== null) {
      deps.clock.clearTimeout(statusTimer);
      statusTimer = null;
    }
    // `voice-unresponsive-signal`: while the service is not answering, the line falls back to
    // saying so rather than to nothing. Hiding it would leave the call looking fine again.
    const unresponsive = deps.unresponsive();
    if (unresponsive !== null) {
      el("status").textContent = unresponsive;
      return;
    }
    el("status-line").hidden = true;
  }

  function setState(name: string): void {
    el("status-line").setAttribute("data-state", name);
  }

  function showError(text: unknown, options?: { readonly stay?: boolean }): void {
    const box = el("error-wrap");
    // Try again is offered by one error only, and it shows it after this; see `reportUnreachable`.
    el("retry-start").hidden = true;
    // So is Read with device voice, by a cloud read that failed (`cloudReadFailed`).
    el("read-on-device").hidden = true;
    deps.holdingReader(() => {
      el("error").textContent = deps.redact(text);
      el("error").hidden = false;
      box.hidden = false;
    });
    setState("error");
    if (errorTimer !== null) deps.clock.clearTimeout(errorTimer);
    errorTimer = options && options.stay ? null : deps.clock.setTimeout(clearError, 12000);
  }

  function clearError(): void {
    const box = el("error-wrap");
    if (errorTimer !== null) deps.clock.clearTimeout(errorTimer);
    errorTimer = null;
    // `#209 voice-socket-open`. Try again belongs to the one error that offers it.
    el("retry-start").hidden = true;
    el("read-on-device").hidden = true;
    // Most callers clear on the way into an action — Read, Talk, sign-in — whether or not anything is
    // showing. With nothing to take away there is nothing to hold, and holding anyway would snap a
    // reader within BOTTOM_SLACK_PX of the newest line down onto it.
    if (box.hidden) return;
    deps.holdingReader(() => {
      el("error").textContent = "";
      el("error").hidden = true;
      box.hidden = true;
    });
  }

  function renderDetail(): void {
    const text = deps.redact(connectionDetail);
    el("connection-detail").textContent = text;
    // `#348 settings-dynamic-lists`. Settings draws it as a readout, one part to a line; the banner
    // keeps the one line it has room for.
    deps.detailChanged(text);
  }

  function dismissBanner(): void {
    if (bannerTimer !== null) {
      deps.clock.clearTimeout(bannerTimer);
      bannerTimer = null;
    }
    el("connection-banner").hidden = true;
  }

  return {
    setStatus,
    dismissStatus,
    holdStatus() {
      if (statusTimer !== null) deps.clock.clearTimeout(statusTimer);
      statusTimer = null;
    },
    setState,
    showError,
    clearError,
    showDetail(text) {
      connectionDetail = String(text);
      renderDetail();
      el("connection-banner").hidden = false;
      if (bannerTimer !== null) {
        deps.clock.clearTimeout(bannerTimer);
      }
      bannerTimer = deps.clock.setTimeout(dismissBanner, BANNER_DISMISS_MS);
    },
    addDetail(text) {
      connectionDetail = `${connectionDetail} · ${text}`;
      renderDetail();
    },
    dismissBanner,
  };
}

// What the page and the tests use of it.
export { BANNER_DISMISS_MS, STATUS_DISMISS_MS };
