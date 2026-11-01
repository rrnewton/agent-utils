// The owner's own speech as the microphone hears it (`#512 voice-recap-private-stack`): the level
// and cap the server names for holding a page message while he speaks, and one capture buffer's
// level. When to hold, and for how long, are the call's. Moved here from web/voice.js unchanged but
// for its types (`#624 voice-page-modularity`).

/** The level at or above which the microphone hears the owner, in dBFS, and the longest a page message waits, in ms (0: never). */
export interface SpeechHold {
  readonly levelDbfs: number;
  readonly capMs: number;
}

/** The level and cap when the server names none, as the server's own defaults are. */
const SPEECH_HOLD_DBFS_DEFAULT = -42;
const SPEECH_HOLD_CAP_MS_DEFAULT = 20000;

/** The hold client-config names, or this page's defaults where it names none or something unusable. */
function speechHoldFrom(named: unknown): SpeechHold {
  const given = named && typeof named === "object" ? named as { level_dbfs?: unknown; cap_ms?: unknown } : {};
  const level = Number(given.level_dbfs);
  const cap = Number(given.cap_ms);
  return {
    levelDbfs: Number.isFinite(level) ? Math.min(0, level) : SPEECH_HOLD_DBFS_DEFAULT,
    capMs: Number.isFinite(cap) && cap >= 0 ? Math.min(cap, 600000) : SPEECH_HOLD_CAP_MS_DEFAULT,
  };
}

/** One capture buffer's level, in dBFS: the RMS of its samples against full scale. -Infinity for silence. */
function bufferDbfs(samples: Float32Array): number {
  let sum = 0;
  for (let at = 0; at < samples.length; at += 1) sum += samples[at]! * samples[at]!;
  const rms = samples.length > 0 ? Math.sqrt(sum / samples.length) : 0;
  return rms > 0 ? 20 * Math.log10(rms) : -Infinity;
}

// What the page and the tests use of it.
export { SPEECH_HOLD_CAP_MS_DEFAULT, SPEECH_HOLD_DBFS_DEFAULT, bufferDbfs, speechHoldFrom };
