// The channel directory (`#39 channel-alias`, `#199 removable-config-channels`, `#19 channel-browser`,
// `#355 provider-grouped-channels`; `#624 voice-page-modularity`, moved from the legacy script into
// core/channels). The one owner of what the server last said the channels and their services were: the
// channels on the reader's list, the configured ones taken off it, each configured chat provider's
// description, the deployment-wide capabilities an older server states for its one provider, whether
// the bridge can register and list channels, and the chat service's own name. What to call a channel,
// grouping channels by service, the pickers and Settings are the page's, which read this owner.

import type { ChannelInfo, ProviderDescription } from "../../contract/index.ts";

/**
 * What the deployment as a whole can do, from client-config's flags: the one provider's, for an older
 * server that describes none, and what a channel whose provider is not described falls back to.
 */
export interface DeploymentCapabilities {
  threading: boolean;
  upstreamReadMark: boolean;
  upstreamClear: boolean;
  idempotentPosts: boolean;
  repliesInThread: boolean;
  liveDelivery: string;
  livePollSeconds: number;
}

/** The directory's commands and questions. */
export interface ChannelDirectory {
  /** What the server last said the channels were, including the names the owner gave them. */
  channels(): ChannelInfo[];
  /** The server said so. */
  setChannels(channels: ChannelInfo[]): void;
  /** One channel the server answered with (a rename, an alias): it replaces the one with its id. */
  replaceChannel(updated: ChannelInfo): void;
  /** The channel on the list with id `id`, or null. */
  known(id: unknown): ChannelInfo | null;
  /** Configured channels the owner took off his list, as the server last reported them. */
  hidden(): ChannelInfo[];
  /** The server said so. */
  setHidden(channels: ChannelInfo[]): void;
  /** Each configured chat provider, by key. */
  providers(): Map<string, ProviderDescription>;
  /** Remember every provider's description; the ones kept, in the order given. */
  learnProviders(providers: unknown): ProviderDescription[];
  /** The provider description for the channel `id`, or null when the server did not say. */
  providerOf(id: unknown): ProviderDescription | null;
  /** What the deployment as a whole can do. */
  capabilities(): Readonly<DeploymentCapabilities>;
  /** Client-config, or the saved shell, said what the deployment can do in one respect. */
  setCapability<K extends keyof DeploymentCapabilities>(key: K, value: DeploymentCapabilities[K]): void;
  /** Whether the provider bridge resolves channel links or references into stable channel ids. */
  registrationSupported(): boolean;
  setRegistrationSupported(on: boolean): void;
  /** Whether the bridge can list the channels its account sees. */
  discoverySupported(): boolean;
  setDiscoverySupported(on: boolean): void;
  /** The chat backend's own display name from client-config, or "" from an older server. */
  providerName(): string;
  setProviderName(name: string): void;
}

/** Empty until the saved shell or client-config says otherwise. */
export function createChannelDirectory(): ChannelDirectory {
  /** What the server last said the channels were, including the names the owner gave them. */
  let knownChannels: ChannelInfo[] = [];

  /**
   * Configured channels the owner took off his list, as the server last reported them. `#199
   * removable-config-channels`. Only Settings reads this, to offer each one back; a server that
   * cannot hide one, or a sign-in it was not sent to, leaves it empty.
   */
  let hiddenChannels: ChannelInfo[] = [];

  /** Whether the provider bridge resolves channel links or references into stable channel ids. */
  let channelRegistrationSupported = false;

  /** Whether the bridge can list the channels its account sees. `#19 channel-browser`. */
  let channelDiscoverySupported = false;
  /**
   * The chat backend's own display name from client-config, or "" from an older server. Visible
   * copy names the service the reader recognises, never the connector that implements it.
   */
  let chatProviderName = "";

  /**
   * Each configured chat provider, by key, from client-config's `providers`.
   *
   * A deployment may read several services, and they differ in what they can do: one has threads,
   * another does not; one is polled, another pushed. The page shows ONE channel at a time, so the
   * capability flags it acts on are that channel's provider's, applied whenever the channel changes
   * by the page's `applyChannelProvider`. An older server sends no providers, and the deployment-wide
   * flags it does send are then the one provider's, exactly as before.
   */
  let providerDescriptions = new Map<string, ProviderDescription>();
  const deploymentCapabilities: DeploymentCapabilities = {
    threading: false,
    upstreamReadMark: false,
    upstreamClear: false,
    idempotentPosts: false,
    repliesInThread: false,
    liveDelivery: "off",
    livePollSeconds: 0,
  };

  const known = (id: unknown): ChannelInfo | null =>
    knownChannels.find((channel) => String(channel.id) === String(id)) || null;

  return {
    channels: () => knownChannels,
    setChannels(channels) {
      knownChannels = channels;
    },
    replaceChannel(updated) {
      knownChannels = knownChannels.map((channel) =>
        String(channel.id) === String(updated.id) ? updated : channel
      );
    },
    known,
    hidden: () => hiddenChannels,
    setHidden(channels) {
      hiddenChannels = channels;
    },
    providers: () => providerDescriptions,
    learnProviders(providers) {
      providerDescriptions = new Map();
      const described: ProviderDescription[] = [];
      for (const provider of Array.isArray(providers) ? (providers as unknown[]) : []) {
        const held = provider as ProviderDescription | null;
        if (!held || typeof held.key !== "string") continue;
        providerDescriptions.set(held.key, held);
        described.push(held);
      }
      return described;
    },
    providerOf(id) {
      const channel = knownChannels.find((candidate) => String(candidate.id) === String(id));
      const key = channel && typeof channel.provider === "string" ? channel.provider : null;
      return (key && providerDescriptions.get(key)) || null;
    },
    capabilities: () => deploymentCapabilities,
    setCapability(key, value) {
      deploymentCapabilities[key] = value;
    },
    registrationSupported: () => channelRegistrationSupported,
    setRegistrationSupported(on) {
      channelRegistrationSupported = on;
    },
    discoverySupported: () => channelDiscoverySupported,
    setDiscoverySupported(on) {
      channelDiscoverySupported = on;
    },
    providerName: () => chatProviderName,
    setProviderName(name) {
      chatProviderName = name;
    },
  };
}
