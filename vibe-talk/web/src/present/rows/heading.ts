// A message's rich heading, decided without the DOM (`#242 rich-headers`; `#624
// voice-page-modularity`, moved from the legacy script into present/rows; mu-readcore):
// whether the heading the server sent restates the body's first words, and how long it is.

import type { Message } from "../../contract/index.ts";
import { rawTextOf } from "./message.ts";

/** A message's heading as the header line shows it. */
export interface MessageHeading {
  tag: string;
  time: string;
  title: string;
  cut: number;
}

export function headingOf(message: Message): MessageHeading | null {
  const heading = message && (message as { heading?: unknown }).heading;
  if (!heading || typeof heading !== "object") return null;
  const piece = (value: unknown) => (typeof value === "string" ? value : "");
  const fields = heading as { tag?: unknown; time?: unknown; title?: unknown; cut?: unknown };
  const [tag, time, title] = [piece(fields.tag), piece(fields.time), piece(fields.title)];
  const cut = fields.cut as number;
  if (!(tag || time || title) || !Number.isInteger(cut) || cut <= 0) return null;
  const content = rawTextOf(message);
  if (cut > content.length || content.slice(0, cut).includes("\n")) return null;
  return { tag, time, title, cut };
}

/**
 * Whether a drawn body begins with the mark on the words its heading restates: its first paragraph
 * marked, or that paragraph's first child. A heading whose words are not marked is not shown, so a
 * body can never lose words to a heading that does not restate them.
 */

export function headingLength(heading: MessageHeading, extra = 0): string {
  const characters = [...`${heading.tag}${heading.title}${heading.time ? `@${heading.time}` : ""}`].length + extra;
  const bucket = Math.max(16, Math.ceil(characters / 8) * 8);
  return bucket <= 96 ? String(bucket) : "long";
}

/**
 * The heading's pieces for the row's header line: the tag, the title, and the time after an `@`,
 * each a span of its own and every one of them text. No separators: the gap between them is the
 * stylesheet's.
 */
