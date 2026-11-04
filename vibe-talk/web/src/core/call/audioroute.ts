// The audio route of the call in progress (`#619 voice-bluetooth-output`; `#624 voice-page-modularity`, moved
// from the legacy script into core/call): the kind and label of the device the call's microphone opened, the
// kind and label of the output it plays to, and the sink it was aimed at, for the readout in Settings and the
// call-end record; null between calls. The record carries device KINDS to the server (`AudioRoute` in
// src/voice_health.rs), never labels. Opening the microphone, listing devices and aiming the output are the
// page's; so is the rule that turns a label into a kind (`classify`).
//
// The updates below assume a route stands, as the page's own did: a call that ended while its microphone was
// still opening (which takes the route away) throws at the next update, exactly as it always has.

/** One call's route. */
export interface CallAudioRoute {
  inputKind: string;
  outputKind: string;
  inputLabel: string;
  outputLabel: string;
  sink: string;
}

/** What the route asks of the page. */
export interface AudioRouteDependencies {
  /** The kind of a device, from the label the browser reports for it. */
  classify(label: string): string;
}

/** The route's commands and questions. */
export interface AudioRoute {
  /** The route of the call in progress, or null between calls. */
  current(): CallAudioRoute | null;
  /** `route` stands now (null: no call does). */
  set(route: CallAudioRoute | null): void;
  /** A call is opening its microphone: a fresh route, of no kind yet. */
  begin(): void;
  /** The microphone opened on a track labelled `label`: the input's kind and label, and the output's kind with it. */
  noteInput(label: string): void;
  /**
   * The microphone was opened again on a chosen device, labelled `chosenLabel`: the input keeps a label its
   * track gave, else takes the chosen device's, and the kinds follow that label.
   */
  settleInput(chosenLabel: string): void;
  /** The call's playback was aimed at `sink`, where a route stands. */
  noteSink(sink: string): void;
  /** The output chosen is labelled `label`, where a route stands. */
  noteOutput(label: string): void;
}

/** Between calls. */
export function createAudioRoute(deps: AudioRouteDependencies): AudioRoute {
  let callAudioRoute: CallAudioRoute | null = null;
  return {
    current: () => callAudioRoute,
    set(route) {
      callAudioRoute = route;
    },
    begin() {
      callAudioRoute = { inputKind: "other", outputKind: "other", inputLabel: "", outputLabel: "", sink: "" };
    },
    noteInput(label) {
      callAudioRoute!.inputKind = deps.classify(label);
      callAudioRoute!.inputLabel = label;
      callAudioRoute!.outputKind = callAudioRoute!.inputKind;
    },
    settleInput(chosenLabel) {
      if (!callAudioRoute!.inputLabel) callAudioRoute!.inputLabel = chosenLabel;
      callAudioRoute!.inputKind = deps.classify(callAudioRoute!.inputLabel);
      callAudioRoute!.outputKind = callAudioRoute!.inputKind;
    },
    noteSink(sink) {
      if (callAudioRoute) {
        callAudioRoute.sink = sink;
      }
    },
    noteOutput(label) {
      if (callAudioRoute) {
        callAudioRoute.outputKind = deps.classify(label);
        callAudioRoute.outputLabel = label;
      }
    },
  };
}
