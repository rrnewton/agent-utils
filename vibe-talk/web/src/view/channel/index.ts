// view/channel (`#624 voice-page-modularity`): the channel's rows, cards, replies and columns as the DOM
// draws them. It calls present/* for what to say and how to arrange it, and core/* for what is held.
// Moving in from the legacy script a piece at a time; see the plan's step 16.

export { FOLD_FLOW_BLOCKS, FOLD_INLINE, RESTATED_CLASS, elisionLine, foldRuns, foldedTextLine, textNodesOf } from "./fold.ts";
export { DAY_TITLED, EDITED_MARK, freshenDayTitle, rowTime, rowTimeTabStop, timeNode } from "./time.ts";
