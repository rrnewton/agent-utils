// Canonical GitHub document addresses, shared by message navigation and the reader.
const MAX_ADDRESS = 2048;

/** An owner/repository name as the server's `valid_name` accepts it. */
export function validName(part: string): boolean {
  return part.length >= 1 && part.length <= 100 && /^[A-Za-z0-9._+\-]+$/.test(part);
}

/**
 * How many dots the segment is made of (`.` and `%2e` spellings), or -1 where it
 * is anything else: the server's `dot_segment` suspicion, counted, so `.`/`..`
 * navigation and a refused `...` read off the same walk.
 */
export function dotCount(segment: string): number {
  let rest = segment;
  let dots = 0;
  while (rest !== "") {
    if (rest.startsWith(".")) {
      rest = rest.slice(1);
      dots += 1;
    } else if (rest.slice(0, 3).toLowerCase() === "%2e") {
      rest = rest.slice(3);
      dots += 1;
    } else {
      return -1;
    }
  }
  return segment === "" ? -1 : dots;
}

/**
 * The canonical document key for an absolute GitHub address, or null where the
 * server would not recognise it as a document. A step-for-step mirror of the
 * blob and raw arms of `recognize_url` in `src/documents.rs` (strict scheme and
 * host, no decoding, dot segments refused), so a key minted here is byte-for-byte
 * the key the server mints from the same address: `github:owner/repo@ref:path`.
 */
export function githubKeyForUrl(address: string): string | null {
  if (address.length > MAX_ADDRESS || /[\\ \u0000-\u001f\u007f]/.test(address)) return null;
  const lower = address.toLowerCase();
  let rest: string;
  if (lower.startsWith("https://")) rest = address.slice("https://".length);
  else if (lower.startsWith("http://")) rest = address.slice("http://".length);
  else return null;
  const hostEnd = rest.search(/[/?#]/);
  const host = (hostEnd === -1 ? rest : rest.slice(0, hostEnd)).toLowerCase();
  let tail = hostEnd === -1 ? "" : rest.slice(hostEnd);
  tail = tail.split("#")[0] ?? "";
  let path = tail.split("?")[0] ?? "";
  if (path.startsWith("/")) path = path.slice(1);
  if (path.endsWith("/")) path = path.slice(0, -1);
  const segments = path === "" ? [] : path.split("/");
  if (segments.some((segment) => segment === "" || dotCount(segment) !== -1)) return null;
  if (host === "github.com" || host === "www.github.com") {
    const [owner, repo, blob, ref, ...file] = segments;
    if (owner === undefined || repo === undefined || ref === undefined || blob !== "blob") return null;
    if (!validName(owner) || !validName(repo) || file.length === 0) return null;
    return `github:${owner}/${repo}@${ref}:${file.join("/")}`;
  }
  if (host === "raw.githubusercontent.com") {
    const [owner, repo, ref, ...file] = segments;
    if (owner === undefined || repo === undefined || ref === undefined) return null;
    if (!validName(owner) || !validName(repo) || file.length === 0) return null;
    return `github:${owner}/${repo}@${ref}:${file.join("/")}`;
  }
  return null;
}
