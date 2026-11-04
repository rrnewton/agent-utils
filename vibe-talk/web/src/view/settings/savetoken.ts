// The Save button beside the token box, which visibly reacts (`#624 voice-page-modularity`, moved from the
// legacy script into view/settings): its words, its look and whether it can be pressed, and the timer that
// puts it back after a save. A banner that says "saved" while the button looks untouched leaves two questions
// open at once, whether the click registered and whether the save worked, so the button itself changes: at
// once on the click, and again on the result. Saving the token, and what the button says when, are the page's.

/** The page capabilities the button uses; none is called while it is mounted. */
export interface SaveTokenButtonDependencies {
  lookup(id: string): HTMLElement;
  setTimeout(run: () => void, ms: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/** The button's two commands; the timer is private. */
export interface SaveTokenButton {
  /** Draw Save as `label`, with `className`, pressable or not; a revert still waiting is dropped. */
  markSave(label: string, className: string, disabled: boolean): void;
  /** Run `revert` after `ms` milliseconds, unless the button is marked again first. */
  revertLater(revert: () => void, ms: number): void;
}

/** No revert waiting; mounting draws nothing. */
export function mountSaveTokenButton(deps: SaveTokenButtonDependencies): SaveTokenButton {
  /** The timer putting the button back after a save, or null. */
  let saveRevert: ReturnType<typeof setTimeout> | null = null;

  function markSave(label: string, className: string, disabled: boolean): void {
    const button = deps.lookup("save-token") as HTMLButtonElement;
    button.textContent = label;
    button.className = className;
    button.disabled = disabled;
    if (saveRevert !== null) {
      deps.clearTimeout(saveRevert);
      saveRevert = null;
    }
  }

  return {
    markSave,
    revertLater(revert, ms) {
      saveRevert = deps.setTimeout(revert, ms);
    },
  };
}
