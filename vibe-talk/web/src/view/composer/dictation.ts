// Speaking into a message box (`#821 voice-page-read-aloud-react`): the browser's own speech recognition,
// started by a press (a user gesture, which Chrome requires), writing what it hears into the box as it hears
// it, the words it is still unsure of after the ones it has settled on. A page cannot switch a phone's keyboard
// into its voice mode, so where the browser has no recognition, or refuses it (no microphone, no permission,
// no network: Chrome's recognition is a server's), the box stays focused and a line says to use the keyboard's
// microphone. Which box, what the line says and keeping what was heard as a draft are the page's.
//
// `#829 voice-page-dictation-polish`: while it listens the phone's keyboard stays down, so it does not cover the
// reply being spoken. The box keeps the focus and its caret but asks for no keyboard (`inputmode="none"`),
// from before it takes the focus where the page can say so (`quietKeyboard`), until a tap in the box asks
// for the keyboard back (`showKeyboard`), or the listening stops, ends or fails.
//
// `#1503 react-dictation-keeps-listening`: listening keeps going until the owner stops it or sends. A
// recognition session still ends at the first pause (Chrome on Android ends it there), so a session that
// ends while the box is open and listening is started again, with what it settled on kept across the
// restart and its unsure words replaced, never appended. An explicit stop, a send, the box closing and a
// permission error end it for good. Sessions that hear nothing are counted, never timed: after
// `DICTATION_EMPTY_SESSION_LIMIT` of them in a row it stops and says to tap to listen again, so a
// recognition that ends at once forever cannot spin. A later setting may offer the keyboard's microphone
// instead of the page's (`start` is where that choice would go, leaving `launch` and the restart below
// as the page microphone's own); it is not built here.

/** The browser's recognition, as far as this reads it (`SpeechRecognition`, `webkitSpeechRecognition`). */
export interface Recognition {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  onresult: ((event: RecognitionResultEvent) => void) | null;
  onerror: ((event: { readonly error?: string }) => void) | null;
  onend: (() => void) | null;
  start(): void;
  abort(): void;
}

/** A recognition result event, as far as this reads it. */
export interface RecognitionResultEvent {
  readonly resultIndex: number;
  readonly results: ArrayLike<{ readonly isFinal: boolean; readonly 0: { readonly transcript: string } }>;
}

/** What dictation asks of the page. */
export interface DictationDependencies {
  /** The browser's recognition constructor, or null where it has none. */
  recognition(): (new () => Recognition) | null;
  /** The language to hear, as the browser names it (`navigator.language`). */
  lang(): string;
  /** The box the words go into. */
  box(): Pick<HTMLTextAreaElement, "value" | "getAttribute" | "setAttribute" | "removeAttribute">;
  /** Say `text` on the box's state line ("" clears it). */
  say(text: string): void;
  /** The microphone is listening now, or not: for the box's look. */
  listening(on: boolean): void;
  /** Words the recognition settled on were written into the box: keep them as the draft. */
  settled(): void;
}

/** What the state line says while listening, where dictation cannot run, and when it has stopped. */
const DICTATION_LISTENING = "Listening… speak your reply.";
const DICTATION_UNAVAILABLE = "Dictation is not available here: tap the microphone on your keyboard.";
const DICTATION_STOPPED = "Stopped listening. Tap to listen again.";

/** Empty sessions in a row after which listening stops instead of restarting (`#1503`, counted, not timed). */
const DICTATION_EMPTY_SESSION_LIMIT = 5;

/** Errors no restart can fix: no permission, or no microphone to hear with. */
const DICTATION_FINAL_ERRORS: ReadonlySet<string> = new Set(["not-allowed", "service-not-allowed", "audio-capture"]);

/** The dictation's commands. */
export interface Dictation {
  /** Start listening into the box; whether it could. Where it cannot, the line says so. */
  start(): boolean;
  /** Stop listening, keeping what is in the box. */
  stop(): void;
  /** Whether it is listening now. */
  active(): boolean;
  /**
   * The box is about to take the focus to be spoken into: where the browser can hear, keep the keyboard down
   * from the start. `start` keeps it down; anything that does not listen after gives it back.
   */
  quietKeyboard(): void;
  /** The keyboard back (a tap in the box), listening or not. */
  showKeyboard(): void;
}

