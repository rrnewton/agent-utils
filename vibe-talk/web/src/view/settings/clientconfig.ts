// What the server's client-config answer puts into force (`#552 wit-tts-provider`, `#563
// read-aloud-fallback-chain`, `#528 thread-card-markdown-and-cap`, `#408 copy-upstream-link`, `#343
// pwa-app-name`, `#232 upstream-mark-read`, `#457 settings-server-side`, `#355
// provider-grouped-channels`, `#44 live-push`, `#195 send-resilience`, `#252 thread-button-state`,
// `#46 conversation-replay`, `#512 voice-recap-private-stack`, `#427 server-thread-hues`; `#624
// voice-page-modularity`, moved from the legacy script into view/settings for the plan's step 17):
// the read-aloud providers and their chain, threading and provider facts, who is who, the channel
// list and the Settings boxes drawn from it, summaries, replay, and the feature settings applied
// after the token's scope. Every owner it touches is handed in; the order below is the page's.

/** The read-aloud owner, as client-config writes it. */
export interface ReadAloudConfigTarget {
  agentReadAloud: unknown;
  witReadAloud: unknown;
  chainDeviceTail: boolean;
  witVoice: unknown;
  storedWitVoice(): unknown;
  setReadAudioSource(source: string, announce: boolean): void;
  storedReadAudioSource(): string;
  loadWitVoices(): unknown;
  applyReadSpeed(speed: unknown): void;
  readSpeed: unknown;
}

/** The client-config answer, as this owner reads it. */
export interface ClientConfigLike {
  read_aloud?: { backend?: string; label?: string; playback?: string; local_only?: boolean; chain?: string[] } | null;
  threading_supported?: boolean;
  broadcast_supported?: boolean;
  live_poll_seconds?: unknown;
  live_delivery?: unknown;
  upstream_read_mark_supported?: unknown;
  thread_names_supported?: boolean;
  discuss_available?: boolean;
  providers?: unknown;
  self_author_id?: unknown;
  owner_author_id?: unknown;
  owner_author_ids?: unknown;
  conversational_voice?: unknown;
  channels?: unknown[];
  hidden_channels?: unknown;
  channel_registration_supported?: boolean;
  channel_discovery_supported?: boolean;
  upstream_clear_supported?: boolean;
  token_scope?: unknown;
  client_settings_supported?: boolean;
  summaries_unavailable?: string;
  replay_enabled?: boolean;
  transcript_records_supported?: boolean;
  replay_unavailable?: string;
  replay_via_profile?: boolean;
  speech_hold?: unknown;
  idempotent_posts_supported?: boolean;
  replies_in_thread?: boolean;
  speech_prep_enabled?: unknown;
  noise_rules?: unknown;
  voice_settings?: unknown;
  voice_prompt?: unknown;
  voice_post_prefix?: unknown;
  message_cache?: unknown;
  app_name?: string;
  chat_provider_name?: string;
}

