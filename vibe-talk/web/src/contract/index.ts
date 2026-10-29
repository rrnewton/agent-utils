// The vibe-talk wire contract for the page's modules (`#624 voice-page-modularity`): its types, and
// the validators every answer from the server is decoded by. Generated from the Rust-derived schema
// by contract/generate.mjs (validators.js and validators.d.ts); this is its public interface.

export { decode, decodeTagged, is, roots } from "./validators.js";
export type * from "./validators.js";
