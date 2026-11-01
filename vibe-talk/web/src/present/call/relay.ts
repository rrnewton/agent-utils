// What a call says when it relays channel messages (#624 voice-page-modularity).
// Framing is read in turn order; the view can supply live properties without copying state.
export interface RelayFraming { readonly preamble: string; readonly open: string; readonly close: string; }

/**
 * What the agent is being ASKED TO DO with what follows, in the reader's chosen mode.
 *
 * First in the turn, before the quoted text, because a task sentence placed after the data would
 * sit behind whatever the third party wrote — and the whole point of the framing is that no line
 * of channel text is ever the last instruction in the turn.
 *
 * "Then stop" is in both of them on purpose. This is an interruption of a conversation the reader
 * is already having; the agent's job here is to say the one thing and hand the floor back, not to
 * open a topic.
 */
function relayTask(mode: string, count: number) {
  const many = count !== 1;
  if (mode === "full") {
    return (
      `Read ${many ? `these ${count} new chat messages` : "this new chat message"} out loud to ` +
      `the user word for word, saying who wrote ${many ? "each one" : "it"}. Then stop.`
    );
  }
  return (
    `Tell the user out loud, in one short sentence, what ` +
    `${many ? `these ${count} new chat messages say` : "this new chat message says"}. Then stop.`
  );
}

/** The whole turn: the task, the framing, and the fenced quotations. */
function relayTurn(quotes: readonly string[], mode: string, framing: RelayFraming) {
  return [
    relayTask(mode, quotes.length),
    framing.preamble,
    framing.open,
    ...quotes,
    framing.close,
  ].join("\n");
}

/** "1 new message", "3 new messages". */
function relayCount(count: number) {
  return `${count} new message${count === 1 ? "" : "s"}`;
}

export { relayTask, relayTurn, relayCount };
