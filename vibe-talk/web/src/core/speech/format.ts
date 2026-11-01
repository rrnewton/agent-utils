// The PCM format a call can decode (#624 voice-page-modularity).
// The provider name is requested only when an unsupported format needs its diagnostic.

// "pcm_16000" -> 16000. Anything else (ulaw, mp3) is refused rather than played as noise.
function outputRateFrom(format: string | undefined, providerName: () => string) {
  if (!format) {
    return 16000; // the documented default for this websocket.
  }
  const match = /^pcm_(\d+)$/.exec(format);
  if (!match) {
    throw new Error(
      `the agent negotiated audio format "${format}", which this page cannot decode. ` +
        `Set ${providerName()}'s output format to PCM (for example pcm_16000).`
    );
  }
  return Number(match[1]);
}

export { outputRateFrom };
