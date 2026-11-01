// Audio as bytes and a read cut for a device voice (`#624 voice-page-modularity`, moved from the legacy
// script into core/speech): a microphone's samples resampled and made 16-bit PCM for the call, a
// streamed WAV's header read and its bytes joined, the time a device utterance took, and a read cut
// into the short utterances a phone's engine says whole (`#554 read-pause-reply-resume`).

// Linear resample to the rate in the session descriptor. Doing this explicitly beats asking for
// an AudioContext at that rate and hoping: a browser is allowed to give you a different rate, and
// the failure is silent and sounds like a chipmunk.
function downsampleTo(input: Float32Array, inputRate: number, outputRate: number): Float32Array {
  if (inputRate === outputRate) {
    return input;
  }
  const ratio = inputRate / outputRate;
  const out = new Float32Array(Math.floor(input.length / ratio));
  for (let i = 0; i < out.length; i += 1) {
    const at = i * ratio;
    const low = Math.floor(at);
    const high = Math.min(low + 1, input.length - 1);
    const frac = at - low;
    out[i] = (input[low] as number) * (1 - frac) + (input[high] as number) * frac;
  }
  return out;
}

function floatToPcm16(samples: Float32Array): Int16Array {
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i] as number));
    out[i] = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
  }
  return out;
}

// Short utterances avoid mobile engines' long-text limits while keeping every part of the row.
const BROWSER_SPEECH_CHUNK_CHARS = 180;

// `#554 read-pause-reply-resume`. One sentence per utterance, so a resume says the sentence it was
// paused in again, and no more of what was heard. A sentence shorter than this ("Yes.", or an "e.g.")
// is said with the next one.
const BROWSER_SPEECH_SENTENCE_MIN_CHARS = 24;

function browserSpeechChunks(text: unknown): string[] {
  let remaining = Array.from(String(text).replace(/\s+/gu, " ").trim());
  const chunks: string[] = [];
  while (remaining.length > 0) {
    let end = Math.min(BROWSER_SPEECH_CHUNK_CHARS, remaining.length);
    const head = remaining.slice(0, end).join("");
    // Closing quotes (straight, curly or angled) and brackets may follow the end of a sentence.
    const sentence = [...head.matchAll(/[.!?]+[\u0022\u0027\u2019\u201d\u00bb)\]]*\s+/gu)]
      .map((found) => Array.from(head.slice(0, (found.index as number) + found[0].length)).length)
      .find((length) => length >= BROWSER_SPEECH_SENTENCE_MIN_CHARS);
    if (sentence !== undefined) {
      end = sentence;
    } else if (end < remaining.length) {
      // A sentence longer than an utterance: cut at its last clause, else its last word.
      const clauses = [...head.matchAll(/[.!?;:]\s+/gu)];
      const last = clauses[clauses.length - 1];
      const sentenceEnd = last ? Array.from(head.slice(0, (last.index as number) + last[0].length)).length : 0;
      const space = remaining.slice(0, end).lastIndexOf(" ");
      if (sentenceEnd >= end / 2) end = sentenceEnd;
      else if (space > 0) end = space + 1;
    }
    const chunk = remaining.slice(0, end).join("").trim();
    if (chunk) chunks.push(chunk);
    remaining = remaining.slice(end);
  }
  return chunks;
}

function speechElapsedMs(start: number, end: number): number {
  return Math.max(0, Math.round(end - start));
}

function joinBytes(first: Uint8Array, second: Uint8Array): Uint8Array {
  if (first.length === 0) return second;
  const joined = new Uint8Array(first.length + second.length);
  joined.set(first, 0);
  joined.set(second, first.length);
  return joined;
}

/** The sample format and where the samples begin, once the WAV header is whole; null until then. */
function wavLayout(bytes: Uint8Array): { channels: number; rate: number; dataAt: number } | null {
  if (bytes.length < 12) return null;
  // Every read here is inside the bytes: the header is twelve at least, and each chunk is read only
  // once its eight-byte head is whole.
  const tag = (at: number): string =>
    String.fromCharCode(bytes[at] as number, bytes[at + 1] as number, bytes[at + 2] as number, bytes[at + 3] as number);
  if (tag(0) !== "RIFF" || tag(8) !== "WAVE") throw new Error("the audio is not a WAV stream");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let format: { channels: number; rate: number } | null = null;
  let at = 12;
  while (at + 8 <= bytes.length) {
    const size = view.getUint32(at + 4, true);
    if (tag(at) === "data") {
      if (format === null) throw new Error("the audio names no format before its samples");
      return { ...format, dataAt: at + 8 };
    }
    if (at + 8 + size > bytes.length) return null;
    if (tag(at) === "fmt ") {
      format = { channels: view.getUint16(at + 10, true), rate: view.getUint32(at + 12, true) };
      if (view.getUint16(at + 8, true) !== 1 || view.getUint16(at + 22, true) !== 16 || format.channels < 1) {
        throw new Error("the audio is not 16-bit PCM");
      }
    }
    at += 8 + size + (size % 2);
  }
  return null;
}

export {
  BROWSER_SPEECH_CHUNK_CHARS,
  BROWSER_SPEECH_SENTENCE_MIN_CHARS,
  browserSpeechChunks,
  downsampleTo,
  floatToPcm16,
  joinBytes,
  speechElapsedMs,
  wavLayout,
};
