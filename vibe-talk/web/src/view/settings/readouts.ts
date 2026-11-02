// Settings' short readouts (`#624 voice-page-modularity`), moved from the legacy script.
// These draw facts owned by the page or its feature owners; the Slack-link control keeps its
// device setting through core/settings. Construction draws nothing and reads no lazy dependency.

import { DEVICE_SETTINGS } from "../../core/settings/index.ts";
import type { DeviceSettings } from "../../core/settings/index.ts";
import { setTextIfChanged } from "../dom/index.ts";

export interface SettingsReadoutsDependencies {
  lookup(id: string): HTMLElement;
  deploymentHost(): string;
  pageOutOfDate(): boolean;
  readonly pageBuild: string;
  readonly serverPageBuild: string;
  readonly voice: { readonly name: string; readonly settings_url: string | null; readonly instance_id: string | null };
  renderFolds(): void;
  deviceSettings(): DeviceSettings;
}

export interface SettingsReadouts {
  serverHost(): void;
  pageBuild(): void;
  agentLink(): void;
  speechPrep(enabled: unknown): void;
  slackOpenChanged(): void;
}

/** Made where the header first draws; later feature values are read only by their own readouts. */
export function mountSettingsReadouts(deps: SettingsReadoutsDependencies): SettingsReadouts {
  /**
   * `#372 settings-header-layout`. The host on one line, whole in its tooltip, and in two parts: the name,
   * cut with an ellipsis where the line is too short for it, and the port, never cut, since the port is
   * what tells two instances on one host apart (`#342 settings-server-hostname`) and a phone has no
   * tooltip to show it in. `[::1]:8443` splits at its last colon; `[::1]` has no port.
   */
  function renderServerHost(): void {
    const host = deps.deploymentHost();
    const [, name, port] = /^(.+?)(:\d+)?$/.exec(host) || ["", host, ""];
    const namePart = document.createElement("span");
    namePart.className = "settings-server-name";
    namePart.textContent = name!;
    const portPart = document.createElement("span");
    portPart.className = "settings-server-port";
    portPart.textContent = port || "";
    deps.lookup("settings-server-host").replaceChildren(namePart, portPart);
    deps.lookup("settings-server-host").style.setProperty("--port-width", `${(port || "").length}ch`);
    deps.lookup("settings-server-host").title = host;
  }

  /** The reload chip shown exactly while this page is out of date, and Settings saying which build it is. */
  function renderPageBuild(): void {
    const stale = deps.pageOutOfDate();
    const chip = deps.lookup("page-update");
    if (chip.hidden === stale) chip.hidden = !stale;
    const short = (build: string): string => build.slice(0, 8);
    setTextIfChanged(deps.lookup("settings-page-build"), deps.pageBuild === "" ? "" : stale
      ? `This page is build ${short(deps.pageBuild)}; the server now serves ${short(deps.serverPageBuild)}. Reload to run it.`
      : `This page is build ${short(deps.pageBuild)}.`);
  }

  /**
   * Show the link when there is an agent to link to, and hide the whole group when there is not.
   *
   * Hidden rather than greyed: an older server does not report the id at all, and a dead link
   * labelled "open this agent's configuration" is worse than no link — it is a control that is
   * present, looks live, and goes somewhere wrong.
   */
  function renderAgentLink(): void {
    const url = deps.voice.settings_url;
    const id = deps.voice.instance_id;
    deps.lookup("agent-settings").hidden = !url;
    // `#351 settings-polish`. A fold shown or hidden with it changes what Expand all is about.
    deps.renderFolds();
    if (!url) {
      return;
    }
    deps.lookup("agent-id").textContent = id || deps.voice.name;
    deps.lookup("agent-console-link").textContent = `Open ${deps.voice.name} configuration`;
    deps.lookup("agent-console-link").setAttribute("href", url);
  }

  /**
   * Say which arm this deployment is in for the rewrite-for-speech pass.
   *
   * Reported, never controlled: the rewrite happens on the server before a message reaches this page
   * at all, and the voice agent's own tool calls never pass through this browser. A switch here
   * could only govern one of the three paths that read a message out.
   *
   * `#332 settings-voice-agent-section`. The line opens Help's How messages are said, rather than
   * standing on Settings as a group with no control in it; the paragraphs under it say what the
   * rewrite does, so the line only says which arm. `#347 settings-prose` renamed the entry "How
   * messages are pre-processed for speech", the words of the link under Reading full messages in
   * channel view that opens it.
   *
   * A server too old to send the field is NOT reported as off. Older servers prepared bodies
   * unconditionally, so "off" would be a false statement about a deployment that is in fact doing
   * the rewriting — and the reader is looking at this line precisely to find out which arm they are
   * in. Only an explicit `false` says off.
   */
  function renderSpeechPrepState(enabled: unknown): void {
    const state = deps.lookup("speech-prep-state");
    if (!state) {
      return;
    }
    state.textContent =
      enabled === false
        ? "Off on this server: messages are read out exactly as they were typed."
        : "On for this server: messages are rewritten for the ear, and the channel view still " +
          "shows what was typed.";
  }

  function slackOpenChanged(): void {
    const chosen = (deps.lookup("slack-open-in") as HTMLSelectElement).value === "app" ? "app" : "browser";
    let saved = false;
    try {
      deps.deviceSettings().keep(DEVICE_SETTINGS.slackOpen, chosen);
      saved = deps.deviceSettings().keptAs(DEVICE_SETTINGS.slackOpen, chosen);
    } catch (_error) {
      saved = false;
    }
    deps.lookup("slack-open-state").textContent = saved
      ? chosen === "app"
        ? "Saved. Open in Slack goes to the app from here; copied links are web addresses either way."
        : "Saved. Open in Slack opens the message in a browser tab."
      : "This browser refused to store the setting, so links will open in the browser after a reload.";
  }

  return { serverHost: renderServerHost, pageBuild: renderPageBuild, agentLink: renderAgentLink,
    speechPrep: renderSpeechPrepState, slackOpenChanged };
}
