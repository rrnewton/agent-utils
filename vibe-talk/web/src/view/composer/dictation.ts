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

/** What the state line says while listening, and where dictation cannot run. */
const DICTATION_LISTENING = "Listening… speak your reply.";
const DICTATION_UNAVAILABLE = "Dictation is not available here: tap the microphone on your keyboard.";

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
    restore();
    deps.say(DICTATION_UNAVAILABLE);
    return false;
  }

  function finish(said: string | null): void {
    recognition = null;
    restore();
    deps.listening(false);
    deps.say(said === null ? "" : said);
  }

  return {
    start() {
      if (recognition !== null) return true;
      const Make = deps.recognition();
      if (Make === null) return unavailable();
      let heard: Recognition;
      try {
        heard = new Make();
      } catch (_error) {
        return unavailable();
      }
      heard.lang = deps.lang();
      heard.continuous = false;
      heard.interimResults = true;
      // What the box held before, and what the recognition has settled on since; the unsure words follow.
      const before = deps.box().value;
      const joined = (said: string) => (before && said && !/\s$/.test(before) ? `${before} ${said}` : `${before}${said}`);
      let settledWords = "";
      heard.onresult = (event) => {
        if (recognition !== heard) return;
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
        // Silence is no failure: the box is there to type in. Anything else is, and is said.
        finish(event && event.error === "no-speech" || event && event.error === "aborted" ? null : DICTATION_UNAVAILABLE);
      };
      heard.onend = () => {
        if (recognition === heard) finish(null);
      };
      try {
        heard.start();
      } catch (_error) {
        return unavailable();
      }
      recognition = heard;
      quiet();
      deps.listening(true);
      deps.say(DICTATION_LISTENING);
      return true;
    },
    stop() {
      const heard = recognition;
      if (heard === null) return;
      finish(null);
      try {
        heard.abort();
      } catch (_error) {
        // Already over.
      }
    },
    active: () => recognition !== null,
    quietKeyboard() {
      if (deps.recognition() !== null) quiet();
    },
    showKeyboard: restore,
  };
}

// What the page and the tests use of it.
export { DICTATION_LISTENING, DICTATION_UNAVAILABLE };
