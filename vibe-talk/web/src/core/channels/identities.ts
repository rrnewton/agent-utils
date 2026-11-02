// Who is speaking in the channel (`#323 slack-system-bots`, `#402 who-is-who-channel-box`, `#536
// mark-own-messages-read`; `#624 voice-page-modularity`, moved from the legacy script into
// core/channels). The one owner of what the page knows about accounts: the reader's own choices of who
// is who, everyone seen in each channel, the accounts this server posts as and the owner's own, the
// coding agents each provider names, and the bucket each author falls in. Drawing rows by bucket, and
// Settings' list of who is who, are the page's; they read this owner (Settings through `IdentityReader`).
//
// The voice transcript tells its two speakers apart by side, colour and corner — three signals at
// once — and the channel view told them apart by nothing at all: a wall of identical rows whose
// only distinguishing mark was a display name anyone can set to anything. The owner asked for the
// same treatment here, and for a way to say which account is which.
//
// FOUR BUCKETS, and the two that matter are drawn exactly as the transcript's two speakers:
//
//   me      the owner — INCLUDING the voice bot, which posts on his behalf through this bridge.
//           Drawn as `mine`. That the words reached the channel through an intermediary does not
//           make them somebody else's.
//   coder   the coding agent. Drawn as `theirs`. These are the two halves of the conversation the
//           reader is actually having, which is why they get the transcript's own idiom.
//   human   another person.
//   bot     another bot.
//
// KEYED ON THE AUTHOR SNOWFLAKE, never on the display name. `global_name` is chosen by whoever owns
// the account and can be changed at any moment, and this whole page exists partly because an agent
// once described messages that did not exist — a mapping that a rename silently redirects is the
// same class of defect.

import type { StoragePort } from "../../ports/index.ts";

// PER CHANNEL, and the key is new because the shape is. The census used to be one flat map of
// every account seen anywhere, which made both halves of this section wrong about the word
// "channel" that was already in its heading: Settings listed accounts from channels the reader was
// not looking at, and the sole-other-bot guess below counted bots from them too — so a second bot
// glimpsed once in another channel could stop the coding agent being recognised here. The old key
// is not migrated and is deliberately removed: nothing in it says which channel anything was seen
// in, so there is no honest way to split it, and the census costs one channel read to rebuild.
// `IDENTITY_KEY` is web/src/core/settings's: its sync merges two devices' choices.
const AUTHORS_KEY = "vibe-talk.voice.channel-authors";
const LEGACY_AUTHORS_KEY = "vibe-talk.voice.authors";
const SELF_ID_KEY = "vibe-talk.voice.self-id";

const BUCKETS: readonly string[] = Object.freeze(["me", "coder", "human", "bot"]);

// What each bucket is called in Settings' list of who is who is web/src/present/settings's
// (`BUCKET_LABELS`, `#624 voice-page-modularity`).

/** One account as the census keeps it. */
export interface SeenAuthor {
  readonly name: string;
  readonly bot: boolean;
}

/** Who may be drawn as the coding agent: the named accounts, and whether only those. */
export interface CodingAgents {
  readonly ids: Set<string>;
  readonly namedOnly: boolean;
}

/** A provider description, or client-config itself, as far as the accounts in it go. */
export interface DescribedAccounts {
  readonly coding_agent_ids?: unknown;
  readonly coding_agents_named_only?: unknown;
  readonly self_author_id?: unknown;
  readonly owner_author_id?: unknown;
}

/**
 * The coding agents a provider description, or client-config itself, names.
 */
function codingAgentsFrom(described: DescribedAccounts | null | undefined): CodingAgents {
  const ids = described && Array.isArray(described.coding_agent_ids) ? (described.coding_agent_ids as unknown[]) : [];
  return {
    ids: new Set(ids.map(String)),
    namedOnly: Boolean(described && described.coding_agents_named_only === true),
  };
}

/**
 * What Settings' list of who is who reads (`#402 who-is-who-channel-box`): the census, whose accounts
 * are this bridge's, the buckets and which one an account is in, and whether the reader chose it. The
 * owner below is one; a later Settings move takes this interface rather than reaching for the page.
 */
export interface IdentityReader {
  /** Everyone seen in one channel, as a plain map, whether or not that channel has been read yet. */
  seenIn(channelId: unknown): Readonly<Record<string, SeenAuthor>>;
  /** Whether `id` is an account this server posts as. */
  isSelfId(id: unknown): boolean;
  /** The buckets an account can be put in, in the picker's order. */
  buckets(): readonly string[];
  /** What bucket an author falls in, chosen or guessed. */
  bucketFor(authorId: unknown, isBot: unknown, channelId: unknown): string;
  /** Whether the reader has chosen a bucket for `id`. */
  chosen(id: string): boolean;
}

