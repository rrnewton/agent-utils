// The card moving up as its thread opens (`#298 thread-card-header`; `#624 voice-page-modularity`, moved
// from the legacy script into view/channel): the card at the top of a thread just opened started where its
// card in the list was and let slide to its place, the thread fading in under it, the line from the root
// following it up, and each ended by its own animation or, should the browser never say, by a timer. Where
// the card in the list was and how long the stylesheet says it takes are the page's; why it is a FLIP and
// when it never runs are told where the legacy script opens the section.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** What the flight asks of the page. */
export interface CardFlightDependencies {
  readonly clock: ClockPort;
  /** How long the card takes, in ms: the stylesheet's `--card-flight`, 0 for less motion (`cardFlightMs`). */
  ms(): number;
  /** Whether `node` and everything it is in are drawn (`drawnOnPage`). */
  drawn(node: HTMLElement): boolean;
  /** Whether the lines between the two columns are drawn (`columnLinksShown`). */
  linksShown(): boolean;
  /** Draw the lines between the columns now (`drawColumnLinks`). */
  drawLinks(): void;
  /** Draw them at the next frame (`scheduleColumnLinks`). */
  scheduleLinks(): void;
  /** Run `run` at the next frame (`requestAnimationFrame`), or answer false where the browser has no frames. */
  frame(run: () => void): boolean;
}

/** The flight's commands. */
export interface CardFlight {
  /**
   * Move `card`, at the top of a thread just opened, up from `from`, where its card in the list was, and
   * fade in `pane`, the thread under it. Nothing without a place to come from, with no time to do it in,
   * with the card not drawn, or with nowhere to go.
   */
  flyThreadCard(card: HTMLElement, from: { readonly left: number; readonly top: number } | null, pane: HTMLElement): void;
  /**
   * The browser says `node`'s animation ended: the card at the top is at its place, or the thread has
   * faded in. Each ends its own part; the second to end ends the flight. An animation of something in it
   * is not its own.
   */
  endCardFlightPart(node: HTMLElement, event: { readonly target?: unknown } | null | undefined): void;
  /** End the card's move and the thread's fade, if they are under way, wherever they have got to. */
  endCardFlight(): void;
}

/** Nothing moving. */
export function createCardFlight(deps: CardFlightDependencies): CardFlight {
  /**
   * The card moving up while it is: the card at the top, the pane fading in under it, and the timer that
   * ends both should the browser never say their animations ended — a thread closed as it opens, its
   * card no longer drawn. Null with none.
   */
  let cardFlight: { card: HTMLElement; pane: HTMLElement; timer: TimerHandle } | null = null;

  function flyThreadCard(card: HTMLElement, from: { readonly left: number; readonly top: number } | null, pane: HTMLElement): void {
    // A card still moving from the last thread stops where it is going: this one starts afresh.
    endCardFlight();
    if (!from || !deps.drawn(card)) return;
    const ms = deps.ms();
    if (!(ms > 0)) return;
    const to = card.getBoundingClientRect();
    const dx = from.left - to.left;
    const dy = from.top - to.top;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    card.style.setProperty("--flight-x", `${dx}px`);
    card.style.setProperty("--flight-y", `${dy}px`);
    card.setAttribute("data-flying", "");
    pane.setAttribute("data-arriving", "");
    // The fade under it (180 ms after 60) ends inside this too, at the stylesheet's own length.
    cardFlight = { card, pane, timer: deps.clock.setTimeout(endCardFlight, ms + 100) };
    // The line from the root follows the card up, each frame it moves.
    if (!deps.linksShown()) return;
    const follow = (): void => {
      if (!card.hasAttribute("data-flying")) return;
      deps.drawLinks();
      deps.frame(follow);
    };
    deps.frame(follow);
  }

  function endCardFlightPart(node: HTMLElement, event: { readonly target?: unknown } | null | undefined): void {
    const flight = cardFlight;
    if (!flight || !event || event.target !== node) return;
    if (node === flight.card) {
      node.removeAttribute("data-flying");
      // Drawn once more where the card came to rest.
      deps.scheduleLinks();
    } else if (node === flight.pane) {
      node.removeAttribute("data-arriving");
    } else {
      return;
    }
    if (!flight.card.hasAttribute("data-flying") && !flight.pane.hasAttribute("data-arriving")) {
      deps.clock.clearTimeout(flight.timer);
      cardFlight = null;
    }
  }

  function endCardFlight(): void {
    const flight = cardFlight;
    if (!flight) return;
    cardFlight = null;
    deps.clock.clearTimeout(flight.timer);
    flight.card.removeAttribute("data-flying");
    flight.pane.removeAttribute("data-arriving");
    deps.scheduleLinks();
  }

  return { flyThreadCard, endCardFlightPart, endCardFlight };
}
