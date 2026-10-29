// How wide characters are drawn, in em, by the model the page's line counts use (the fold, the
// clamp, the elision labels). Moved here from web/voice.js unchanged but for its types (`#624
// voice-page-modularity`): pure heuristics over characters, no layout measured.

/** CJK ideographs, kana, Hangul and full-width forms: as wide as the type is tall, and breakable. */
const WIDE_SCRIPT = /[\u1100-\u115f\u2e80-\u303e\u3041-\u33ff\u3400-\u4dbf\u4e00-\u9fff\ua000-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe4f\uff00-\uff60\uffe0-\uffe6\u{20000}-\u{3fffd}]/u;

/** An emoji, drawn at least as wide as an ideograph. */
const PICTOGRAPH = /\p{Extended_Pictographic}/u;

/**
 * How wide one character is, in em, at the NARROW end of the common interface fonts (Roboto, the
 * San Francisco and Segoe faces, DejaVu and Noto Sans): narrower than any of them draws it, so a
 * line of the model holds at least as much as a real line does. A space is the space's width; the
 * thin letters and stops the thinnest; then the slim letters and brackets; the ordinary lowercase
 * of every alphabet; capitals, figures and symbols; the widest letters; and the wide scripts.
 */
function glyphWidth(ch: string): number {
  if (/\s/.test(ch)) return 0.24;
  if (/[.,:;'`|!ijlI-]/.test(ch)) return 0.19;
  if (/[ftr()[\]{}"/\\*]/.test(ch)) return 0.28;
  if (/[mwMW@%]/.test(ch)) return 0.72;
  if (/[A-Z0-9#$&?+=<>~^_]/.test(ch)) return 0.52;
  if (WIDE_SCRIPT.test(ch) || PICTOGRAPH.test(ch)) return 0.9;
  return 0.46;
}

/** `glyphWidth` for ASCII, looked up rather than matched: it is most of every message. */
const ASCII_EM = Array.from({ length: 128 }, (_unused, code) => glyphWidth(String.fromCharCode(code)));

/** How wide one character is, in em; see `glyphWidth`. */
function glyphEm(ch: string): number {
  const code = ch.charCodeAt(0);
  return ch.length === 1 && code < 128 ? ASCII_EM[code]! : glyphWidth(ch);
}

/** The width of the space between two words, in em, at the same narrow end. */
const SPACE_EM = glyphEm(" ");

/**
 * One character of code, inline or in a block, in em of the message text: every character of a
 * monospaced face is one width, and code is drawn at 0.85 of the text's size (web/voice.css,
 * `.md code` and `.md pre`). The narrowest common face is Consolas, at 0.55em, so 0.47 of the text's
 * em; this is a little under that. Not the proportional widths: an "m" there is 0.72, and a line of
 * them in code would be counted as wrapping where it does not.
 */
const MONO_EM = 0.45;

/** ...and a character of a wide script or an emoji in code, drawn from a fallback face at 0.85. */
const MONO_WIDE_EM = 0.76;

/** How wide one character of code is, in em of the message text; see `MONO_EM`. */
const monoEm = (ch: string): number => (WIDE_SCRIPT.test(ch) || PICTOGRAPH.test(ch) ? MONO_WIDE_EM : MONO_EM);

// What the page and the tests use of it.
export { MONO_EM, SPACE_EM, WIDE_SCRIPT, glyphEm, monoEm };