export interface ClientConfigDependencies {
  el(id: string): HTMLElement;
  setTitle(title: string): void;
  readAloud(): ReadAloudConfigTarget;
  providerFacts: {
    setThreading(on: boolean): void; threading(): boolean; setBroadcast(on: boolean): void; broadcastSupported(): boolean;
    fromConfig(config: ClientConfigLike): void;
    upstreamReadMark(): boolean; liveDelivery(): string; livePollSeconds(): number;
  };
  channelDirectory: {
    setCapability(name: string, value: unknown): void;
    setProviderName(name: string): void; providerName(): string;
    setChannels(channels: unknown[]): void; setHidden(ids: unknown): void;
    setRegistrationSupported(on: boolean): void; registrationSupported(): boolean;
    setDiscoverySupported(on: boolean): void; channels(): { id: unknown }[];
  };
  serverFacts: {
    setThreadNames(on: boolean): void; setDiscussAvailable(on: boolean): void;
    setTokenScope(scope: unknown): void;
    tokenScope(): unknown; markConfigApplied(): void;
  };
  whoIsWho: {
    setDeploymentCodingAgents(config: ClientConfigLike): void;
    setOwnerAuthorId(id: unknown): void; learnOwnerIds(ids: unknown): void;
  };
  learnProviders(providers: unknown): void;
  noteSelfAuthor(id: unknown): void;
  voiceProvider: { learn(voice: unknown): void; current(): { name: string } };
  voiceCall(): { session: { providerName: string }; speechHold: unknown };
  settingsReadouts: { agentLink(): void; speechPrep(enabled: unknown): void };
  channelSettings: {
    renderHiddenChannels(): void; fillServiceSelect(): void;
    renderAddChannelForm(): void; renderChannelBox(): void;
  };
  fillChannelSelect(id: string): void;
  restoreChannelComposer(): void;
  summaryRows: {
    setDisabledByServer(text: string): void; disabledByServer(): string; mode(): boolean;
  };
  setSummaryMode(on: boolean): void;
  storedSummarizeCollapsed(): boolean;
  renderScrollTools(): void;
  applyChannelProvider(): void;
  upstreamClearBox: { render(): void };
  callResume: { setAllowed(on: boolean): void };
  voiceTranscript(): { transcriptRecordsSupported: boolean; replayUnavailable: string | null; resumeViaProfile: boolean };
  speechHoldFrom(hold: unknown): unknown;
  renderResumeState(): void;
  noiseSettings: { apply(rules: unknown): void };
  agentSettings: { applySendSetting(settings: unknown): void; applyPrompt(prompt: unknown): void };
  agentPrefix: { apply(prefix: unknown): void };
  serverCacheBox: { apply(cache: unknown): void };
  seedServerThreadHues(channelId: string): unknown;
  selectedChannelId(): string;
  startChannelStream(channelId: string): void;
  settingsSync(): { setSupported(on: boolean): void };
  messageCache: { saveCacheShell(config: ClientConfigLike): void };
  uiState: { chosenViewsNow(): { channel: string }[]; forgetChosen(drop: (channel: string) => boolean): void };
  saveUiState(): void;
  keyHints: { render(): void };
}

/** What applying client-config is, to the page. */
export interface ClientConfig {
  apply(config: ClientConfigLike): void;
}

