// Settings > Labels (`#1825 vibe-talk-hashtag-labels`): browse and search every
// hashtag label in the loaded history, combine spellings into groups, and jump
// to the filtered view. Counts are the server's held copy only — the list says
// "in loaded history", never all-time (coordb amendment). Checkbox selection
// persists across filter changes (owner thread 167), because combining means
// search, check, search again, check.

/** One label row as `GET /api/v1/labels` serves it. */
export interface LabelRow {
  readonly name: string;
  readonly count: number;
  readonly group_id?: number | null;
}

/** One label group as the API serves it. */
export interface LabelGroupRow {
  readonly id: number;
  readonly display: string;
  readonly spellings: readonly string[];
}

/** What the labels browser asks of the page. */
export interface LabelsDependencies {
  lookup(id: string): HTMLElement;
  request(path: string, options: { method: string; body?: unknown }): Promise<unknown>;
  /** Open the search filtered to `label` (and leave Settings). */
  openLabel(label: string): void;
  guard(run: () => Promise<unknown>): () => void;
}

/** The labels browser's commands. */
export interface LabelsView {
  /** (Re)read the labels from the server and draw the list. */
  refresh(): Promise<void>;
  /** Every spelling a label slug may mean (its whole group), for search. */
  expand(slug: string): string[];
}

