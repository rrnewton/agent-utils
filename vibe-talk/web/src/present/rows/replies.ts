// What a message's replies are called (`#624 voice-page-modularity`, moved from the legacy script into
// present/rows; `#228 reply-visible`): their count, and the name of the circle that jumps to one.

/** "1 reply", "3 replies". */
const countOf = (n: number): string => `${n} ${n === 1 ? "reply" : "replies"}`;

/** The circle's accessible name, for the `at`th (from 1) of `of` replies. No channel data. */
const replyMarkerName = (at: number, of: number): string =>
  of === 1 ? "Jump to the reply to this message" : `Jump to reply ${at} of ${of} to this message`;

export { countOf, replyMarkerName };
