// Hand-written declarations for the browser globals voice.js reads that TypeScript's DOM library
// does not describe. The wire contract's types are generated separately, in contract.d.ts.

interface Window {
  /** Safari's prefixed Web Audio constructor, used when `AudioContext` is absent. */
  webkitAudioContext?: typeof AudioContext;
}

interface HTMLLIElement {
  /**
   * The message objects a rendered row stands for: one on an ordinary row, several on a combined
   * one. Set by voice.js when it draws the row.
   */
  messages?: VibeTalk.Message[];
}
