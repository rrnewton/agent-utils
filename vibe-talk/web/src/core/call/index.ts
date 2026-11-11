// The voice call: its logical session, reconnection, and the decisions about what it says and hands
// on, without the audio or the socket themselves. Its public interface. `#624 voice-page-modularity`.

export {
  LOST_TURN,
  RECONNECT_ASKED,
  RECONNECT_MIN_GAP_MS,
  RECONNECT_RECORDS_WAIT_MS,
  RECONNECTS_PER_CALL_MAX,
  reconnectAsked,
  reconnectVerdict,
} from "./reconnect.ts";
export type { ReconnectingCall } from "./reconnect.ts";
export {
  STORED_SEAM_DETAIL,
  boundaryFor,
  callRecapLine,
  reconnectFreshWords,
  recapChip,
  recapSentence,
  recapWords,
  seamDetailFor,
} from "./recap.ts";
export type { Boundary, Recap } from "./recap.ts";
export { notSentDetail, reconnectSeamDetail, spokenOutcomeKind, spokenYesSeam } from "./seams.ts";
export { createAudioRoute } from "./audioroute.ts";
export type { AudioRoute, AudioRouteDependencies, CallAudioRoute } from "./audioroute.ts";
export { audioConstraints, createCallDevices } from "./devices.ts";
export type { CallDeviceLike, CallDevicePreferences, CallDevices, CallDevicesDependencies, CallRouteLike, CallStreamLike, CallTrackLike, MicSoundSettings } from "./devices.ts";
export { createVoiceProvider } from "./provider.ts";
export type { ConversationalVoice, VoiceProvider } from "./provider.ts";
export { blankResumeLast, createCallResume } from "./resume.ts";
export type { CallResume, CallResumeDependencies, ReplayPayload, ResumeLast, ResumeNote } from "./resume.ts";
