
// Sending from the channel's message box (`#300 reply-marks-read`; `#624
// voice-page-modularity`, moved from the legacy script into view/composer; mu-readcore):
// the box's draft remembered and restored per channel/thread, and Send invoking the
// outbox's narrow enqueue capability with the destination captured here, never re-read
// from the DOM selection downstream.

/** What sending from the box asks of the page. */
export interface ChannelSendDependencies {
  composerKeys: { onKey(event: KeyboardEvent, send: () => void, box: EventTarget | null): boolean };
  el(id: string): HTMLInputElement & HTMLElement;
  selectedChannelId(): string;
  composerState(): { key: string };
  selectionThreadId(): string | null;

  knownWritable(channel: string): boolean;
  rootThisStarts(channel: string, threadId: string | null): string | null;
  queueOutgoingMessage(request: { channel: string; threadId: string | null; replyTo: null; text: string; marksRead: string | null }): Promise<unknown>;
  channelDrafts: {
    get(key: string): string | undefined;
    set(key: string, text: string): void;
    delete(key: string): void;
    save(): boolean;
  };
  storageOkay(): boolean;
  noteDraft(kind: string, key: string, channel: string, held: boolean): void;
  fitBox(box: HTMLElement): void;
  draftRefusedWords: string;
  scrollToNewest(): void;
  renderChannelNavigation(): void;
  renderDraftChip(): void;
}

/** The box's send commands: the legacy names, unchanged. */
export interface ChannelSend {
  sendChannelMessage(): Promise<void>;
  rememberChannelDraft(): void;
  restoreChannelComposer(): void;
  saveChannelDrafts(): boolean;
  channelComposerKey(): string;
  onComposerKey(event: KeyboardEvent, send: () => void, box?: EventTarget | null): boolean;
}

/** Sending from the box over `deps`. */
export function createChannelSend(deps: ChannelSendDependencies): ChannelSend {
  const el = deps.el;
  const selectedChannelId = deps.selectedChannelId;
  const queueOutgoingMessage = deps.queueOutgoingMessage;
  const channelDrafts = deps.channelDrafts;
  const noteDraft = deps.noteDraft;
  const scrollToNewest = deps.scrollToNewest;
  const renderChannelNavigation = deps.renderChannelNavigation;
  const renderDraftChip = deps.renderDraftChip;
  const composerState = deps.composerState;
  const knownChannel = (channel: string) => ({ writable: deps.knownWritable(channel) });
  const rootThisStarts = deps.rootThisStarts;
  const outbox = { storageOkay: () => deps.storageOkay() };
  const selection = () => ({ threadId: deps.selectionThreadId() });
  const composerKeys = deps.composerKeys;
  const fitGrowingBox = deps.fitBox;
  const CHANNEL_DRAFT_REFUSED = deps.draftRefusedWords;

async function sendChannelMessage() {
  const key = composerState().key;
  const channel = selectedChannelId();
  const text = el("channel-compose-text").value;
  if (knownChannel(channel)?.writable === false) {
    el("channel-compose-state").textContent = "This channel is read-only.";
    return;
  }
  if (!text.trim() || !channel) return;
  rememberChannelDraft();
  // `#300 reply-marks-read`. The first reply in a thread nobody has answered marks its root, as Reply
  // on the root would; a later one marks nothing, and the main channel's box never does.
  const completion = queueOutgoingMessage({
    channel, threadId: selection().threadId, replyTo: null, text,
    marksRead: rootThisStarts(channel, selection().threadId),
  });
  channelDrafts.delete(key);
  // A failed outbox write leaves the previous durable draft available as a last resort.
  if (outbox.storageOkay()) saveChannelDrafts();
  noteDraft("composer", key, String(channel), false);
  el("channel-compose-text").value = "";
  fitGrowingBox(el("channel-compose-text"));
  el("channel-compose-state").textContent = "";
  scrollToNewest();
  await completion;
}


function rememberChannelDraft() {
  if (!composerState().key) return;
  const text = el("channel-compose-text").value;
  const before = channelDrafts.get(composerState().key) || "";
  // `#846 draft-chip-honest`. Blank words are no draft, and are not kept.
  if (text.trim()) channelDrafts.set(composerState().key, text);
  else channelDrafts.delete(composerState().key);
  if (!saveChannelDrafts()) {
    el("channel-compose-state").textContent = CHANNEL_DRAFT_REFUSED;
  }
  // `#819 voice-page-quoted-reply-and-draft-chip`. Found again by the Draft chip; no newer for being unchanged.
  if (text !== before) noteDraft("composer", composerState().key, String(selectedChannelId() || ""), Boolean(text.trim()));
}

function restoreChannelComposer() {
  composerState().key = channelComposerKey();
  const channel = selectedChannelId();
  const readOnly = knownChannel(channel)?.writable === false;
  el("channel-compose-text").value = channelDrafts.get(composerState().key) || "";
  el("channel-compose-state").textContent = readOnly
    ? "This channel is read-only."
    : "";
  el("channel-compose-text").disabled = false;
  el("channel-send").disabled = !channel || readOnly;
  renderChannelNavigation();
  // `#340 composer-grow-box`. The draft put back is the box's height too.
  fitGrowingBox(el("channel-compose-text"));
  // `#819 voice-page-quoted-reply-and-draft-chip`. Another channel's drafts, or this one's again.
  renderDraftChip();
}

function saveChannelDrafts() {
  return channelDrafts.save();
}

function channelComposerKey() {
  return JSON.stringify([selectedChannelId(), selection().threadId]);
}


function onComposerKey(event: KeyboardEvent, send: () => void, box: EventTarget | null = event && event.target): boolean {
  return composerKeys.onKey(event, send, box);
}

// How long after a paste an Enter is part of it (`PASTE_ENTER_MS`), when each box last took one, each
// box's new-line key (`NEWLINE_KEYS`) and putting a new line in a box (`insertNewline`) are
// web/src/view/composer's (`paste-never-sends`, `#575 mobile-keyboard-compose`).

/** A message box's `beforeinput`, `input` or `paste`, noted (`composerKeys`). `paste-never-sends`. */

  return { sendChannelMessage, rememberChannelDraft, restoreChannelComposer, saveChannelDrafts, channelComposerKey, onComposerKey };
}
