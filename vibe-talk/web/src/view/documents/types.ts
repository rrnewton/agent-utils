// The Documents list's wire records (`#629 documents-view-client`): the client mirror of
// `DocumentRow` in src/http/api.rs and `Mention` in src/documents.rs, as `GET /api/v1/documents`
// answers them. Server `Option<T>` fields are `T | null` here, as the JSON carries them.

/** One mention of a document in one message. */
export interface DocumentMention {
  /** Channel the message was in. */
  channel_id: string;
  /** Message that linked the document. */
  message_id: string;
  /** Thread the message is a reply in, when it is one. */
  thread_id: string | null;
  /** When the message was posted, in Unix milliseconds, when known. */
  posted_ms: number | null;
  /** Display name of the author. */
  author: string;
  /** The link's label, when the writer gave one. */
  label: string | null;
}

/** One document row of `GET /api/v1/documents`. */
export interface DocumentRow {
  /** Canonical document key. */
  key: string;
  /** Document kind: `github_markdown`, `github_file`, `paste`, `google_doc`, `github_issue`, `github_gist`. */
  kind: string;
  /** Issue number, for `github_issue` rows; else null. `#1176 doc-kind-buckets`. */
  issue_number: number | null;
  /** Front-matter status, for fetched `github_issue` rows; else null. */
  issue_status: string | null;
  /** The address as linked. */
  url: string;
  /** Last path segment. */
  file_name: string;
  /** Real title when fetched, else the link label, else the file name. */
  title: string;
  /** Whether a fetched copy is in the bounded server cache. */
  fetched: boolean;
  /** Cached content version, when fetched. */
  version: string | null;
  /** When the cached copy was fetched, in Unix milliseconds. */
  fetched_ms: number | null;
  /** Where the document lives now, when a moved fetch found it elsewhere. */
  moved_to: string | null;
  /** How many messages link this document. */
  mention_count: number;
  /** First mention, in Unix milliseconds, when known. */
  first_mentioned_ms: number | null;
  /** Latest mention, in Unix milliseconds, when known. */
  last_mentioned_ms: number | null;
  /** Every mention, oldest first. */
  mentions: DocumentMention[];
}

/** The answer of `GET /api/v1/documents` without a sync cursor. */
export interface DocumentsListAnswer {
  documents: DocumentRow[];
  total: number;
  revision: number;
}
