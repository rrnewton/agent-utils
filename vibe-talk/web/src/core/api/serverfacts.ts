// What the server has said about itself (`#457 settings-server-side`, `#528 thread-card-markdown-and-cap`,
// `#624 voice-page-modularity`, moved from the legacy script into core/api): the scope it saw on the saved
// token, whether it names threads, whether its `/client-config` has answered in this page (as opposed to the
// saved shell standing in), and the page build it last said it serves. What each changes is the page's.

/** The facts' commands and questions. */
export interface ServerFacts {
  /**
   * `read` or `write`: the scope the server saw on the saved token. Null from an older server that does not
   * say, which keeps the old behaviour of asking and reading the refusal.
   */
  tokenScope(): "read" | "write" | null;
  /** Take the scope client-config said (`token_scope`): anything but `read` or `write` is none. */
  setTokenScope(said: unknown): void;
  /** Whether this server names threads (`thread_names_supported`): whether a summary may be on its way. */
  threadNames(): boolean;
  setThreadNames(on: boolean): void;
  /** Whether Discuss can run here (`discuss_available`): a direct LLM summarizer is configured. */
  discussAvailable(): boolean;
  setDiscussAvailable(on: boolean): void;
  /** Whether `/client-config` has answered in this page. */
  configApplied(): boolean;
  /** `/client-config` has answered. */
  markConfigApplied(): void;
  /**
   * The deployment's internal link hosts (`internal_link_hosts`, `#2007
   * vibe-talk-speech-short-links`), or none before client-config has said, or
   * where the deployment configured none: no host is internal then.
   */
  internalLinkHosts(): readonly string[];
  /** Take the internal link hosts client-config said: host names only, settled for matching. */
  setInternalLinkHosts(said: unknown): void;
  /** The page build the server last said it serves (`page_build`), or "" before a read has said. */
  pageBuild(): string;
  /** A read said the server serves `build`: whether that is news, a build said and not the one held. */
  notePageBuild(build: string): boolean;
}

/** Nothing said yet. */
export function createServerFacts(): ServerFacts {
  let tokenScope: "read" | "write" | null = null;
  let threadNamesSupported = false;
  let discussSupported = false;
  let clientConfigApplied = false;
  let serverPageBuild = "";
  let linkHosts: readonly string[] = [];

  return {
    tokenScope: () => tokenScope,
    setTokenScope(said) {
      tokenScope = said === "read" || said === "write" ? said : null;
    },
    threadNames: () => threadNamesSupported,
    setThreadNames(on) {
      threadNamesSupported = on;
    },
    discussAvailable: () => discussSupported,
    setDiscussAvailable(on) {
      discussSupported = on;
    },
    configApplied: () => clientConfigApplied,
    markConfigApplied() {
      clientConfigApplied = true;
    },
    internalLinkHosts: () => linkHosts,
    setInternalLinkHosts(said) {
      if (!Array.isArray(said)) {
        linkHosts = [];
        return;
      }
      const settled: string[] = [];
      for (const entry of said) {
        if (typeof entry !== "string") continue;
        const host = entry.trim().toLowerCase().replace(/^www\./, "").replace(/\.$/, "");
        if (host !== "" && !settled.includes(host)) settled.push(host);
      }
      linkHosts = settled;
    },
    pageBuild: () => serverPageBuild,
    notePageBuild(build) {
      if (!build || build === serverPageBuild) return false;
      serverPageBuild = build;
      return true;
    },
  };
}
