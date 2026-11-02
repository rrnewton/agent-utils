// When the channel is read again on its own (`#617 cold-load-first-paint` and the poll; `#624
// voice-page-modularity`, moved from the legacy script; plan step 14): the poll's wait, its quick retries
// after a stand-in read, and the timer that arms it, held while the page is hidden (`createChannelPoll`,
// `#226 page-energy-profile`). The poll itself — what it reads — is the page's.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long between the channel's own reads, the authoritative fallback behind any pushed change. */
const DISCORD_POLL_MS = 45000;

/**
 * After a page the server answered from its own copy, the next reads come this soon, one after the
 * other, rather than at the next poll, forty-five seconds on: the read upstream it stood in for runs
 * on, and has usually landed by then.
 */
const STAND_IN_RETRY_MS = [3000, 10000];

/** The stand-in reads' quick retries, held between one read landing and the next poll being armed. */
export interface StandInRetries {
  /**
   * A read landed, answered from the server's own copy (`failed`) or not. Answers whether the next poll
   * should come after the next quick retry: never once they are spent, and never after a current read,
   * which starts them again.
   */
  noteRead(failed: boolean): boolean;
  /** The wait for a poll armed for `delay`: shortened to the next quick retry when one is wanted, which it spends. */
  take(delay: number): number;
}

/** No quick retries wanted, none spent. */
export function createStandInRetries(): StandInRetries {
  /** How many of `STAND_IN_RETRY_MS` the reads since the last current one have taken. */
  let standInRetries = 0;
  /** Whether the next poll armed should come after the next of `STAND_IN_RETRY_MS`. */
  let standInRetryWanted = false;
  return {
    noteRead(failed) {
      if (failed) {
        standInRetryWanted = standInRetries < STAND_IN_RETRY_MS.length;
      } else {
        standInRetries = 0;
        standInRetryWanted = false;
      }
      return standInRetryWanted;
    },
    take(delay) {
      if (!standInRetryWanted) return delay;
      const wait = Math.min(delay, STAND_IN_RETRY_MS[standInRetries] as number);
      standInRetries += 1;
      standInRetryWanted = false;
      return wait;
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
  /** The wait for a poll armed for `delay`, shortened for a stand-in read's quick retry (`StandInRetries.take`). */
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

export { DISCORD_POLL_MS, STAND_IN_RETRY_MS };