/** Not listening. */
export function createDictation(deps: DictationDependencies): Dictation {
  let recognition: Recognition | null = null;
  // The box's own `inputmode` while the keyboard is kept down (null: it had none); undefined while it is not.
  let keyboardWas: string | null | undefined;
  // Listening was asked for and has not been stopped for good: sessions that end under it restart.
  let wantListening = false;
  // What the box held before listening, and what every session so far settled on: kept across restarts.
  let before = "";
  let settledWords = "";
  // Sessions in a row that heard nothing, and whether this session has heard anything.
  let emptySessions = 0;
  let sessionHeard = false;

  function quiet(): void {
    const box = deps.box();
    if (keyboardWas === undefined) keyboardWas = box.getAttribute("inputmode");
    box.setAttribute("inputmode", "none");
  }

  function restore(): void {
    if (keyboardWas === undefined) return;
    const box = deps.box();
    if (keyboardWas === null) box.removeAttribute("inputmode");
    else box.setAttribute("inputmode", keyboardWas);
    keyboardWas = undefined;
  }

  // Where it cannot listen, the line says so and the keyboard is the way to speak.
  function unavailable(): false {
    wantListening = false;
    recognition = null;
    restore();
    deps.listening(false);
    deps.say(DICTATION_UNAVAILABLE);
    return false;
  }

  function finish(said: string | null): void {
    wantListening = false;
    recognition = null;
    restore();
    deps.listening(false);
    deps.say(said === null ? "" : said);
  }

  // One recognition session, hearing into the box over `before` and `settledWords`. Whether it started.
  function launch(): boolean {
    const Make = deps.recognition();
    if (Make === null) return false;
    let heard: Recognition;
    try {
      heard = new Make();
    } catch (_error) {
      return false;
    }
    heard.lang = deps.lang();
    heard.continuous = true;
    heard.interimResults = true;
    sessionHeard = false;
    const joined = (said: string) => (before && said && !/\s$/.test(before) ? `${before} ${said}` : `${before}${said}`);
    heard.onresult = (event) => {
      if (recognition !== heard) return;
      sessionHeard = true;
      emptySessions = 0;
      let unsure = "";
      let gained = false;
      for (let at = event.resultIndex; at < event.results.length; at += 1) {
        const result = event.results[at]!;
        const words = String(result[0].transcript || "");
        if (result.isFinal) {
          settledWords += words;
          gained = true;
        } else {
          unsure += words;
        }
      }
      deps.box().value = joined(`${settledWords}${unsure}`.replace(/^\s+/, ""));
      if (gained) deps.settled();
    };
    heard.onerror = (event) => {
      if (recognition !== heard) return;
      const error = event && typeof event.error === "string" ? event.error : "";
      if (DICTATION_FINAL_ERRORS.has(error)) {
        finish(DICTATION_UNAVAILABLE);
        return;
      }
      // Silence and blips end nothing here: the session's own end restarts it while listening is wanted.
    };
    heard.onend = () => {
      if (recognition !== heard) return;
      if (!wantListening) return;
      if (!sessionHeard) {
        emptySessions += 1;
        if (emptySessions >= DICTATION_EMPTY_SESSION_LIMIT) {
          finish(DICTATION_STOPPED);
          return;
        }
      }
      recognition = null;
      if (!launch()) unavailable();
    };
    recognition = heard;
    try {
      heard.start();
    } catch (_error) {
      if (recognition === heard) recognition = null;
      return false;
    }
    return true;
  }

  return {
    start() {
      if (wantListening) return true;
      if (deps.recognition() === null) return unavailable();
      before = deps.box().value;
      settledWords = "";
      emptySessions = 0;
      wantListening = true;
      if (!launch()) return unavailable();
      quiet();
      deps.listening(true);
      deps.say(DICTATION_LISTENING);
      return true;
    },
    stop() {
      const heard = recognition;
      if (heard === null) {
        wantListening = false;
        return;
      }
      finish(DICTATION_STOPPED);
      try {
        heard.abort();
      } catch (_error) {
        // Already over.
      }
    },
    active: () => wantListening,
    quietKeyboard() {
      if (deps.recognition() !== null) quiet();
    },
    showKeyboard: restore,
  };
}

// What the page and the tests use of it.
export { DICTATION_EMPTY_SESSION_LIMIT, DICTATION_LISTENING, DICTATION_STOPPED, DICTATION_UNAVAILABLE };
