// What kind of link an address is, for the Links filter (`#212 link-filter`, `#266
// link-filter-kinds`): the address alone decides. Moved here from web/voice.js unchanged but for
// its types (`#624 voice-page-modularity`).

/** A link's kind: one of the page's `LINK_KINDS`, "other" for every address no kind claims. */
export type LinkKind = "pr" | "commit" | "action" | "issue" | "file" | "other";

/**
 * An address on GitHub itself — `github.com` or `www.github.com`, in any case, and nothing longer
 * that merely begins with it — and its path, up to the query or the fragment. A port, a user name
 * or any other host is not GitHub's site as people link to it, and is Other.
 */
const GITHUB_PATH = /^https?:\/\/(?:www\.)?github\.com(?=[/?#]|$)([^?#]*)/i;

/**
 * `#266 link-filter-kinds`. The host GitHub serves a repository's files from as they are written —
 * where a file's Raw button goes — and the path after it, `/OWNER/REPO/REF/PATH`.
 */
const GITHUB_RAW_PATH = /^https?:\/\/raw\.githubusercontent\.com(?=[/?#]|$)([^?#]*)/i;

/**
 * A commit as GitHub writes it in a path: seven hex digits or more, which is the short form and the
 * full one. `.patch` and `.diff` are the same commit as text.
 */
const COMMIT_ID = /^[0-9a-f]{7,}(?:\.(?:patch|diff))?$/i;

/** A pull request's number, as text too; and a check run's, which is only ever a number. */
const PULL_NUMBER = /^\d+(?:\.(?:patch|diff))?$/;
const RUN_NUMBER = /^\d+$/;

/**
 * The kind of link an address is — one of `LINK_KINDS`, and always exactly one: "pr", "commit",
 * "action", "issue" or "file" where the address says so, and "other" for every address that does
 * not. The address alone decides, and the same address always answers the same.
 *
 * Inside one repository on GitHub, `/OWNER/REPO/…`, by the part of the path after it, as GitHub
 * routes them:
 *   - "pr": `/pull/N` and anything under it — its files, its checks, a comment's fragment — but
 *     not `/pull/N/commits/SHA`, which is one commit seen from the pull request; and `/pulls`.
 *   - "commit": `/commit/SHA`, `/commits` and anything under it, `/pull/N/commits/SHA`, and
 *     `/compare`, which is a run of commits.
 *   - "action": `/actions` and anything under it, a check run's `/runs/ID`, and `/checks`.
 *   - "issue": `/issues` and anything under it — one issue, a comment on it, a new one, a list.
 *   - "file": a file or a folder of the code at a branch, a tag or a commit — `/blob/REF/…`,
 *     `/tree/REF/…`, `/raw/REF/…` and `/blame/REF/…` — and any file on raw.githubusercontent.com,
 *     `/OWNER/REPO/REF/PATH`.
 * A trailing slash, the query and the fragment never change the answer. Keywords are matched as
 * GitHub writes them, lowercase, and only in the third place: `github.com/actions/checkout` is the
 * repository named checkout, not a run, and so is Other.
 */
function linkKind(href: unknown): LinkKind {
  const address = String(href);
  const raw = GITHUB_RAW_PATH.exec(address);
  // Owner, repository, ref and at least one name under it: the path a raw file is served at.
  if (raw) return raw[1]!.split("/").filter(Boolean).length >= 4 ? "file" : "other";
  const found = GITHUB_PATH.exec(address);
  if (!found) return "other";
  // Empty segments dropped, so a trailing slash, or a doubled one, changes nothing.
  const [, , section, first = "", second = "", third = ""] = found[1]!.split("/").filter(Boolean);
  switch (section) {
    case "pull":
      if (!PULL_NUMBER.test(first)) return "other";
      return second === "commits" && COMMIT_ID.test(third) ? "commit" : "pr";
    case "pulls":
      return "pr";
    case "commit":
      return COMMIT_ID.test(first) ? "commit" : "other";
    case "commits":
    case "compare":
      return "commit";
    case "actions":
    case "checks":
      return "action";
    case "runs":
      return RUN_NUMBER.test(first) ? "action" : "other";
    case "issues":
      return "issue";
    case "blob":
    case "tree":
    case "raw":
    case "blame":
      // A file view names the ref it is read at; the word alone is not a file.
      return first === "" ? "other" : "file";
    default:
      return "other";
  }
}

// What the page and the tests use of it.
export { linkKind };