/** Mount the browser: first read at once, then on `refresh`. */
export function mountLabels(deps: LabelsDependencies): LabelsView {
  let labels: LabelRow[] = [];
  let groups: LabelGroupRow[] = [];
  let sortByCount = true;
  /** Checked spellings. Module state, so filtering never clears it. */
  const checked = new Set<string>();
  /** The group the dialog edits, or null when combining fresh. */
  let editing: number | null = null;

  const el = (id: string): HTMLElement => deps.lookup(id);
  const input = (id: string): HTMLInputElement => deps.lookup(id) as HTMLInputElement;

  function groupOf(row: LabelRow): LabelGroupRow | undefined {
    return row.group_id == null ? undefined : groups.find((group) => group.id === row.group_id);
  }

  function draw(): void {
    const filter = input("labels-filter").value.trim().toLowerCase();
    const shown = labels
      .filter((row) => filter === "" || row.name.includes(filter))
      .sort((a, b) =>
        sortByCount ? b.count - a.count || a.name.localeCompare(b.name) : a.name.localeCompare(b.name),
      );
    const list = el("labels-list");
    list.replaceChildren(
      ...shown.map((row) => {
        const item = document.createElement("li");
        item.className = "label-row";
        const box = document.createElement("input");
        box.type = "checkbox";
        box.checked = checked.has(row.name);
        box.setAttribute("aria-label", `Select #${row.name} to combine`);
        box.addEventListener("change", () => {
          if (box.checked) checked.add(row.name);
          else checked.delete(row.name);
          drawCombine();
        });
        const name = document.createElement("button");
        name.type = "button";
        name.className = "label-name";
        name.textContent = `#${row.name}`;
        name.title = `Show only #${row.name}`;
        name.addEventListener("click", () => deps.openLabel(row.name));
        const count = document.createElement("span");
        count.className = "label-count";
        count.textContent = `${row.count} in loaded history`;
        item.append(box, name, count);
        const group = groupOf(row);
        if (group) {
          const marker = document.createElement("button");
          marker.type = "button";
          marker.className = "label-group-marker";
          marker.textContent = "(G)";
          marker.title = `In the group "${group.display}" — edit it`;
          marker.setAttribute("aria-label", `Edit the label group ${group.display}`);
          marker.addEventListener("click", () => openDialog(group));
          item.append(marker);
        }
        return item;
      }),
    );
    el("labels-state").textContent =
      labels.length === 0
        ? "No hashtag labels in the loaded history yet. Type one, like #kvm, at the end of a message."
        : `${labels.length} label${labels.length === 1 ? "" : "s"} in the loaded history. Tick spellings that mean the same thing, then Combine.`;
    drawCombine();
  }

  function drawCombine(): void {
    const button = input("labels-combine");
    button.textContent = `Combine (${checked.size})`;
    button.disabled = checked.size < 2 && editing === null;
  }

  /** Open the dialog: fresh combine from the checks, or `group` to edit it. */
  function openDialog(group?: LabelGroupRow): void {
    editing = group ? group.id : null;
    const picked = group
      ? [...group.spellings]
      : [...checked].sort(
          (a, b) =>
            (labels.find((row) => row.name === b)?.count || 0) -
            (labels.find((row) => row.name === a)?.count || 0),
        );
    // The name starts as the most-occurring variant (owner thread 167).
    input("label-group-name").value = group
      ? group.display
      : picked[0] || "";
    (el("label-group-spellings") as HTMLTextAreaElement).value = picked.join("\n");
    el("label-group-title").textContent = group ? `Edit group "${group.display}"` : "Combine labels";
    (el("label-group-dissolve") as HTMLElement).hidden = !group;
    el("label-group-state").textContent = "";
    el("label-group-dialog").hidden = false;
  }

  function closeDialog(): void {
    el("label-group-dialog").hidden = true;
    editing = null;
  }

  async function save(): Promise<void> {
    const display = input("label-group-name").value.trim().toLowerCase();
    const spellings = (el("label-group-spellings") as HTMLTextAreaElement).value
      .split("\n")
      .map((line) => line.trim().replace(/^#/, "").toLowerCase())
      .filter(Boolean);
    try {
      const answer = (await deps.request(
        editing === null ? "/api/v1/label-groups" : `/api/v1/label-groups/${editing}`,
        { method: editing === null ? "POST" : "PUT", body: { display, spellings } },
      )) as { groups?: LabelGroupRow[] };
      groups = answer.groups || [];
      checked.clear();
      closeDialog();
      await refresh();
    } catch (error) {
      const refusal = error as { detail?: string; message?: string };
      el("label-group-state").textContent =
        refusal.detail || refusal.message || "The server did not answer.";
    }
  }

  async function dissolve(): Promise<void> {
    if (editing === null) return;
    try {
      const answer = (await deps.request(`/api/v1/label-groups/${editing}`, {
        method: "DELETE",
      })) as { groups?: LabelGroupRow[] };
      groups = answer.groups || [];
      closeDialog();
      await refresh();
    } catch (error) {
      const refusal = error as { detail?: string; message?: string };
      el("label-group-state").textContent =
        refusal.detail || refusal.message || "The server did not answer.";
    }
  }

  async function refresh(): Promise<void> {
    try {
      const answer = (await deps.request("/api/v1/labels", { method: "GET" })) as {
        labels?: LabelRow[];
        groups?: LabelGroupRow[];
      };
      labels = answer.labels || [];
      groups = answer.groups || [];
      draw();
    } catch {
      el("labels-state").textContent = "Labels could not be read from this server.";
    }
  }

  input("labels-filter").addEventListener("input", draw);
  input("labels-sort").addEventListener("click", () => {
    sortByCount = !sortByCount;
    const button = input("labels-sort");
    button.textContent = sortByCount ? "Sort: most used" : "Sort: A–Z";
    button.setAttribute("aria-pressed", String(!sortByCount));
    draw();
  });
  input("labels-combine").addEventListener("click", () => openDialog());
  input("label-group-save").addEventListener("click", deps.guard(save));
  input("label-group-dissolve").addEventListener("click", deps.guard(dissolve));
  input("label-group-close").addEventListener("click", closeDialog);
  void refresh();

  return {
    refresh,
    expand(slug: string): string[] {
      const group = groups.find(
        (held) => held.display === slug || held.spellings.includes(slug),
      );
      return group ? [...group.spellings] : [slug];
    },
  };
}
