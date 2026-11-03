// Speaking into a message box (`#821 voice-page-read-aloud-react`): the browser's own speech recognition,
// started by a press (a user gesture, which Chrome requires), writing what it hears into the box as it hears
// it, the words it is still unsure of after the ones it has settled on. A page cannot switch a phone's keyboard
// into its voice mode, so where the browser has no recognition, or refuses it (no microphone, no permission,
// no network: Chrome's recognition is a server's), the box stays focused and a line says to use the keyboard's
// microphone. Which box, what the line says and keeping what was heard as a draft are the page's.

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
  box(): HTMLTextAreaElement | HTMLInputElement;
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
}

/** Not listening. */
export function createDictation(deps: DictationDependencies): Dictation {
  let recognition: Recognition | null = null;

  function finish(said: string | null): void {
    recognition = null;
    deps.listening(false);
    deps.say(said === null ? "" : said);
  }

  return {
    start() {
      if (recognition !== null) return true;
      const Make = deps.recognition();
      if (Make === null) {
        deps.say(DICTATION_UNAVAILABLE);
        return false;
      }
      let heard: Recognition;
      try {
        heard = new Make();
      } catch (_error) {
        deps.say(DICTATION_UNAVAILABLE);
        return false;
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
        deps.say(DICTATION_UNAVAILABLE);
        return false;
      }
      recognition = heard;
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
  };
}

// What the page and the tests use of it.
export { DICTATION_LISTENING, DICTATION_UNAVAILABLE };
