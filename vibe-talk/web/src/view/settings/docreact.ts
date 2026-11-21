// The Documents React policy picker (`#2006 vibe-talk-documents-react`):
// where a React from the Documents reader posts — the thread of the most
// recent mention (the default), or one discussion thread in that
// mention's channel. A synced choice, kept as the Colour theme is; the
// reader only reads it at Send. Construction does nothing.

import { createListening } from "../dom/index.ts";
import type { DocumentsReactPolicy } from "../../core/settings/index.ts";

/** What the picker asks of the page. */
export interface DocumentsReactDependencies {
  lookup(id: string): HTMLElement | null;
  stored(): DocumentsReactPolicy;
  keep(choice: DocumentsReactPolicy): void;
}

/** The picker, mounted. */
export interface DocumentsReactControl {
  /** Draw the kept choice in the picker (a sync landing, a restore). */
  restore(): void;
  unmount(): void;
}

/** Mount the Documents React policy picker. */
export function mountDocumentsReact(deps: DocumentsReactDependencies): DocumentsReactControl {
  const { listen, unmount } = createListening();
  function restore(): void {
    const select = deps.lookup("documents-react-policy") as HTMLSelectElement | null;
    if (select) select.value = deps.stored();
  }
  const select = deps.lookup("documents-react-policy");
  if (select) {
    listen(select, "change", () => {
      const choice = (select as HTMLSelectElement).value === "discussion" ? "discussion" : "mention";
      deps.keep(choice);
    });
  }
  return { restore, unmount };
}
