// Putting the list in a read mode (`#221 read-modes`; `#624 voice-page-modularity`, moved from
// the legacy script): the mode's owner made here, and the one act that puts the list in a mode.
// What the modes are, their names and the cycle between them stay present/rows' (`createReadMode`,
// `#1168` included: a stored Hide now reads as Fold, and asking for Hide here folds). Words,
// order and timing are as they were in the script.

import { READ_MODES } from "../../present/rows/index.ts";
import type { ReadMode } from "../../present/rows/index.ts";

/** What putting the list in a read mode asks of the page. */
export interface ReadModeActDependencies {
  /** The mode's owner, made by present/rows' factory, handed in as a maker. */
  makeReadModes(): ReadMode;
  /** Run `change` as a filter change, the reader's place kept; its read, if it made one. */
  keepingFilterPlace(change: () => unknown): unknown;
  clearReadRuns(): void;
  settleUiState(): void;
  saveUiState(): void;
  disarmBacklog(): void;
  clearLastDismissal(): void;
  clearBacklogCount(): void;
  renderTodoControls(): void;
  threading(): boolean;
  eachColumnRenderCached(): void;
  forgetCards(): void;
  sideColumnUp(): boolean;
  renderSideColumn(): void;
  loadDiscordEnter(): Promise<unknown>;
  applyReadRuns(): void;
  renderScrollTools(): void;
  guardQuietly(run: () => unknown): () => unknown;
  requestVisibleSummaries(): void;
}

/** The read mode's owner and the act that puts the list in a mode. */
export interface ReadModeAct {
  readonly readModes: ReadMode;
  setReadMode(mode: string): void;
}

/** Make the read mode act, the mode's owner first as the page made it. */
export function createReadModeAct(deps: ReadModeActDependencies): ReadModeAct {
  const readModes = deps.makeReadModes();

  /**
   * Put the list in read mode `mode`. `#221 read-modes`. `#243 filter-keeps-selection`: a read mode is
   * a filter like any other, and the whole change — the notes and controls over the list included — is
   * made inside `keepingFilterPlace`: the reader's message stays where it is on the screen, or the line
   * of the run it joins, or the nearest message left in its place.
   */
  function setReadMode(mode: string): void {
    if (!READ_MODES.includes(mode)) return;
    if (mode === "hide") mode = "fold";
    const wasHiding = readModes.hiding();
    const read = deps.keepingFilterPlace(() => {
      readModes.set(mode);
      // Runs opened under Fold read are a way of looking at that list, and the reader has just
      // chosen another way: coming back to Fold read folds them all again.
      deps.clearReadRuns();
      // Kept across a reload, as the reader left it. `#189 restore-ui-state`.
      deps.settleUiState();
      deps.saveUiState();
      // The UI-state record above is the restore; this copy is only the sync's.
      readModes.keep();
      deps.disarmBacklog();
      // An undo belongs to the act it undoes, and leaving the view is the reader moving on. Keeping
      // it would offer to restore messages into a list they are no longer looking at. Showing or
      // collapsing what is read leaves every message in the list, so the undo stays between those two.
      if (readModes.hiding() !== wasHiding) deps.clearLastDismissal();
      if (!readModes.hiding()) {
        deps.clearBacklogCount();
      }
      deps.renderTodoControls();
      if (deps.threading()) {
        // One read mode, for both columns. `#229 desktop-two-column`.
        deps.eachColumnRenderCached();
        // `#404 threads-unpack-all`. And inside every unpacked block.
        deps.forgetCards();
        if (deps.sideColumnUp()) deps.renderSideColumn();
      } else if (readModes.hiding() !== wasHiding) {
        // A read of another route, which keeps the reader's place as a refresh does and is placed again
        // when it lands. What it brings is the reader's own act, not something that arrived.
        return deps.loadDiscordEnter();
      } else {
        // Show read and Fold read are the same read of the channel, drawn two ways.
        deps.applyReadRuns();
        deps.renderScrollTools();
      }
      return undefined;
    });
    if (read) deps.guardQuietly(() => read)();
    deps.requestVisibleSummaries();
  }

  return { readModes, setReadMode };
}
