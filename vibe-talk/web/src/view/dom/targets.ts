// What a key does depends on where it lands (`#624 voice-page-modularity`, moved from the legacy
// script; plan step 16): a field the reader types in, a control that keeps its own keys, a link.
// These read only the node a key landed on; what the key then does is the page's.

/** The parts of a key's target these read; any may be missing, as on a text node or the window. */
interface KeyTarget {
  readonly tagName?: unknown;
  readonly type?: unknown;
  readonly isContentEditable?: unknown;
  readonly getAttribute?: unknown;
}

/** A key's target as the parts these read, or null for none. */
const keyTarget = (node: unknown): KeyTarget | null => (node && typeof node === "object" ? node as KeyTarget : null);

/** A target's tag, upper case, or "" for none. */
const upperTag = (node: unknown): string => String(keyTarget(node)?.tagName || "").toUpperCase();

/**
 * The input types that take no typing: on one of these a single key is a command, not a character.
 * Every other input (text, search, password, number, a date) is a field the reader types in.
 */
const UNTYPED_INPUTS: readonly string[] = Object.freeze(["checkbox", "radio", "range", "button", "submit", "reset", "image", "file", "color"]);

/** Whether `node` is somewhere the reader types: a text input, a textarea, or editable content. */
function isTextField(node: unknown): boolean {
  const target = keyTarget(node);
  if (!target) return false;
  if (target.isContentEditable === true) return true;
  const tag = upperTag(target);
  if (tag === "TEXTAREA") return true;
  return tag === "INPUT" && !UNTYPED_INPUTS.includes(String(target.type || "text").toLowerCase());
}

/**
 * Whether `node` keeps the arrow keys for itself: a picker, a slider, the reading-width handle. A
 * single key on one of those is the control's too: a picker's type-to-find, a slider's step.
 */
function keepsOwnKeys(node: unknown): boolean {
  const target = keyTarget(node);
  if (!target) return false;
  const tag = upperTag(target);
  if (tag === "SELECT") return true;
  if (tag === "INPUT" && String(target.type || "").toLowerCase() === "range") return true;
  const role = typeof target.getAttribute === "function" ? (target as Element).getAttribute("role") : null;
  return role === "separator" || role === "slider" || role === "spinbutton";
}

/** The controls a Space presses, toggles or opens. On one of them Space is the control's own. */
function pressedBySpace(node: unknown): boolean {
  return ["BUTTON", "INPUT", "SELECT", "TEXTAREA", "SUMMARY"].includes(upperTag(node));
}

/** A link: Ctrl+Enter (Cmd+Enter) on one is the browser's, which opens it in a new tab. */
const isLink = (node: unknown): boolean => upperTag(node) === "A";

/** The controls an Enter presses or follows: on one of them Enter is its own, not the row's. */
function answersEnter(node: unknown): boolean {
  return ["BUTTON", "A", "INPUT", "SELECT", "TEXTAREA", "SUMMARY"].includes(upperTag(node)) || isTextField(node);
}

/** A textarea holding more than it shows, which has a page of its own for the page keys to move through. */
function scrollsItself(field: Element): boolean {
  return String(field.tagName || "").toUpperCase() === "TEXTAREA" &&
    Number(field.scrollHeight) > Number(field.clientHeight) + 1;
}

// What the page and the tests use of it.
export { UNTYPED_INPUTS, answersEnter, isLink, isTextField, keepsOwnKeys, pressedBySpace, scrollsItself };
