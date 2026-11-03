// Putting plain text on the clipboard (`#198 copy-message-text`, `#408 copy-upstream-link`; `#624
// voice-page-modularity`, moved from the legacy script into view/dom): the asynchronous Clipboard API
// first, and where that is absent or refuses, the old selection-and-copy path through a hidden textarea.
// The page hands over the browser's clipboard (`navigator.clipboard`); saying whether it went is the caller's.

/** The asynchronous Clipboard API, as far as copying plain text uses it. */
export interface TextClipboard {
  writeText?: (text: string) => Promise<void>;
}

/**
 * Put `value` on the clipboard as plain text; whether it went. `clipboard` gives the browser's Clipboard
 * API, asked for inside the attempt, so a host without one falls back like one that refuses.
 *
 * The asynchronous Clipboard API first: it needs a secure context and a user gesture, both of which a
 * tap on the ⋯ menu over HTTPS provides, and a key gives as well. Where that API is absent, or refuses
 * (as it does with the document not focused), the old selection-and-copy path through a hidden textarea
 * (`copyThroughTextarea`). False when neither works, for the caller to say so: never a silent no-op the
 * reader mistakes for a copy.
 */
async function copyText(value: string, clipboard: () => TextClipboard | null | undefined): Promise<boolean> {
  try {
    const api = clipboard();
    if (api && typeof api.writeText === "function") {
      await api.writeText(value);
      return true;
    }
  } catch (_error) {
    // Refused: the old way may still be allowed.
  }
  return copyThroughTextarea(value);
}

/**
 * Copy `value` as the browser copies a selection: in a textarea out of sight, selected, by
 * `execCommand("copy")`. The textarea is fixed at the corner of the window, so taking the focus scrolls
 * nothing, and the focus goes back where it was, so the keys still move the list. Whether it went.
 */
function copyThroughTextarea(value: string): boolean {
  if (typeof document.execCommand !== "function" || !document.body) return false;
  const back = document.activeElement as HTMLElement | null;
  const scratch = document.createElement("textarea");
  scratch.value = value;
  scratch.setAttribute("readonly", "");
  scratch.setAttribute("aria-hidden", "true");
  scratch.setAttribute("tabindex", "-1");
  scratch.style.position = "fixed";
  scratch.style.top = "0";
  scratch.style.left = "0";
  scratch.style.opacity = "0";
  document.body.append(scratch);
  let copied = false;
  try {
    scratch.focus({ preventScroll: true });
    scratch.select();
    copied = document.execCommand("copy");
  } catch (_error) {
    copied = false;
  } finally {
    if (scratch.parentNode) scratch.parentNode.removeChild(scratch);
    if (back && back !== document.body && typeof back.focus === "function") back.focus({ preventScroll: true });
  }
  return copied;
}

export { copyText, copyThroughTextarea };
