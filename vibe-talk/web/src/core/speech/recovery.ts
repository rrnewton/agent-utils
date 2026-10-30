// When a read aloud is started again after it was cut off, and when a ticket can no longer carry it
// (`#290 speech-ticket-refresh`, `#368 speech-ticket-gone`, `#376 read-aloud-blocks`, `#487
// read-aloud-recovery`; `#624 voice-page-modularity`, moved from the legacy script; plan step 13).
// Why a cut-off read starts again at all is told where the legacy script does it (`recoverRead`).

import { ticketLeftMs } from "./prepared.ts";
import type { PreparedEntry, SpeechMoment } from "./prepared.ts";

/** How long to wait before each try at starting a cut-off read again: about as long as a restart. */
const READ_RECOVERY_WAITS_MS = [1000, 3000, 8000];

/**
 * The life a ticket must still have for a read in blocks to start on it: half of the ten minutes a
 * ticket lasts, longer than the messages read aloud take.
 */
const BLOCK_READ_TICKET_MARGIN_MS = 5 * 60 * 1000;

/** How long to wait before try `recovery` (0 for the first) at a cut-off read, or null when the tries are spent. */
export function recoveryWait(recovery: number): number | null {
  return recovery < READ_RECOVERY_WAITS_MS.length ? READ_RECOVERY_WAITS_MS[recovery] as number : null;
}

/** Whether a ticket (`until`, on both clocks) has ended at `now`: past either deadline is past. */
export function ticketEnded(ticket: { readonly until: SpeechMoment }, now: SpeechMoment): boolean {
  return now.mono >= ticket.until.mono || now.wall >= ticket.until.wall;
}

/** Whether `entry`'s ticket has too little life left at `now` for a read in blocks to start on it. */
export function ticketTooShortForBlocks(entry: PreparedEntry, now: SpeechMoment): boolean {
  return ticketLeftMs(entry, now) < BLOCK_READ_TICKET_MARGIN_MS;
}

export { BLOCK_READ_TICKET_MARGIN_MS, READ_RECOVERY_WAITS_MS };