/** What the who-is-who asks of the page. */
export interface IdentitiesDependencies {
  /** The device's storage (`deviceStorage`). */
  storage(): StoragePort;
  /** The key the reader's own choices are kept under (`IDENTITY_KEY`). */
  readonly identityKey: string;
  /** The provider description for the channel `channelId`, or null when the server did not say. */
  providerOf(channelId: unknown): DescribedAccounts | null;
}

/** The who-is-who's commands, and its questions. */
export interface Identities extends IdentityReader {
  /** Read the choices, the census and the account this server posts as back from the device. */
  load(): void;
  /** Record that this id is the account this bridge posts as. True where that is news: the page redraws. */
  noteSelfAuthor(id: unknown): boolean;
  /** The reader's own account from the configuration (`owner_author_id`), or null. */
  setOwnerAuthorId(id: unknown): void;
  /** Take down `owner_author_ids` from a client configuration, or none from an older server's. */
  learnOwnerIds(list: unknown): void;
  /** The coding agents client-config itself names (`coding_agent_user_ids`). */
  setDeploymentCodingAgents(described: DescribedAccounts | null | undefined): void;
  /** The accounts each described provider speaks as, replacing the ones known before. */
  learnProviderAccounts(providers: readonly DescribedAccounts[]): void;
  /** Whether `id` is one of the owner's own accounts. */
  isOwnerId(id: unknown): boolean;
  /** Who may be drawn as the coding agent in channel `channelId`: its provider's, else the deployment's. */
  codingAgentsOf(channelId: unknown): CodingAgents;
  /** The bucket the reader chose for `id`, or undefined. */
  choiceOf(id: string): string | undefined;
  /** The reader chose a bucket for this account: kept on the device. */
  choose(id: string, bucket: string): void;
  /** Another device said who the accounts are (`#488 slack-app-agents`): its choices replace these. */
  replaceChoices(held: Readonly<Record<string, unknown>>): void;
  /** Remember an author, in the channel they spoke in, so Settings can offer a row for them later. True where that is news. */
  noteAuthor(channelId: unknown, id: unknown, name: unknown, isBot: unknown): boolean;
  /** Keep the census on the device. */
  keepSeen(): void;
}