/** Make the client-config applier from the page's owners, handed in as they stand. */
export function createClientConfig(deps: ClientConfigDependencies): ClientConfig {
  function applyClientConfig(config: ClientConfigLike): void {
    const speech = config.read_aloud;
    const effectiveSpeech = speech || {
      backend: "legacy",
      label: "Voice provider",
      playback: "audio",
      local_only: false,
    };
    // Device speech is always a local browser choice. A configured server-audio provider adds the
    // other side of the bar toggle; its label comes from the provider trait and is never guessed.
    deps.readAloud().agentReadAloud = String(effectiveSpeech.playback) === "audio" ? effectiveSpeech : null;
    // `#552 wit-tts-provider`. A Wit.ai backend is server audio too, but
    // Settings names it as its own choice and offers its voice catalogue.
    deps.readAloud().witReadAloud = String(effectiveSpeech.backend || "") === "wit" ? effectiveSpeech : null;
    // `#563 read-aloud-fallback-chain`: the device voice is a chain's
    // last resort only when the configured order ends with it.
    const chainLabels = Array.isArray(effectiveSpeech.chain) ? effectiveSpeech.chain : [];
    deps.readAloud().chainDeviceTail = chainLabels.length > 0 && chainLabels[chainLabels.length - 1] === "Device voice";
    if (deps.readAloud().witReadAloud && !deps.readAloud().witVoice) deps.readAloud().witVoice = deps.readAloud().storedWitVoice();
    deps.readAloud().setReadAudioSource(
      deps.readAloud().storedReadAudioSource() || (deps.readAloud().witReadAloud ? "wit" : deps.readAloud().agentReadAloud ? "agent" : "device"), false);
    if (deps.readAloud().witReadAloud) void deps.readAloud().loadWitVoices();
    deps.readAloud().applyReadSpeed(deps.readAloud().readSpeed);
    deps.providerFacts.setThreading(config.threading_supported === true);
    deps.channelDirectory.setCapability("threading", deps.providerFacts.threading());
    deps.providerFacts.setBroadcast(config.broadcast_supported === true);
    deps.channelDirectory.setCapability("broadcast", deps.providerFacts.broadcastSupported());
    // `#528 thread-card-markdown-and-cap`: whether a long first message's summary may be on its way.
    deps.serverFacts.setThreadNames(config.thread_names_supported === true);
    // `catchup-side-agent`: whether Discuss has a direct LLM summarizer to ask.
    deps.serverFacts.setDiscussAvailable(config.discuss_available === true);
    deps.learnProviders(config.providers);
    deps.whoIsWho.setDeploymentCodingAgents(config);
    // The selected backend owns its display name. An older server leaves it unspecified, so the
    // page stays neutral instead of guessing a platform from channel IDs or deployment details.
    deps.channelDirectory.setProviderName(
      typeof config.chat_provider_name === "string" ? config.chat_provider_name.trim() : "");
    for (const id of ["chat-provider-settings", "chat-provider-help"]) {
      deps.el(id).textContent = deps.channelDirectory.providerName() || "Chat";
    }
    // `#408 copy-upstream-link`: the list of keys was said at first load, before any service was
    // named; say it again now the names are known, so the link's keys name their service.
    deps.keyHints.render();
    // `#343 pwa-app-name`. What this deployment calls its app. The server already wrote it into the
    // title of the page it served; taking it from this answer as well keeps the title the name the
    // server gives now, should the two ever differ. An older server sends none, and the title stays
    // as served.
    const appName = typeof config.app_name === "string" ? config.app_name.trim() : "";
    if (appName) deps.setTitle(appName);
    // WHO THIS BRIDGE IS, from the server, before a single message is drawn.
    //
    // The page used to learn this only as a side effect of the reader replying from the app, or of
    // the live feed delivering a message this server had posted. A reader who had done neither got a
    // channel in which NOTHING was recognised: their own words were not "me", and the bridge's own
    // account still counted as a second bot, so the "the only bot that is not us" guess for the
    // coding agent became a coin toss it declines to call. Every row fell through to the same
    // third-party colour, which is exactly what the owner reported seeing.
    if (config && config.self_author_id) {
      deps.noteSelfAuthor(config.self_author_id);
    }
    // The reader's own Discord account, if the operator has said what it is. Not derivable; see
    // `ownerAuthorId` in web/src/core/channels/identities.ts.
    deps.whoIsWho.setOwnerAuthorId(config && config.owner_author_id);
    deps.whoIsWho.learnOwnerIds(config && config.owner_author_ids);
    deps.voiceProvider.learn(config && config.conversational_voice);
    deps.voiceCall().session.providerName = deps.voiceProvider.current().name;
    deps.settingsReadouts.agentLink();
    // `#39 channel-alias`. Both pickers are drawn from the same list and through the same naming
    // rule, so the name in the bar and the name in Settings are one answer rather than two.
    deps.channelDirectory.setChannels(config.channels || []);
    deps.channelDirectory.setHidden(Array.isArray(config.hidden_channels) ? config.hidden_channels : []);
    deps.channelSettings.renderHiddenChannels();
    deps.channelDirectory.setRegistrationSupported(config.channel_registration_supported === true);
    deps.channelDirectory.setDiscoverySupported(
      deps.channelDirectory.registrationSupported() && config.channel_discovery_supported === true);
    // `#232 upstream-mark-read`. Read per channel, by `upstreamClearOffered`, in the channel box —
    // and, with the token's scope, by Settings' picker as it is filled, which offers All channels only
    // where one channel can take Clear unread (`#351 settings-polish`). So both before the pickers.
    deps.channelDirectory.setCapability("upstreamClear", config.upstream_clear_supported === true);
    deps.serverFacts.setTokenScope(config.token_scope);
    // `#457 settings-server-side`. Only a server that says it keeps settings is asked for
    // them: an older one is never probed for a route it has not got.
    deps.settingsSync().setSupported(config.client_settings_supported === true);
    deps.fillChannelSelect("discord-channel");
    // `#355 provider-grouped-channels`. The Service picker before Settings' channel picker, which lists
    // that service's channels; the hidden list and the add form are that service's too.
    deps.channelSettings.fillServiceSelect();
    deps.fillChannelSelect("settings-channel");
    deps.channelSettings.renderHiddenChannels();
    deps.channelSettings.renderAddChannelForm();
    // The picker has only just been given a value, and both halves of the box are scoped by it —
    // the render at load ran against an empty picker and listed nothing whatever was stored.
    deps.channelSettings.renderChannelBox();
    deps.restoreChannelComposer();
    // `#44 live-push`. The server says whether it is watching the channel at all, and how often
    // (`ProviderFacts.fromConfig`): without it a quiet channel and an unwatched one look the same.
    deps.providerFacts.fromConfig(config);
    deps.channelDirectory.setCapability("upstreamReadMark", deps.providerFacts.upstreamReadMark());
    // `#195 send-resilience`. Read per channel, by `channelPostsOnce`, when a send fails.
    deps.channelDirectory.setCapability("idempotentPosts", config.idempotent_posts_supported === true);
    // `#252 thread-button-state`. Read per channel, by `channelRepliesInThread`, for a reply's route.
    deps.channelDirectory.setCapability("repliesInThread", config.replies_in_thread === true);
    deps.summaryRows.setDisabledByServer(
      typeof config.summaries_unavailable === "string" ? config.summaries_unavailable.trim() : "");
    if (deps.summaryRows.disabledByServer() && deps.summaryRows.mode()) deps.setSummaryMode(false);
    // ...and back on when the server can summarise again and the owner's setting still says so.
    else if (!deps.summaryRows.disabledByServer() && !deps.summaryRows.mode() && deps.storedSummarizeCollapsed()) deps.setSummaryMode(true);
    deps.renderScrollTools();
    deps.channelDirectory.setCapability("liveDelivery", deps.providerFacts.liveDelivery());
    deps.channelDirectory.setCapability("livePollSeconds", deps.providerFacts.livePollSeconds());
    // The picker already holds a channel; the flags above are the deployment's, and that channel's
    // own provider may answer differently.
    deps.applyChannelProvider();
    // The clear-unread control is offered only to a token that may change it (`deps.serverFacts.tokenScope`), on
    // a channel whose provider can, which `applyChannelProvider` has just settled.
    deps.upstreamClearBox.render();
    // `#46 conversation-replay`. What the SERVER permits, which is not the same as what the reader
    // has asked for — and the screen has to be able to say which of the two is stopping it.
    deps.callResume.setAllowed(config.replay_enabled === true);
    // `#270 voice-transcript-markers`. Records, `records=1` and the send memory's cursor go only to a
    // server that says it takes them.
    deps.voiceTranscript().transcriptRecordsSupported = config.transcript_records_supported === true;
    // ...and why replay is off where the operator turned it on, which each new conversation's line
    // says ("recap unsupported") rather than saying nothing, and Resuming's disabled switch says
    // beside it (`#331 resume-on-websocket`).
    deps.voiceTranscript().replayUnavailable = typeof config.replay_unavailable === "string" ? config.replay_unavailable : null;
    // `#512 voice-recap-private-stack`. Whether a new call's recap rides in the profile the bridge
    // fetches, and how the owner's own speech holds the page's messages.
    deps.voiceTranscript().resumeViaProfile = config.replay_via_profile === true;
    deps.voiceCall().speechHold = deps.speechHoldFrom(config.speech_hold);
    deps.renderResumeState();
    deps.settingsReadouts.speechPrep(config.speech_prep_enabled);
    deps.noiseSettings.apply(config.noise_rules);
    // `#264 voice-send-approval`. After `tokenScope`, which decides whether the box may be changed.
    deps.agentSettings.applySendSetting(config.voice_settings);
    // `#272 voice-agent-instructions`. After `tokenScope` too, for the same reason.
    deps.agentSettings.applyPrompt(config.voice_prompt);
    // `#303 voice-post-prefix`. After `tokenScope` too, for the same reason.
    deps.agentPrefix.apply(config.voice_post_prefix);
    // `#208 persistent-cache`. After `tokenScope` as well: only a write token is shown the group.
    deps.serverCacheBox.apply(config.message_cache);
    // `#427 server-thread-hues`: this browser's remembered colours for the channel up are offered
    // to the server now — the token's scope is known, the picker holds its channel, and no read
    // or stream of it has gone out yet, so the offer is issued before anything that could colour
    // those threads itself. Not awaited: sign-in does not wait on a colour offer, and the reads
    // that follow carry the server's answer either way (first writer wins there).
    void deps.seedServerThreadHues(deps.selectedChannelId());
    // Started here rather than when the Discord view opens: PUSH TWO is the point of it, and an
    // arriving message has to be able to reach a call that is happening on the OTHER tab. Following
    // only the visible view would mean the relay was off precisely while the reader was talking.
    deps.startChannelStream(deps.selectedChannelId());
    deps.serverFacts.markConfigApplied();
    deps.messageCache.saveCacheShell(config);
    // ...and the views chosen in channels it no longer lists, as their saved rows went: a thread's
    // summary is the credential's reading too. `#205 channel-view-memory`.
    const listed = new Set(deps.channelDirectory.channels().map((channel) => String(channel.id)));
    if (deps.uiState.chosenViewsNow().some((entry) => !listed.has(entry.channel))) {
      deps.uiState.forgetChosen((channel) => !listed.has(channel));
      deps.saveUiState();
    }
  }

  return { apply: applyClientConfig };
}
