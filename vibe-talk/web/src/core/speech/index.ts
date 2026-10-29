// What a read plays and the pieces a block is read in (`#376 read-aloud-blocks`, `#486
// read-aloud-pieces`, `read-chunk-balance`): the page's half of src/render.rs `read_pieces`. Its
// public interface. `#624 voice-page-modularity`.

export { balanceSentences, pieceSkewPasses, readPieces, sentenceEnds, speechSegments } from "./pieces.ts";
export { createPreparedSpeech, ticketLeftMs } from "./prepared.ts";
export type { PreparedBlock, PreparedEntry, PreparedSpeech, PreparedSpeechDependencies, SpeechMoment } from "./prepared.ts";
