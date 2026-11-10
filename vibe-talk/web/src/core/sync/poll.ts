// When the channel is read again on its own (`#617 cold-load-first-paint` and the poll; `#624
// voice-page-modularity`, moved from the legacy script; plan step 14): the poll's wait, what a
// stand-in read leaves of it, and the timer that arms it, held while the page is hidden (`createChannelPoll`,
// `#226 page-energy-profile`). The poll itself — what it reads — is the page's.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long between the channel's own reads, the authoritative fallback behind any pushed change. */
const DISCORD_POLL_MS = 45000;

/** The stand-in reads' retries, held between one read landing and the next poll being armed. */
export interface StandInRetries {
  /**
   * A read landed, answered from the server's own copy (`failed`) or not. Answers whether the next poll
   * should come sooner than the normal cadence: never (`#1227 poll-storm-stale-thread`). The server
   * schedules its own upstream refreshes and pushes what they bring; a stale answer only marks the
   * view stale, it never shortens the poll, so a failing upstream route is not kept busy by the
   * reads waiting on it.
   */
  noteRead(failed: boolean): boolean;
  /** The wait for a poll armed for `delay`: always the normal cadence's wait, never shortened. */
  take(delay: number): number;
}

/** No quick retries: a stand-in answer changes when the view says it is stale, not when it is read again. */
export function createStandInRetries(): StandInRetries {
  return {
    noteRead() {
      return false;
    },
    take(delay) {
      return delay;
    },
  };
}

/** What the poll's timer asks of the page. */
export interface ChannelPollDependencies {
  readonly clock: ClockPort;
  /** The clock's time, in ms (`Date.now`). */
  now(): number;
  /** Whether the page is out of sight (`document.hidden`). */
  hidden(): boolean;
  /** Whether the channel is the view on screen, the only one polled (`currentView`). */
  onChannel(): boolean;
  /** The wait for a poll armed for `delay`, as a stand-in read leaves it (`StandInRetries.take`). */
  take(delay: number): number;
  /** One poll of the channel on screen, which arms the next (`pollDiscord`). */
  poll(): void;
}

/** The poll's timer: armed, stopped, held while hidden, and taken up again. */
export interface ChannelPoll {
  /**
   * Self-rescheduling rather than setInterval, for two reasons: a slow fetch can never stack a second one
   * up behind it, and the next delay is armed only once the previous poll has actually finished. Polling
   * stops the moment the channel stops being the visible view, so a voice call is never sharing its
   * network with a background refresh nobody is looking at — and, `#226 page-energy-profile`, while the
   * page is hidden: a poll armed or come due then waits for the page to be shown (`resume`). `delay` is the
   * rest of a wait a glance away interrupted.
   */
  schedule(delay?: number): void;
  /** Stop the poll, and forget a wait held for the page to be shown. */
  stop(): void;
  /** Whether a poll is armed. */
  armed(): boolean;
  /** The page went out of sight: an armed poll stops, and waits for it to be shown. */
  pause(): void;
  /** Whether a poll armed, or come due, while the page was hidden waits for it to be shown. */
  paused(): boolean;
  /** When the poll last armed was due, by the clock. */
  dueAt(): number;
  /**
   * Take up the poll a hidden page paused: `"now"`, when it came due meanwhile; `"fresh"`, a whole wait,
   * behind a read coming back has just made; otherwise the rest of the wait it was in, so that glancing
   * away again and again does not put the poll off for good.
   */
  resume(how: "now" | "fresh" | "rest"): void;
}

/** Nothing armed. */
export function createChannelPoll(deps: ChannelPollDependencies): ChannelPoll {
  let discordPollTimer: TimerHandle | null = null;
  /** Whether a poll was armed, or came due, while the page was hidden, and waits for it to be shown. */
  let discordPollPaused = false;
  /** When the poll last armed was due, by the clock. */
  let discordPollDueAt = 0;

  function stopDiscordPolling(): void {
    discordPollPaused = false;
    if (discordPollTimer !== null) {
      deps.clock.clearTimeout(discordPollTimer);
      discordPollTimer = null;
    }
  }

  function scheduleDiscordPoll(delay = DISCORD_POLL_MS): void {
    stopDiscordPolling();
    delay = deps.take(delay);
    discordPollDueAt = deps.now() + delay;
    if (deps.hidden()) {
      discordPollPaused = true;
      return;
    }
    discordPollTimer = deps.clock.setTimeout(() => {
      discordPollTimer = null;
      if (!deps.onChannel()) return;
      if (deps.hidden()) {
        discordPollPaused = true;
        return;
      }
      deps.poll();
    }, delay);
  }

  return {
    schedule: scheduleDiscordPoll,
    stop: stopDiscordPolling,
    armed: () => discordPollTimer !== null,
    pause() {
      if (discordPollTimer !== null) {
        stopDiscordPolling();
        discordPollPaused = true;
      }
    },
    paused: () => discordPollPaused,
    dueAt: () => discordPollDueAt,
    resume(how) {
      if (!discordPollPaused) return;
      discordPollPaused = false;
      if (!deps.onChannel()) return;
      if (how === "now") deps.poll();
      else scheduleDiscordPoll(how === "fresh" ? DISCORD_POLL_MS : Math.max(0, discordPollDueAt - deps.now()));
    },
  };
}

export { DISCORD_POLL_MS };
