// The server's copy, searched (`#605`, `#607` and `#608 server-side-search`; `#624
// voice-page-modularity`, moved from the legacy script into core/search). The one owner of a search
// sent to the server: the search key it answers, its ticket (so a late answer to an old search is
// dropped), what it found, which channels it covered, how much it examined and where the next page
// resumes, and whether it covers every channel the server answers for or only the one on screen
// (`#search-all-channels`, for the page's life). When the search text changes it waits
// `SERVER_SEARCH_DELAY_MS` before asking, so typing sends one request, not one per key. Which search
// the page is showing, the request itself and drawing what came back are the page's, passed in.

import type { Message, SearchResponse, SearchScope } from "../../contract/index.ts";
import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** How long the search text rests before the server is asked, in milliseconds. */
const SERVER_SEARCH_DELAY_MS = 400;

/** One search sent to the server, as far as it has come. */
export interface ServerSearchState {
  /** The search this answers (the page's key), or "" for none. */
  key: string;
  /** Counts searches started: an answer to an older one is dropped. */
  ticket: number;
  loading: boolean;
  /** Why the last ask failed, or "". */
  failed: string;
  hits: Message[];
  scopes: SearchScope[];
  /** Messages the server tested, over every page asked for. */
  examined: number;
  /** Where the next page continues (`below`), or null for none. */
  resume: number | null;
}

/** What the server search asks of the page. */
export interface ServerSearchDependencies {
  /** The timer the wait for the text to rest runs on. */
  readonly clock: ClockPort;
  /** The request path for the search on screen: its first page, or (`more`) the page below `resume`. */
  pathFor(more: boolean, resume: number | null): string;
  /** Ask the server; resolves with its decoded answer. */
  fetchAnswer(path: string): Promise<SearchResponse>;
  /** Draw the search's state again. */
  render(): void;
  /** Run `fn`, reporting a failure quietly rather than letting it escape a timer. */
  guard(fn: () => Promise<void>): void;
}

/** The server search's commands. */
export interface ServerSearch {
  /** The search as it stands: read, never written. */
  state(): Readonly<ServerSearchState>;
  /**
   * The page's search is now `key` ("" for none). The same search: drawn again, nothing asked. Another:
   * started afresh (loading where `open`, the server's copy being open to this device), drawn, and asked
   * for once the text has rested, where there is one to ask.
   */
  schedule(key: string, open: boolean): void;
  /** Ask for the search's first page, or (`more`) the next one. */
  run(more: boolean): Promise<void>;
  /** The next page, where there is one and nothing is being asked already. */
  more(): Promise<void>;
  /** Forget which search this answers, so the next `schedule` starts it afresh (the copy it searched grew). */
  forget(): void;
  /** Whether the search covers every channel the server answers for, not only the one on screen. Off at first. */
  allChannels(): boolean;
  /**
   * All channels ticked or unticked, as `Boolean(on)` says; kept for the page's life, whatever is searched.
   * Nothing is drawn or asked: the page schedules its search again.
   */
  chooseAllChannels(on: unknown): void;
}

/** Nothing searched yet. */
export function createServerSearch(deps: ServerSearchDependencies): ServerSearch {
  let serverSearch: ServerSearchState = {
    key: "", ticket: 0, loading: false, failed: "", hits: [], scopes: [], examined: 0, resume: null,
  };
  let serverSearchTimer: TimerHandle | null = null;
  /** Whether the search covers every channel (`#search-all-channels`). */
  let searchAllChannels = false;

  async function run(more: boolean): Promise<void> {
    const state = serverSearch;
    const ticket = state.ticket;
    const path = deps.pathFor(more, state.resume);
    state.loading = true;
    deps.render();
    try {
      const answer = await deps.fetchAnswer(path);
      if (serverSearch.ticket !== ticket) return;
      const known = new Set(state.hits.map((hit) => `${hit.channel_id} ${hit.id}`));
      state.hits = [...state.hits, ...answer.hits.filter((hit) => !known.has(`${hit.channel_id} ${hit.id}`))];
      state.scopes = answer.scopes;
      state.examined += answer.scopes.reduce((sum, scope) => sum + scope.examined, 0);
      state.resume = typeof answer.resume_below_ms === "number" ? answer.resume_below_ms : null;
      state.failed = "";
    } catch (error) {
      if (serverSearch.ticket !== ticket) return;
      const said = error as { detail?: unknown; message?: unknown } | null;
      state.failed = String((said && (said.detail || said.message)) || "the server did not answer");
    } finally {
      if (serverSearch.ticket === ticket) {
        state.loading = false;
        deps.render();
      }
    }
  }

  return {
    state: () => serverSearch,
    schedule(key, open) {
      // The same search: drawn again all the same, so the line follows what the page has loaded since
      // and a hit the page now holds goes.
      if (key === serverSearch.key) {
        deps.render();
        return;
      }
      if (serverSearchTimer !== null) deps.clock.clearTimeout(serverSearchTimer);
      serverSearchTimer = null;
      serverSearch = { key, ticket: serverSearch.ticket + 1, loading: key !== "" && open, failed: "",
        hits: [], scopes: [], examined: 0, resume: null };
      deps.render();
      if (key === "" || !open) return;
      serverSearchTimer = deps.clock.setTimeout(() => {
        serverSearchTimer = null;
        deps.guard(() => run(false));
      }, SERVER_SEARCH_DELAY_MS);
    },
    run,
    forget() {
      serverSearch.key = "";
    },
    allChannels: () => searchAllChannels,
    chooseAllChannels(on) {
      searchAllChannels = Boolean(on);
    },
    more() {
      if (serverSearch.resume === null || serverSearch.loading) return Promise.resolve();
      return run(true);
    },
  };
}

// What the page and the tests use of it.
export { SERVER_SEARCH_DELAY_MS };
