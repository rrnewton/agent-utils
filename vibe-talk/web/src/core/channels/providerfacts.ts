// What the selected channel's chat service can do (`#44 live-push`, `#232 upstream-mark-read`; `#624
// voice-page-modularity`, moved from the legacy script into core/channels): whether it has threads, whether it
// can move its own read cursor, how the server delivers its live updates and how often it reads the channel.
// Taken from the channel's own provider where the server describes one, else from the deployment's answer;
// before any answer, nothing (no threads, no cursor, off). Acting on them is the page's.

import type { DeploymentCapabilities } from "./directory.ts";

/** A provider as the server describes it, as far as its facts are read. */
export interface DescribedProvider {
  readonly threading_supported?: unknown;
  readonly upstream_read_mark_supported?: unknown;
  readonly live_delivery?: unknown;
  readonly live_poll_seconds?: unknown;
}

/** The client-config answer, as far as these facts are read. */
export interface FactsConfig {
  readonly live_poll_seconds?: unknown;
  readonly live_delivery?: unknown;
  readonly upstream_read_mark_supported?: unknown;
}

/** The facts' commands and questions. */
export interface ProviderFacts {
  /** Whether the selected channel's service has threads. */
  threading(): boolean;
  setThreading(on: boolean): void;
  /** Whether it can move its own read cursor. Default-off for older servers. */
  upstreamReadMark(): boolean;
  /** `off`, `poll`, or `push`, as reported by the server. */
  liveDelivery(): string;
  /** Seconds between the server's own reads of the channel; 0 means it is not watching at all. */
  livePollSeconds(): number;
  /** The deployment's answer: how it watches channels and whether its service can move its read cursor. */
  fromConfig(config: FactsConfig): void;
  /** The selected channel's own provider, where the server describes one, else the deployment's capabilities. */
  fromProvider(provider: DescribedProvider | null | undefined, capabilities: Readonly<DeploymentCapabilities>): void;
}

/** No answer yet. */
export function createProviderFacts(): ProviderFacts {
  let threadingSupported = false;
  let upstreamReadMarkSupported = false;
  let liveDelivery = "off";
  let livePollSeconds = 0;

  return {
    threading: () => threadingSupported,
    setThreading(on) {
      threadingSupported = on;
    },
    upstreamReadMark: () => upstreamReadMarkSupported,
    liveDelivery: () => liveDelivery,
    livePollSeconds: () => livePollSeconds,
    fromConfig(config) {
      // `#44 live-push`. The server says whether it is watching the channel at all, and how often.
      // Without it the page would have to infer "live" from a stream that is attached and silent —
      // which is exactly what a quiet channel looks like, so the indicator would be a guess.
      livePollSeconds = Number(config.live_poll_seconds) || 0;
      liveDelivery = typeof config.live_delivery === "string" && ["off", "poll", "push"].includes(config.live_delivery)
        ? config.live_delivery
        : livePollSeconds > 0
          ? "poll"
          : "off";
      upstreamReadMarkSupported = config.upstream_read_mark_supported === true;
    },
    fromProvider(provider, capabilities) {
      threadingSupported = provider
        ? provider.threading_supported === true
        : capabilities.threading;
      upstreamReadMarkSupported = provider
        ? provider.upstream_read_mark_supported === true
        : capabilities.upstreamReadMark;
      liveDelivery =
        provider && typeof provider.live_delivery === "string" && ["off", "poll", "push"].includes(provider.live_delivery)
          ? provider.live_delivery
          : capabilities.liveDelivery;
      livePollSeconds = provider
        ? Number(provider.live_poll_seconds) || 0
        : capabilities.livePollSeconds;
    },
  };
}
