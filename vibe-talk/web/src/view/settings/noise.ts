// Settings' automatic-read rules (`#196 auto-read-noise`; `#624 voice-page-modularity`).
// Moved from the legacy script. The server judges messages; the channel owner supplies the tally
// and re-reads after a rule changes. This editor keeps only the server's reported rules.

/** What the automatic-read rules editor asks of the page. */
export interface NoiseRulesDependencies {
  lookup(id: string): HTMLElement;
  request(path: string, options: { method: string; body: unknown }): Promise<unknown>;
  guard(run: () => Promise<void>): () => void;
  status(text: string): void;
  tally(): { readonly matched: number; readonly total: number };
  selectedChannel(): string | null;
  /** The existing mutation read, keeping the reader's place. Its guard is applied by the editor. */
  reloadChannel(): Promise<void>;
}

/** The editor's named commands; its rules are private. */
export interface NoiseRulesEditor {
  apply(view: unknown): void;
  render(): void;
  renderCount(): void;
  save(rules: string[], said: string): Promise<void>;
  add(): Promise<void>;
}

/** No rules reported yet. Construction neither reads nor draws. */
export function mountNoiseRules(deps: NoiseRulesDependencies): NoiseRulesEditor {
  /** The rules as the server last reported them, or null when it does not say. */
  let noiseRules: string[] | null = null;

  /** The server's own sentence about how a rule matches, quoted beside the list. */
  let noiseMatching = "";

  /** Take down what the server said the rules are — from client-config, or from a save. */
  function applyNoiseRules(view: unknown): void {
    const reported = view as { readonly rules?: unknown; readonly matching?: unknown } | null | undefined;
    if (reported && Array.isArray(reported.rules)) {
      noiseRules = reported.rules.map(String);
      noiseMatching = typeof reported.matching === "string" ? reported.matching : "";
    } else {
      noiseRules = null;
      noiseMatching = "";
    }
    renderNoiseRules();
  }

  /** Draw the editor from `noiseRules`: the sentence, one row per rule, and the count. */
  function renderNoiseRules(): void {
    deps.lookup("noise-editor").hidden = noiseRules === null;
    if (noiseRules === null) {
      // Said rather than left blank: an older server, or a store that is not answering, and the
      // owner looking for the list should learn which side of that he is on.
      deps.lookup("noise-state").textContent =
        "This server does not say which messages it reads automatically, so there is nothing to edit here.";
      return;
    }
    deps.lookup("noise-matching").textContent = noiseMatching;
    deps.lookup("noise-rule-list").replaceChildren(...noiseRules.map(noiseRuleRow));
    renderNoiseCount();
  }

  /**
   * One rule and its Remove.
   *
   * The rule is the OWNER's own text, shown as he typed it, through `textContent` like everything
   * else this page draws. Removing sends the whole list without it, which is how the server takes
   * every change: one list, replaced, so two devices cannot leave half an edit each.
   */
  function noiseRuleRow(rule: string, index: number): HTMLLIElement {
    const li = document.createElement("li");
    li.className = "noise-rule";
    const text = document.createElement("span");
    text.className = "noise-rule-text";
    text.textContent = rule;
    const remove = document.createElement("button");
    remove.className = "secondary noise-rule-remove";
    remove.setAttribute("type", "button");
    remove.setAttribute("title", "Stop reading messages like this automatically");
    remove.textContent = "Remove";
    remove.addEventListener(
      "click",
      deps.guard(() =>
        saveNoiseRules(
          (noiseRules || []).filter((_held, at) => at !== index),
          `Removed. Messages reading "${rule}" count as unread again.`
        )
      )
    );
    li.append(text, remove);
    return li;
  }

  /**
   * The live count under the list: "Matches 9 of the 50 loaded messages."
   *
   * Recounted wherever the loaded messages change — `renderChannelRows`, a to-do read, an arrival —
   * so it moves when the owner adds a rule and the channel is re-read under it.
   */
  function renderNoiseCount(): void {
    if (noiseRules === null) {
      return;
    }
    const { matched, total } = deps.tally();
    deps.lookup("noise-count").textContent =
      total === 0
        ? "No messages are loaded yet, so there is nothing to count."
        : `Matches ${matched} of the ${total} loaded message${total === 1 ? "" : "s"}.`;
  }

  /**
   * Replace the server's list, take back what it STORED, and re-read the channel under it.
   *
   * The re-read is the point of the third step. Every loaded message was judged under the old list,
   * and the page has no matcher of its own to re-judge them with — so the server is asked again.
   */
  async function saveNoiseRules(rules: string[], said: string): Promise<void> {
    const payload = await deps.request("/api/v1/noise-rules", { method: "PUT", body: { rules } });
    applyNoiseRules(payload);
    deps.lookup("noise-state").textContent = said;
    deps.status(said);
    if (deps.selectedChannel()) {
      deps.guard(() => deps.reloadChannel())();
    }
  }

  /** Add what is in the field as a rule. The server trims it and drops a duplicate. */
  async function addNoiseRule(): Promise<void> {
    const typed = (deps.lookup("noise-rule-new") as HTMLInputElement).value.trim();
    if (!typed) {
      deps.lookup("noise-state").textContent = "Type the message to read automatically, then Add.";
      return;
    }
    await saveNoiseRules(
      [...(noiseRules || []), typed],
      `Added. A message reading "${typed}" counts as read from now on.`
    );
    (deps.lookup("noise-rule-new") as HTMLInputElement).value = "";
  }

  return { apply: applyNoiseRules, render: renderNoiseRules, renderCount: renderNoiseCount,
    save: saveNoiseRules, add: addNoiseRule };
}
