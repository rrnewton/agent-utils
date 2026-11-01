// What Settings' channel box and Service picker say (`#355 provider-grouped-channels`, `#349
// channel-settings-all`, `#374 registered-writable-follows-policy`; `#624 voice-page-modularity`, moved
// from the legacy script into present/settings). Which service is showing, and the channel list, are
// the page's.

/** One service as the server describes it, as far as its line under the Service picker reads it. */
interface ServiceDescription {
  readonly live_delivery?: unknown;
  readonly live_poll_seconds?: unknown;
  readonly threading_supported?: unknown;
  readonly channel_registration_supported?: unknown;
  readonly channel_discovery_supported?: unknown;
  readonly upstream_clear_supported?: unknown;
}

/** What the chosen service, called `name`, does here, as one line under the Service picker. */
function serviceFacts(provider: ServiceDescription, name: string): string {
  const seconds = Number(provider.live_poll_seconds) || 0;
  const arrive = provider.live_delivery === "push"
    ? "New messages arrive as they are sent."
    : provider.live_delivery === "poll" && seconds > 0
      ? `New messages are checked for every ${seconds} seconds.`
      : "New messages show when a channel is read again.";
  const threads = provider.threading_supported === true ? "Threads are shown." : "No threads.";
  const adding = provider.channel_registration_supported === true
    ? `Add a channel by pasting its link${provider.channel_discovery_supported === true ? ", or browse its channels" : ""}.`
    : "Add a channel by its ID.";
  const clear = provider.upstream_clear_supported === true
    ? `Unread can be cleared in ${name} itself.`
    : `Unread cannot be cleared in ${name} from here.`;
  return `${name}: ${arrive} ${threads} ${adding} ${clear}`;
}

/** The line under the Service picker on "Not connected": the channels of no service this server reads. */
const STRAY_SERVICE_FACTS = "Channels on your list whose chat service this server no longer reads. They can be renamed here.";

/**
 * The add form as the service `service` takes a channel: by a link or reference (`byLink`), or by an id,
 * which with several services is not always a snowflake (`anyId`: a Slack that does not register
 * channels takes its own ids), so the field asks for "the channel's ID" there.
 */
function addChannelForm(byLink: boolean, anyId: boolean, service: string): {
  readonly help: string; readonly label: string; readonly inputMode: string; readonly maxLength: number; readonly placeholder: string;
} {
  return {
    help: byLink
      ? `Paste a channel link or reference from ${service}. The channel is checked before it is added.`
      : `Enter the channel ID supplied by this deployment. The chat connector must already have access to the channel in ${service}.`,
    label: byLink ? "Channel link or provider reference" : "Channel ID",
    inputMode: byLink ? "url" : anyId ? "text" : "numeric",
    maxLength: byLink ? 2048 : anyId ? 64 : 20,
    placeholder: byLink ? "paste a channel link or reference" : anyId ? "the channel's ID" : "17 to 20 digits",
  };
}

/**
 * `#355 provider-grouped-channels`. What Settings says for a service, called `serviceName`, with no
 * channel on the list: how to get one, by adding, browsing (`browse`) or showing a hidden one of its
 * own again (`hiddenHere`).
 */
function emptyServiceWords(browse: boolean, hiddenHere: boolean, serviceName: string): string {
  const ways = ["Add one"];
  if (browse) ways.push("browse");
  if (hiddenHere) ways.push("show a hidden one again below");
  const last = ways.pop() as string;
  const said = ways.length ? `${ways.join(", ")}, or ${last}` : last;
  return `No ${serviceName} channels on your list yet. ${said}.`;
}

/** `#349 channel-settings-all`. The channel box's line for All channels: `count` of the chosen service's, or of all. */
const allChannelsFacts = (count: number, serviceName: string | null): string => serviceName !== null
  ? `All ${count} ${serviceName} channels on your list.`
  : `All ${count} channels on your list.`;

/** A channel, as far as why it may not be posted to reads it. */
interface WriteSource {
  readonly write_source?: unknown;
  readonly writable?: unknown;
}

/**
 * Why a channel may not be posted to, or that posting follows a service's policy (called `service`),
 * by whose setting `writable` is. `#374 registered-writable-follows-policy`. An older server sends no
 * `write_source`, and then nothing is said beyond what was said before.
 */
function writeSourceWords(channel: WriteSource, service: string): string {
  switch (channel.write_source) {
    case "provider_policy":
      return channel.writable
        ? `Posting follows ${service}'s policy for channels added in this app, which lets them be posted to (registered_channels_writable in the server's configuration).`
        : `Read only: ${service} adds every channel in this app read only (registered_channels_writable in the server's configuration).`;
    case "configuration":
      return channel.writable ? "" : "Read only: the server's configuration file lists it read only.";
    case "added":
      return channel.writable ? "" : "Read only: it was added read only, as chosen or as the chat service answered then, and keeps that.";
    case "provider_gone":
      return "Read only: the chat service it was added through is no longer configured to add channels on this server.";
    default:
      return "";
  }
}

export { STRAY_SERVICE_FACTS, addChannelForm, allChannelsFacts, emptyServiceWords, serviceFacts, writeSourceWords };
export type { ServiceDescription, WriteSource };