/** Nothing known until `load`. */
export function createIdentities(deps: IdentitiesDependencies): Identities {
  /**
   * Explicit choices the reader has made, `{ [authorId]: bucket }`. Beats every guess below.
   *
   * KEYED ON THE ACCOUNT AND NOT ON THE CHANNEL, unlike the census beneath it, and the two are
   * deliberately different: what an account IS does not change because it spoke somewhere else, so
   * saying once that a bot is the coding agent must not have to be said again in the next channel
   * it appears in. The census is per channel because "who is in this conversation" genuinely is a
   * question about one conversation. The sentence under the list on the Settings screen says this
   * out loud, because a reader who has just changed channel is entitled to know which of the two
   * they are looking at.
   */
  let identities: Record<string, string> = {};

  /** Everyone seen in each channel, `{ [channelId]: { [authorId]: { name, bot } } }`, so Settings
   *  has rows to offer before the reader has opened that channel in this session. */
  let authorsSeen: Record<string, Record<string, SeenAuthor>> = {};

  /**
   * The snowflake THIS SERVER posts as.
   *
   * Learned rather than configured, and learned for free: every reply this page sends comes back as
   * the `Message` Discord recorded, and the live feed marks a message this server posted with
   * `self_posted`. Either one identifies the bridge's own account by construction — which is the one
   * account whose messages are the owner's own words. No `/users/@me` call, no configuration, and no
   * name matching.
   */
  let selfAuthorId: string | null = null;

  /**
   * The reader's OWN Discord account, when the deployment has been told what it is.
   *
   * Two accounts are the owner's words and both must be drawn as his: the one this bridge posts as,
   * and the one he types into Discord with himself. The first is free — it is readable out of the
   * bot token. The SECOND CANNOT BE DERIVED from anything the server holds, because a bot's account
   * has no relationship to the human reading the channel, so it is configuration
   * (`discord.owner_user_id`) or a per-account choice in Settings, and absent until one of those.
   *
   * Kept separate from `selfAuthorId` rather than folded into it: they are learned in different ways
   * and one of them can be wrong in a way the other cannot.
   */
  let ownerAuthorId: unknown = null;

  /**
   * The accounts this server posts as, and the owner's own accounts, at EVERY configured provider.
   *
   * A deployment can read several chat services at once, and each has its own account ids, so "me"
   * is a set rather than one id. Ids from different services cannot be confused for one another,
   * which is what makes one set safe for every channel. Filled from client-config's `providers`; an
   * older server sends none and only the two single ids above apply.
   */
  let providerSelfIds = new Set<string>();
  let providerOwnerIds = new Set<string>();

  /**
   * The accounts the configuration names as the coding agents (`coding_agent_user_ids`), and whether
   * only those — and the reader's own choices in Settings — may be drawn as the coding agent, for a
   * deployment that describes no providers one by one. `#323 slack-system-bots`. A provider that is
   * described carries its own (`codingAgentsOf`).
   */
  let deploymentCodingAgents: CodingAgents = { ids: new Set(), namedOnly: false };

  /**
   * `#536 mark-own-messages-read`. Every account the server's configuration names as the owner's
   * (`owner_author_ids`: `[owner] user_ids` and each provider's `owner_user_id`). Before it was sent an
   * account only `[owner] user_ids` named was drawn as a person here while the server took it to be his.
   */
  let configuredOwnerIds = new Set<string>();

  function persistJson(key: string, value: unknown): void {
    try {
      deps.storage().setItem(key, JSON.stringify(value));
    } catch (_error) {
      // Shared origin storage, and this is a presentation preference: a browser that refuses it
      // still gets the guesses below, which are what an unconfigured reader sees anyway.
    }
  }

  function isSelfId(id: unknown): boolean {
    return Boolean(id) && ((Boolean(selfAuthorId) && id === selfAuthorId) || providerSelfIds.has(id as string));
  }

  function isOwnerId(id: unknown): boolean {
    return Boolean(id) &&
      ((Boolean(ownerAuthorId) && id === ownerAuthorId) || providerOwnerIds.has(id as string) ||
        configuredOwnerIds.has(id as string));
  }

  function codingAgentsOf(channelId: unknown): CodingAgents {
    const provider = deps.providerOf(channelId);
    return provider ? codingAgentsFrom(provider) : deploymentCodingAgents;
  }

  function seenIn(channelId: unknown): Record<string, SeenAuthor> {
    return authorsSeen[String(channelId || "")] || {};
  }

  /**
   * What bucket this author falls in, and why, in priority order.
   *
   * The guesses are exactly the ones the owner described, and they are GUESSES — every one of them
   * is overridden by an explicit choice, and Settings shows what was guessed so a wrong one is
   * visible rather than merely wrong.
   *
   * The coding agent is guessed only where the server allows it: an account the configuration names
   * is the coding agent, and where the configuration names any, or the channel is Slack's, no other
   * bot is guessed to be one (`#323 slack-system-bots`).
   *
   * WHICH CHANNEL is an argument because the last guess is a census, and a census is only meaningful
   * about one conversation. Callers pass the channel they are drawing: the message list passes the
   * one it is showing, Settings passes the one its picker is on. They can legitimately differ, and
   * when they do each is right about its own screen.
   */
  function bucketFor(authorId: unknown, isBot: unknown, channelId: unknown): string {
    const id = String(authorId || "");
    // 1. The reader said so.
    if (identities[id]) {
      return identities[id] as string;
    }
    // 2. It is us. Known by construction, not by name — either account the owner speaks through:
    //    the bridge posting on his behalf, or the one he types into Discord with.
    if (isSelfId(id)) {
      return "me";
    }
    if (isOwnerId(id)) {
      return "me";
    }
    if (!isBot) {
      // 3. Not a bot, so a person. Which person is not something this page can know.
      return "human";
    }
    // 4. A bot the configuration names as a coding agent at this channel's provider
    //    (`coding_agent_user_ids`). `#323 slack-system-bots`.
    const agents = codingAgentsOf(channelId);
    if (agents.ids.has(id)) {
      return "coder";
    }
    // 5. Where only a named account may be the coding agent — the configuration names some, or the
    //    provider is Slack, whose channels carry system bots (a workspace's welcome bot, an
    //    integration) — every other bot is another bot, drawn with its own name and "(bot)". A
    //    lone system bot was drawn as the coding agent: nameless, unlabelled, in the agent's colours.
    if (agents.namedOnly) {
      return "bot";
    }
    // 6. A bot, and if it is the ONLY bot IN THIS CHANNEL that is not us then it is the coding agent
    //    — the owner's own heuristic, and true of every channel this bridge is pointed at so far.
    //    With two or more it is a coin toss, so it stays "another bot" and Settings is where it gets
    //    decided. Scoped to the channel: a bot that has never spoken here says nothing about who the
    //    reader is talking to here, and counting it was how the guess went quiet in a channel that
    //    had exactly one candidate in it.
    const otherBots = Object.entries(seenIn(channelId)).filter(
      ([seen, who]) => who.bot && !isSelfId(seen)
    );
    if (otherBots.length === 1 && (otherBots[0] as [string, SeenAuthor])[0] === id) {
      return "coder";
    }
    return "bot";
  }

  return {
    load() {
      const read = <T>(key: string, fallback: T): T => {
        try {
          const held: unknown = JSON.parse(deps.storage().getItem(key) || "null");
          return held && typeof held === "object" && !Array.isArray(held) ? (held as T) : fallback;
        } catch (_error) {
          return fallback;
        }
      };
      identities = read<Record<string, string>>(deps.identityKey, {});
      authorsSeen = read<Record<string, Record<string, SeenAuthor>>>(AUTHORS_KEY, {});
      // Shed the flat census rather than leaving it to rot in the reader's storage under a key nothing
      // reads any more. Losing it costs one channel read; keeping it would cost an explanation.
      try {
        deps.storage().removeItem(LEGACY_AUTHORS_KEY);
      } catch (_error) {
        // A browser that refuses to remove it is one that refused to store it. Nothing reads it.
      }
      // A channel whose entry is not an object is a corrupt entry, and it would otherwise surface as
      // `Object.keys(undefined)` at the first render rather than as an empty list.
      for (const [channel, seen] of Object.entries(authorsSeen)) {
        if (!seen || typeof seen !== "object" || Array.isArray(seen)) {
          delete authorsSeen[channel];
        }
      }
      // A bucket that is not one of ours would otherwise become a `data-who` nobody styles, which reads
      // as "this row is special" rather than as "this entry is corrupt".
      for (const [id, bucket] of Object.entries(identities)) {
        if (!BUCKETS.includes(bucket)) {
          delete identities[id];
        }
      }
      const held = deps.storage().getItem(SELF_ID_KEY);
      selfAuthorId = held || null;
    },
    noteSelfAuthor(id) {
      const found = String(id || "");
      if (!found || selfAuthorId === found) {
        return false;
      }
      selfAuthorId = found;
      try {
        deps.storage().setItem(SELF_ID_KEY, found);
      } catch (_error) {
        // As above.
      }
      return true;
    },
    setOwnerAuthorId(id) {
      ownerAuthorId = id || null;
    },
    learnOwnerIds(list) {
      configuredOwnerIds = new Set(
        (Array.isArray(list) ? (list as unknown[]) : []).filter((id): id is string => typeof id === "string" && id !== "").map(String));
    },
    setDeploymentCodingAgents(described) {
      deploymentCodingAgents = codingAgentsFrom(described);
    },
    learnProviderAccounts(providers) {
      providerSelfIds = new Set();
      providerOwnerIds = new Set();
      for (const provider of providers) {
        if (provider.self_author_id) providerSelfIds.add(String(provider.self_author_id));
        if (provider.owner_author_id) providerOwnerIds.add(String(provider.owner_author_id));
      }
    },
    seenIn,
    isSelfId,
    isOwnerId,
    buckets: () => BUCKETS,
    bucketFor,
    chosen: (id) => Boolean(identities[id]),
    codingAgentsOf,
    choiceOf: (id) => identities[id],
    choose(id, bucket) {
      identities[id] = bucket;
      persistJson(deps.identityKey, identities);
    },
    replaceChoices(held) {
      identities = held as Record<string, string>;
    },
    noteAuthor(channelId, id, name, isBot) {
      const channel = String(channelId || "");
      const key = String(id || "");
      if (!channel || !key) {
        return false;
      }
      const seen = authorsSeen[channel] || (authorsSeen[channel] = {});
      const held = seen[key];
      if (held && held.name === name && held.bot === isBot) {
        return false;
      }
      seen[key] = { name: String(name || ""), bot: Boolean(isBot) };
      return true;
    },
    keepSeen() {
      persistJson(AUTHORS_KEY, authorsSeen);
    },
  };
}

// What the page and the tests use of it.
export { AUTHORS_KEY, BUCKETS, LEGACY_AUTHORS_KEY, SELF_ID_KEY, codingAgentsFrom };
