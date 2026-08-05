(function () {
  "use strict";

  var DATA_URL = "data/timeline.json";
  var SVG_NS = "http://www.w3.org/2000/svg";
  var ROW_HEIGHT = 54;
  var PHASE_TOP = 7;
  var PHASE_HEIGHT = 38;
  var COMPACT_PHASE_TOP = 16;
  var COMPACT_PHASE_HEIGHT = 31;
  var COMPACT_LABEL_WIDTH = 72;
  var STATE_HEIGHT = 6;
  var MIN_VIEW_MS = 1000;
  var PHASE_COLORS = [
    "#287ca3",
    "#555ec4",
    "#7c4db5",
    "#267d72",
    "#a06931",
    "#386da8",
    "#8d4d79",
    "#39794d"
  ];
  var STATE_COLORS = {
    active: "#3bc983",
    tool: "#e7ad43",
    waiting: "#5d8ff4",
    idle: "#68758b",
    blocked: "#ef6470"
  };
  var EDGE_COLORS = {
    spawn: "#4bc6e8",
    message: "#a77bf3",
    result: "#3bc983",
    other: "#5d8ff4"
  };

  function byId(id) {
    var element = document.getElementById(id);
    if (!element) {
      throw new Error("Missing required element #" + id);
    }
    return element;
  }

  var dom = {
    card: document.querySelector(".timeline-card"),
    siteTitle: byId("site-title"),
    identityDetails: byId("site-identity-details"),
    identityList: byId("site-identity-list"),
    meta: byId("dataset-meta"),
    teamFilter: byId("team-filter"),
    search: byId("search"),
    fit: byId("fit"),
    glossaryOpen: byId("glossary-open"),
    summaryMenu: byId("summary-menu"),
    summaryFiles: byId("summary-files"),
    perAgentTracks: byId("per-agent-tracks"),
    showGlobalMessages: byId("show-global-messages"),
    showHighlightedMessages: byId("show-highlighted-messages"),
    viewRange: byId("view-range"),
    rollupRow: byId("rollup-row"),
    rollupTrack: byId("rollup-track"),
    axis: byId("time-axis"),
    scroll: byId("timeline-scroll"),
    svg: byId("timeline-svg"),
    empty: byId("empty-state"),
    statsRange: byId("stats-range-label"),
    statsValues: byId("stats-values"),
    tooltip: byId("tooltip"),
    tooltipTitle: byId("tooltip-title"),
    tooltipBody: byId("tooltip-body"),
    tooltipStats: byId("tooltip-stats"),
    modalBackdrop: byId("modal"),
    modalTitle: byId("modal-title"),
    modalEyebrow: byId("modal-eyebrow"),
    modalClose: byId("modal-close"),
    modalSummary: byId("modal-summary"),
    modalTabs: byId("modal-tabs"),
    modalContent: byId("modal-content"),
    contextMenu: byId("timeline-context-menu"),
    contextMenuTitle: byId("context-menu-title"),
    contextMenuActions: byId("context-menu-actions"),
    laneMenu: byId("lane-agent-menu"),
    laneMenuTitle: byId("lane-agent-menu-title"),
    laneMenuActions: byId("lane-agent-menu-actions"),
    loadError: byId("load-error")
  };

  var app = {
    data: null,
    viewStart: 0,
    viewEnd: 1,
    navigationRange: { start_ms: 0, end_ms: 1 },
    width: 1000,
    labelWidth: 238,
    chartWidth: 762,
    rows: [],
    laneCount: 0,
    rowByAgent: new Map(),
    phasesByAgent: new Map(),
    agentsById: new Map(),
    teamBySlug: new Map(),
    glossaryById: new Map(),
    artifactsById: new Map(),
    artifactCatalogState: "legacy",
    artifactCatalogError: "",
    axisTicks: [],
    selectedTeam: "",
    query: "",
    perAgentTracks: false,
    showGlobalMessages: false,
    showHighlightedMessages: true,
    selection: null,
    drag: null,
    suppressClickUntil: 0,
    renderQueued: false,
    detailRequest: 0,
    modalRestoreFocus: null,
    laneMenuAnchor: null,
    timezone: "UTC",
    timezoneStatus: "archive fallback"
  };

  var timelineCore = window.AgentTimelineCore;

  var markdownRenderer = typeof window.markdownit === "function"
    ? window.markdownit({
        html: false,
        linkify: true,
        typographer: false,
        breaks: false
      })
    : null;

  function array(value) {
    return Array.isArray(value) ? value : [];
  }

  function number(value, fallback) {
    return Number.isFinite(value) ? Number(value) : fallback;
  }

  function text(value, fallback) {
    return typeof value === "string" ? value : (fallback || "");
  }

  function htmlElement(tag, className, content) {
    var element = document.createElement(tag);
    if (className) {
      element.className = className;
    }
    if (content !== undefined && content !== null) {
      element.textContent = String(content);
    }
    return element;
  }

  function secureMarkdownLinks(container) {
    container.querySelectorAll("a[href]").forEach(function (link) {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    });
  }

  function markdownElement(source, className, inline) {
    var container = htmlElement(
      inline ? "span" : "article",
      (className ? className + " " : "") + "rendered-markdown"
    );
    if (!markdownRenderer) {
      container.textContent = text(source);
      container.classList.add("markdown-fallback");
      return container;
    }
    container.innerHTML = inline
      ? markdownRenderer.renderInline(text(source))
      : markdownRenderer.render(text(source));
    secureMarkdownLinks(container);
    linkKnownGlossaryTerms(container);
    return container;
  }

  function glossaryId(value) {
    var candidate = text(value);
    return /^term-[a-z0-9-]+$/.test(candidate) ? candidate : "";
  }

  function glossaryBoundaryCharacter(value) {
    return Boolean(value) && /[A-Za-z0-9_]/.test(value);
  }

  function glossaryMatches(value) {
    var candidates = [];
    app.glossaryById.forEach(function (entry, id) {
      var term = text(entry.term);
      if (!term) {
        return;
      }
      var from = 0;
      while (from < value.length) {
        var start = value.indexOf(term, from);
        if (start < 0) {
          break;
        }
        var end = start + term.length;
        var before = start > 0 ? value.charAt(start - 1) : "";
        var after = end < value.length ? value.charAt(end) : "";
        if (!(glossaryBoundaryCharacter(term.charAt(0)) && glossaryBoundaryCharacter(before)) &&
            !(glossaryBoundaryCharacter(term.charAt(term.length - 1)) &&
              glossaryBoundaryCharacter(after))) {
          candidates.push({ start: start, end: end, id: id, entry: entry });
        }
        from = start + Math.max(1, term.length);
      }
    });
    candidates.sort(function (left, right) {
      return left.start - right.start ||
        (right.end - right.start) - (left.end - left.start) ||
        left.id.localeCompare(right.id);
    });
    var accepted = [];
    var cursor = 0;
    candidates.forEach(function (candidate) {
      if (candidate.start >= cursor) {
        accepted.push(candidate);
        cursor = candidate.end;
      }
    });
    return accepted;
  }

  function linkKnownGlossaryTerms(container) {
    if (!app.glossaryById.size || !document.createTreeWalker) {
      return;
    }
    var walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    var nodes = [];
    var current;
    while ((current = walker.nextNode())) {
      var parent = current.parentElement;
      if (parent && !parent.closest("a, button, code, pre, script, style, textarea")) {
        nodes.push(current);
      }
    }
    nodes.forEach(function (node) {
      var value = node.nodeValue || "";
      var matches = glossaryMatches(value);
      if (!matches.length || !node.parentNode) {
        return;
      }
      var fragment = document.createDocumentFragment();
      var cursor = 0;
      matches.forEach(function (match) {
        fragment.appendChild(document.createTextNode(value.slice(cursor, match.start)));
        var link = htmlElement("a", "glossary-term-link", value.slice(match.start, match.end));
        link.href = "#glossary/" + match.id;
        link.title = text(match.entry.context, "Open project glossary entry");
        link.dataset.glossaryId = match.id;
        link.addEventListener("click", function (event) {
          event.preventDefault();
          openGlossaryEntry(match.entry, true);
        });
        fragment.appendChild(link);
        cursor = match.end;
      });
      fragment.appendChild(document.createTextNode(value.slice(cursor)));
      node.parentNode.replaceChild(fragment, node);
    });
  }

  function safePullRequestUrl(value) {
    try {
      var url = new URL(text(value));
      if (url.protocol !== "https:" || url.hostname.toLowerCase() !== "github.com" ||
          url.username || url.password || !/^\/[^/]+\/[^/]+\/pull\/[1-9][0-9]*\/?$/.test(url.pathname)) {
        return "";
      }
      return url.href;
    } catch (_error) {
      return "";
    }
  }

  function referenceTextElement(source, references, className) {
    var content = text(source);
    var container = htmlElement("span", className || "");
    var cursor = 0;
    array(references)
      .slice()
      .sort(function (left, right) {
        return number(left.start, 0) - number(right.start, 0);
      })
      .forEach(function (reference) {
        var start = number(reference.start, -1);
        var end = number(reference.end, -1);
        var url = safePullRequestUrl(reference.url);
        if (!Number.isInteger(start) || !Number.isInteger(end) || start < cursor ||
            end <= start || end > content.length || !url) {
          return;
        }
        container.appendChild(document.createTextNode(content.slice(cursor, start)));
        var link = htmlElement("a", "pr-reference", content.slice(start, end));
        link.href = url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.title = text(
          reference.title,
          text(reference.repository, "GitHub") + " pull request #" + formatCount(reference.number)
        );
        if (text(reference.title)) {
          var state = reference.draft
            ? "Draft"
            : (text(reference.merged_at) ? "Merged" : text(reference.state, "Unknown state"));
          var branch = text(reference.base_ref) && text(reference.head_label)
            ? text(reference.base_ref) + " ← " + text(reference.head_label)
            : "";
          link.setAttribute(
            "aria-label",
            text(reference.repository, "GitHub") + " pull request #" +
              formatCount(reference.number) + ": " + text(reference.title)
          );
          link.addEventListener("pointerenter", function (event) {
            showTooltip(
              event,
              text(reference.title),
              text(reference.body_excerpt, "No pull request description available.") +
                (branch ? "\n\n" + branch : ""),
              state + " · " + text(reference.repository) +
                (text(reference.author) ? " · @" + text(reference.author) : "")
            );
          });
          link.addEventListener("pointermove", positionTooltip);
          link.addEventListener("pointerleave", hideTooltip);
        }
        container.appendChild(link);
        cursor = end;
    });
    container.appendChild(document.createTextNode(content.slice(cursor)));
    linkKnownGlossaryTerms(container);
    return container;
  }

  function svgElement(tag, attributes, content) {
    var element = document.createElementNS(SVG_NS, tag);
    if (attributes) {
      Object.keys(attributes).forEach(function (name) {
        element.setAttribute(name, String(attributes[name]));
      });
    }
    if (content !== undefined && content !== null) {
      element.textContent = String(content);
    }
    return element;
  }

  function clamp(value, minimum, maximum) {
    return Math.max(minimum, Math.min(maximum, value));
  }

  function rangesOverlap(startA, endA, startB, endB) {
    return endA >= startB && startA <= endB;
  }

  function normalizeKind(value, allowed, fallback) {
    var normalized = text(value).toLowerCase().replace(/[^a-z0-9_-]/g, "");
    return allowed.indexOf(normalized) >= 0 ? normalized : fallback;
  }

  function formatCount(value) {
    return Math.max(0, Number(value) || 0).toLocaleString();
  }

  function formatStatsInline(stats) {
    if (!stats || typeof stats !== "object") {
      return "";
    }
    return [
      formatCount(stats.user_prompts) + " prompts",
      formatCount(stats.agent_responses) + " responses",
      formatCount(stats.inter_agent_messages) + " inter-agent",
      formatCount(stats.tool_calls) + " tools"
    ].join(" · ");
  }

  function installTimezone(value) {
    var candidate = text(value);
    if (!candidate) {
      app.timezone = "UTC";
      app.timezoneStatus = "archive timezone missing; using UTC";
      return;
    }
    try {
      new Intl.DateTimeFormat(undefined, { timeZone: candidate }).format(new Date());
      app.timezone = candidate;
      app.timezoneStatus = "archive";
    } catch (_error) {
      app.timezone = "UTC";
      app.timezoneStatus = "archive timezone unsupported; using UTC";
    }
  }

  function dateFormatter(options) {
    var settings = Object.assign({}, options);
    settings.timeZone = app.timezone;
    return new Intl.DateTimeFormat(undefined, settings);
  }

  function formatFullTime(milliseconds) {
    if (!Number.isFinite(milliseconds)) {
      return "Unknown time";
    }
    return dateFormatter({
      year: "numeric",
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      timeZoneName: "short"
    }).format(new Date(milliseconds));
  }

  function formatCompactTime(milliseconds) {
    return dateFormatter({
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit"
    }).format(new Date(milliseconds));
  }

  function formatTick(milliseconds, span) {
    var options;
    if (span <= 6 * 60 * 60 * 1000) {
      options = { hour: "2-digit", minute: "2-digit", second: "2-digit" };
    } else if (span <= 3 * 24 * 60 * 60 * 1000) {
      options = { weekday: "short", hour: "2-digit", minute: "2-digit" };
    } else if (span <= 120 * 24 * 60 * 60 * 1000) {
      options = { month: "short", day: "2-digit", hour: "2-digit" };
    } else {
      options = { year: "numeric", month: "short", day: "2-digit" };
    }
    return dateFormatter(options).format(new Date(milliseconds));
  }

  function formatRange(start, end) {
    return formatCompactTime(start) + " — " + formatCompactTime(end);
  }

  function normalizeData(raw) {
    if (!raw || typeof raw !== "object") {
      throw new Error("timeline.json must contain a JSON object");
    }
    var normalized = {
      schema_version: raw.schema_version,
      generated_at: text(raw.generated_at),
      display_timezone: text(raw.display_timezone),
      display_timezone_source: text(raw.display_timezone_source, "legacy_team_data"),
      range: raw.range && typeof raw.range === "object" ? raw.range : {},
      teams: array(raw.teams),
      agents: array(raw.agents),
      phases: array(raw.phases),
      edges: array(raw.edges),
      events: array(raw.events),
      rollups: array(raw.rollups),
      summary_files: array(raw.summary_files),
      glossary: array(raw.glossary),
      glossary_path: text(raw.glossary_path),
      artifact_catalog_path: text(raw.artifact_catalog_path)
    };

    var inferredStart = Infinity;
    var inferredEnd = -Infinity;
    function recordTimestamp(value) {
      var timestamp = number(value, NaN);
      if (Number.isFinite(timestamp)) {
        inferredStart = Math.min(inferredStart, timestamp);
        inferredEnd = Math.max(inferredEnd, timestamp);
      }
    }
    normalized.agents.forEach(function (agent) {
      recordTimestamp(agent.start_ms);
      recordTimestamp(agent.end_ms);
    });
    normalized.phases.forEach(function (phase) {
      recordTimestamp(phase.start_ms);
      recordTimestamp(phase.end_ms);
    });
    normalized.events.forEach(function (event) {
      recordTimestamp(event.at_ms);
    });
    if (!Number.isFinite(inferredStart) || !Number.isFinite(inferredEnd)) {
      inferredStart = Date.now() - 3600000;
      inferredEnd = Date.now();
    }
    var start = number(normalized.range.start_ms, inferredStart);
    var end = number(normalized.range.end_ms, inferredEnd);
    if (end <= start) {
      end = start + 1;
    }
    normalized.range = { start_ms: start, end_ms: end };
    return normalized;
  }

  async function loadArtifactCatalog(data) {
    app.artifactsById.clear();
    app.artifactCatalogError = "";
    var path = text(data && data.artifact_catalog_path);
    if (!path) {
      app.artifactCatalogState = "legacy";
      return;
    }
    app.artifactCatalogState = "loading";
    try {
      var loaded = await fetchPath(path, "json");
      var catalog = loaded.content;
      if (!catalog || typeof catalog !== "object" ||
          number(catalog.schema_version, NaN) !== 1 ||
          !Array.isArray(catalog.artifacts)) {
        throw new Error("Artifact catalog is not a supported schema-1 catalog.");
      }
      catalog.artifacts.forEach(function (artifact) {
        if (!artifact || typeof artifact !== "object") {
          throw new Error("Artifact catalog entries must be objects.");
        }
        var id = text(artifact.artifact_id);
        if (!/^artifact-[a-z0-9-]+$/.test(id) || app.artifactsById.has(id)) {
          throw new Error("Artifact catalog contains an invalid or duplicate ID.");
        }
        app.artifactsById.set(id, artifact);
      });
      app.artifactCatalogState = "ready";
    } catch (error) {
      app.artifactsById.clear();
      app.artifactCatalogState = "error";
      app.artifactCatalogError = error instanceof Error ? error.message : String(error);
    }
  }

  function dataVersionLabel(value) {
    if (value === undefined || value === null || value === "") {
      return "schema unknown";
    }
    return "schema " + String(value);
  }

  function safeRepositoryUrl(value) {
    var raw = text(value);
    if (!raw) {
      return "";
    }
    try {
      var parsed = new URL(raw);
      return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.href : "";
    } catch (_error) {
      return "";
    }
  }

  function identitySourceLabel(value) {
    var source = text(value);
    if (source === "explicit") {
      return "explicit";
    }
    if (source === "session_metadata") {
      return "from session metadata";
    }
    return source.replace(/_/g, " ");
  }

  function aggregatedSiteIdentity(teams) {
    var projectsByKey = new Map();
    var projectOrder = [];
    var hostsByKey = new Map();
    var hostOrder = [];
    teams.forEach(function (team) {
      if (!team || typeof team !== "object") {
        return;
      }
      array(team.projects).forEach(function (project) {
        if (!project || typeof project !== "object") {
          return;
        }
        var label = text(project.label);
        var repositoryUrl = safeRepositoryUrl(project.repository_url);
        if (!label) {
          return;
        }
        var key = repositoryUrl || "label:" + label.toLowerCase();
        if (!projectsByKey.has(key)) {
          projectOrder.push(key);
        }
        var prior = projectsByKey.get(key);
        projectsByKey.set(key, {
          label: label,
          repository_url: repositoryUrl,
          primary: project.primary === true || Boolean(prior && prior.primary),
          source: text(project.source)
        });
      });
      array(team.hosts).forEach(function (host) {
        if (!host || typeof host !== "object") {
          return;
        }
        var hostname = text(host.hostname);
        var key = hostname.toLowerCase();
        if (!hostname || hostsByKey.has(key)) {
          return;
        }
        hostOrder.push(key);
        hostsByKey.set(key, {
          hostname: hostname,
          source: text(host.source)
        });
      });
    });
    var projects = projectOrder.map(function (key) { return projectsByKey.get(key); });
    projects.sort(function (left, right) {
      return Number(right.primary) - Number(left.primary);
    });
    return {
      projects: projects,
      hosts: hostOrder.map(function (key) { return hostsByKey.get(key); })
    };
  }

  function projectNode(project) {
    if (project.repository_url) {
      var link = htmlElement("a", "", project.label);
      link.href = project.repository_url;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      return link;
    }
    return htmlElement("span", "", project.label);
  }

  function renderSiteIdentity(data) {
    var identity = aggregatedSiteIdentity(data.teams);
    var primaryProjects = identity.projects.filter(function (project) {
      return project.primary;
    });
    var titleProject = identity.projects.length === 1
      ? identity.projects[0]
      : (primaryProjects.length === 1 ? primaryProjects[0] : null);
    var projectSummary;
    var projectVisible;
    if (titleProject) {
      projectSummary = projectNode(titleProject);
      projectVisible = titleProject.label;
    } else if (identity.projects.length > 1) {
      projectSummary = htmlElement("span", "", "multi-repo");
      projectVisible = "multi-repo";
    } else {
      var fallback = data.teams.length === 1 && data.teams[0]
        ? text(data.teams[0].label, text(data.teams[0].slug, "unknown project"))
        : (data.teams.length > 1 ? "multi-team" : "unknown project");
      projectSummary = htmlElement("span", "", fallback);
      projectVisible = fallback;
    }
    var children = [document.createTextNode("Agent Timeline: "), projectSummary];
    var hostVisible = "";
    if (identity.hosts.length === 1) {
      hostVisible = identity.hosts[0].hostname.split(".")[0];
    } else if (identity.hosts.length > 1) {
      hostVisible = "multi-host";
    }
    if (hostVisible) {
      children.push(document.createTextNode(", " + hostVisible));
    }
    dom.siteTitle.replaceChildren.apply(dom.siteTitle, children);
    var visibleTitle = "Agent Timeline: " + projectVisible + (hostVisible ? ", " + hostVisible : "");
    document.title = visibleTitle;

    var fullLines = [];
    if (identity.projects.length) {
      fullLines.push(
        "Projects: " + identity.projects.map(function (project) {
          return project.label + (project.repository_url ? " (" + project.repository_url + ")" : "");
        }).join(", ")
      );
    }
    if (identity.hosts.length) {
      fullLines.push("Hosts: " + identity.hosts.map(function (host) {
        return host.hostname;
      }).join(", "));
    }
    dom.siteTitle.title = fullLines.join("\n");
    dom.siteTitle.setAttribute("aria-label", visibleTitle + (fullLines.length ? ". " + fullLines.join(". ") : ""));

    var detailChildren = [];
    if (identity.projects.length) {
      detailChildren.push(htmlElement("h2", "", "Projects / repositories"));
      var projectList = htmlElement("ul");
      identity.projects.forEach(function (project) {
        var item = htmlElement("li");
        item.appendChild(projectNode(project));
        var source = identitySourceLabel(project.source);
        if (source) {
          item.appendChild(htmlElement("span", "site-identity-source", "(" + source + ")"));
        }
        projectList.appendChild(item);
      });
      detailChildren.push(projectList);
    }
    if (identity.hosts.length) {
      detailChildren.push(htmlElement("h2", "", "Execution hosts"));
      var hostList = htmlElement("ul");
      identity.hosts.forEach(function (host) {
        var item = htmlElement("li", "", host.hostname);
        var source = identitySourceLabel(host.source);
        if (source) {
          item.appendChild(htmlElement("span", "site-identity-source", "(" + source + ")"));
        }
        hostList.appendChild(item);
      });
      detailChildren.push(hostList);
    }
    dom.identityList.replaceChildren.apply(dom.identityList, detailChildren);
    dom.identityDetails.hidden = detailChildren.length === 0;
  }

  function initializeData(raw) {
    var data = normalizeData(raw);
    app.data = data;
    installTimezone(data.display_timezone);
    app.navigationRange = timelineCore
      ? timelineCore.navigableRange(data.range, data.rollups)
      : data.range;
    app.viewStart = data.range.start_ms;
    app.viewEnd = data.range.end_ms;
    app.agentsById.clear();
    app.teamBySlug.clear();
    app.phasesByAgent.clear();
    app.glossaryById.clear();

    data.teams.forEach(function (team) {
      if (team && typeof team === "object") {
        app.teamBySlug.set(text(team.slug), team);
      }
    });
    data.agents.forEach(function (agent) {
      if (!agent || typeof agent !== "object") {
        return;
      }
      var id = text(agent.id);
      if (id) {
        app.agentsById.set(id, agent);
      }
    });
    data.phases.forEach(function (phase) {
      if (!phase || typeof phase !== "object") {
        return;
      }
      var id = text(phase.agent_id);
      if (!app.phasesByAgent.has(id)) {
        app.phasesByAgent.set(id, []);
      }
      app.phasesByAgent.get(id).push(phase);
    });
    app.phasesByAgent.forEach(function (phases) {
      phases.sort(function (left, right) {
        return number(left.start_ms, 0) - number(right.start_ms, 0);
      });
    });
    var glossaryTerms = new Set();
    data.glossary.forEach(function (entry) {
      if (!entry || typeof entry !== "object") {
        throw new Error("glossary entries must be objects");
      }
      var id = glossaryId(entry.id);
      var term = text(entry.term);
      if (!id || !term || text(entry.url) !== "#glossary/" + id) {
        throw new Error("glossary entry has an invalid stable target");
      }
      if (app.glossaryById.has(id) || glossaryTerms.has(term)) {
        throw new Error("glossary contains a duplicate ID or exact term");
      }
      app.glossaryById.set(id, entry);
      glossaryTerms.add(term);
    });
    dom.glossaryOpen.hidden = !data.glossary.length && !data.glossary_path;

    renderSiteIdentity(data);
    populateTeamFilter();
    populateSummaryFiles();
    var generated = data.generated_at ? "generated " + data.generated_at : "generation time unknown";
    var timezone = app.timezone;
    var timezoneSource = text(data.display_timezone_source).replace(/_/g, " ");
    dom.meta.textContent =
      dataVersionLabel(data.schema_version) + " · " + generated + " · display " + timezone +
      (timezoneSource ? " (" + timezoneSource + ")" : "") +
      (app.timezoneStatus === "archive" ? "" : " · " + app.timezoneStatus);
    scheduleRender();
    openGlossaryFromHash();
  }

  function populateTeamFilter() {
    var current = app.selectedTeam;
    var all = htmlElement("option", "", "All teams");
    all.value = "";
    var options = [all];
    app.data.teams.forEach(function (team) {
      var slug = text(team.slug);
      if (!slug) {
        return;
      }
      var label = text(team.label, slug);
      var option = htmlElement("option", "", label);
      option.value = slug;
      options.push(option);
    });
    dom.teamFilter.replaceChildren.apply(dom.teamFilter, options);
    dom.teamFilter.value = current;
  }

  function populateSummaryFiles() {
    var files = app.data.summary_files;
    var children = [];
    if (!files.length) {
      children.push(htmlElement("p", "summary-files-empty", "No summary files in this dataset."));
    } else {
      files.forEach(function (file) {
        var button = htmlElement("button", "summary-file-button");
        button.type = "button";
        var kind = htmlElement("span", "summary-file-kind", text(file.kind, "summary"));
        var label = htmlElement(
          "span",
          "summary-file-label",
          text(file.label, text(file.period, text(file.path, "Summary")))
        );
        button.append(kind, label);
        button.addEventListener("click", function () {
          dom.summaryMenu.open = false;
          openMarkdownModal({
            eyebrow: text(file.kind, "Summary") + (file.period ? " · " + file.period : ""),
            title: text(file.label, text(file.period, "Summary")),
            path: text(file.path)
          });
        });
        children.push(button);
      });
    }
    dom.summaryFiles.replaceChildren.apply(dom.summaryFiles, children);
  }

  function lowerSearchText(values) {
    return values
      .map(function (value) { return text(value); })
      .join(" ")
      .toLocaleLowerCase();
  }

  function agentOfficialLeaf(agent) {
    var explicit = text(agent.official_leaf);
    if (explicit) {
      return explicit;
    }
    var official = text(agent.official_name, text(agent.path));
    var parts = official.split("/").filter(Boolean);
    return parts.length ? parts[parts.length - 1] : official;
  }

  function agentShortName(agent) {
    return text(
      agent.short_name,
      text(
        agent.nickname,
        text(agent.label, text(agentOfficialLeaf(agent), text(agent.id, "Unknown agent")))
      )
    );
  }

  function agentOfficialName(agent) {
    return text(
      agent.official_name,
      text(agent.path, text(agent.official_leaf, text(agent.nickname, text(agent.id))))
    );
  }

  function namesEqual(left, right) {
    return Boolean(left) && Boolean(right) &&
      left.toLocaleLowerCase() === right.toLocaleLowerCase();
  }

  function agentSecondaryName(agent) {
    var shortName = agentShortName(agent);
    var officialName = agentOfficialName(agent);
    var nickname = text(agent.nickname);
    var parts = [];
    if (officialName && !namesEqual(officialName, shortName)) {
      parts.push(officialName);
    }
    if (nickname && !namesEqual(nickname, officialName)) {
      parts.push("coordinator: " + nickname);
    }
    return parts.join(" · ");
  }

  function agentAccessibleName(agent) {
    var parts = ["Short name: " + agentShortName(agent)];
    var officialName = agentOfficialName(agent);
    var nickname = text(agent.nickname);
    if (officialName) {
      parts.push("Official name: " + officialName);
    }
    if (nickname) {
      parts.push("Coordinator nickname: " + nickname);
    }
    if (text(agent.lifetime_summary)) {
      parts.push("Lifetime summary: " + text(agent.lifetime_summary));
    }
    return parts.join(". ");
  }

  function agentLifetimeSummary(agent) {
    return text(
      agent.lifetime_summary,
      "No lifetime summary is available; regenerate this archive's summaries."
    );
  }

  function frontEllipsize(value, maximumCharacters) {
    var content = text(value);
    var maximum = Math.max(2, Math.floor(number(maximumCharacters, 2)));
    if (content.length <= maximum) {
      return content;
    }
    return "…" + content.slice(-(maximum - 1));
  }

  function truncateLabel(value, width, preserveTail) {
    var content = text(value);
    var characterWidth = preserveTail ? 5.25 : 6.1;
    var maximum = Math.max(0, Math.floor(width / characterWidth));
    if (content.length <= maximum) {
      return content;
    }
    if (maximum < 5) {
      return "";
    }
    if (preserveTail) {
      return "…" + content.slice(-(maximum - 1));
    }
    return content.slice(0, maximum - 1).trimEnd() + "…";
  }

  function fitAgentSecondaryName(agent, width) {
    var full = agentSecondaryName(agent);
    if (!full || full.length * 5.25 <= width) {
      return full;
    }
    var officialName = agentOfficialName(agent);
    if (officialName && !namesEqual(officialName, agentShortName(agent))) {
      return truncateLabel(officialName, width, true);
    }
    return truncateLabel(full, width, true);
  }

  function agentTooltipIdentity(agent, includeLifetimeSummary) {
    var lines = [];
    var officialName = agentOfficialName(agent);
    var nickname = text(agent.nickname);
    if (officialName) {
      lines.push("Official: " + frontEllipsize(officialName, 64));
    }
    if (nickname && !namesEqual(nickname, officialName)) {
      lines.push("Coordinator nickname: " + nickname);
    }
    if (includeLifetimeSummary) {
      lines.push("", agentLifetimeSummary(agent));
    }
    return lines.join("\n");
  }

  function agentSearchText(agent) {
    return lowerSearchText([
      agent.id,
      agent.team,
      agent.path,
      agent.short_name,
      agent.official_name,
      agent.official_leaf,
      agent.label,
      agent.nickname,
      agent.lifetime_summary,
      agent.status
    ]);
  }

  function phaseSearchText(phase) {
    return lowerSearchText([phase.id, phase.phrase, phase.paragraph]);
  }

  function edgeSearchText(edge) {
    var source = app.agentsById.get(text(edge.source_id));
    var target = app.agentsById.get(text(edge.target_id));
    return lowerSearchText([
      edge.id,
      edge.kind,
      edge.phrase,
      edge.paragraph,
      edge.full_text,
      source ? agentSearchText(source) : "",
      target ? agentSearchText(target) : ""
    ]);
  }

  function compareAgents(left, right) {
    var leftPath = agentOfficialName(left);
    var rightPath = agentOfficialName(right);
    if (leftPath && rightPath && leftPath !== rightPath) {
      return leftPath.localeCompare(rightPath, undefined, { numeric: true });
    }
    var startDifference = number(left.start_ms, 0) - number(right.start_ms, 0);
    if (startDifference) {
      return startDifference;
    }
    return agentShortName(left).localeCompare(agentShortName(right));
  }

  function buildRows() {
    var query = app.query.trim().toLocaleLowerCase();
    var eligible = app.data.agents.filter(function (agent) {
      return !app.selectedTeam || text(agent.team) === app.selectedTeam;
    });
    var eligibleById = new Map();
    eligible.forEach(function (agent) {
      eligibleById.set(text(agent.id), agent);
    });

    var directMatches = new Set();
    if (query) {
      eligible.forEach(function (agent) {
        var id = text(agent.id);
        var phases = app.phasesByAgent.get(id) || [];
        var agentMatches = agentSearchText(agent).indexOf(query) >= 0;
        var phaseMatches = phases.some(function (phase) {
          return phaseSearchText(phase).indexOf(query) >= 0;
        });
        if (agentMatches || phaseMatches) {
          directMatches.add(id);
        }
      });
      app.data.edges.forEach(function (edge) {
        if (edgeSearchText(edge).indexOf(query) >= 0) {
          if (eligibleById.has(text(edge.source_id))) {
            directMatches.add(text(edge.source_id));
          }
          if (eligibleById.has(text(edge.target_id))) {
            directMatches.add(text(edge.target_id));
          }
        }
      });
    } else {
      eligible.forEach(function (agent) {
        directMatches.add(text(agent.id));
      });
    }

    var included = new Set(directMatches);
    directMatches.forEach(function (id) {
      var current = eligibleById.get(id);
      var guard = 0;
      while (current && current.parent_id && guard < eligible.length) {
        var parentId = text(current.parent_id);
        if (!eligibleById.has(parentId)) {
          break;
        }
        included.add(parentId);
        current = eligibleById.get(parentId);
        guard += 1;
      }
    });

    var children = new Map();
    var roots = [];
    eligible.forEach(function (agent) {
      var id = text(agent.id);
      if (!included.has(id)) {
        return;
      }
      var parentId = text(agent.parent_id);
      if (parentId && included.has(parentId)) {
        if (!children.has(parentId)) {
          children.set(parentId, []);
        }
        children.get(parentId).push(agent);
      } else {
        roots.push(agent);
      }
    });
    roots.sort(compareAgents);
    children.forEach(function (items) {
      items.sort(compareAgents);
    });

    var rows = [];
    var visited = new Set();
    function visit(agent, treeDepth) {
      var id = text(agent.id);
      if (!id || visited.has(id)) {
        return;
      }
      visited.add(id);
      rows.push({
        agent: agent,
        treeDepth: treeDepth,
        directMatch: directMatches.has(id),
        agentTextMatch: !query || agentSearchText(agent).indexOf(query) >= 0
      });
      (children.get(id) || []).forEach(function (child) {
        visit(child, treeDepth + 1);
      });
    }
    roots.forEach(function (root) {
      visit(root, 0);
    });
    eligible
      .filter(function (agent) {
        return included.has(text(agent.id)) && !visited.has(text(agent.id));
      })
      .sort(compareAgents)
      .forEach(function (agent) {
        visit(agent, Math.max(0, number(agent.depth, 0)));
      });

    app.rows = rows;
    app.rowByAgent.clear();
    var packed = null;
    if (!app.perAgentTracks && timelineCore) {
      packed = timelineCore.packLifetimes(rows.map(function (row, inputIndex) {
        var agent = row.agent;
        return {
          id: text(agent.id),
          start_ms: number(agent.start_ms, app.data.range.start_ms),
          end_ms: number(agent.end_ms, app.data.range.end_ms),
          official_name: agentOfficialName(agent),
          input_index: inputIndex,
          dedicated: !text(agent.parent_id)
        };
      }));
    }
    rows.forEach(function (row, index) {
      var agentId = text(row.agent.id);
      row.index = packed ? number(packed.lane_by_id[agentId], index) : index;
      app.rowByAgent.set(text(row.agent.id), row);
    });
    app.laneCount = packed ? number(packed.lane_count, rows.length) : rows.length;
  }

  function measure() {
    var measured = Math.round(dom.axis.getBoundingClientRect().width || dom.card.clientWidth || 1000);
    app.width = Math.max(280, measured);
    var cssWidth = parseFloat(
      getComputedStyle(dom.card).getPropertyValue("--label-width")
    );
    app.labelWidth = Number.isFinite(cssWidth) ? cssWidth : 238;
    app.chartWidth = Math.max(1, app.width - app.labelWidth);
  }

  function configureTrackMode() {
    dom.card.classList.toggle("per-agent-mode", app.perAgentTracks);
    dom.card.classList.toggle("packed-mode", !app.perAgentTracks);
    if (app.perAgentTracks) {
      dom.card.style.removeProperty("--label-width");
    } else {
      dom.card.style.setProperty("--label-width", COMPACT_LABEL_WIDTH + "px");
    }
    dom.svg.setAttribute("data-track-mode", app.perAgentTracks ? "per-agent" : "packed");
  }

  function phaseTop() {
    return app.perAgentTracks ? PHASE_TOP : COMPACT_PHASE_TOP;
  }

  function phaseHeight() {
    return app.perAgentTracks ? PHASE_HEIGHT : COMPACT_PHASE_HEIGHT;
  }

  function timeToX(milliseconds) {
    var span = Math.max(1, app.viewEnd - app.viewStart);
    return app.labelWidth + ((milliseconds - app.viewStart) / span) * app.chartWidth;
  }

  function niceTickInterval(span, width) {
    var target = span / Math.max(2, width / 115);
    var intervals = [
      1000, 5000, 15000, 30000,
      60000, 5 * 60000, 15 * 60000, 30 * 60000,
      3600000, 3 * 3600000, 6 * 3600000, 12 * 3600000,
      24 * 3600000, 2 * 24 * 3600000, 7 * 24 * 3600000,
      14 * 24 * 3600000, 30 * 24 * 3600000, 90 * 24 * 3600000,
      180 * 24 * 3600000, 365 * 24 * 3600000
    ];
    for (var index = 0; index < intervals.length; index += 1) {
      if (intervals[index] >= target) {
        return intervals[index];
      }
    }
    return intervals[intervals.length - 1];
  }

  function computeTicks() {
    var span = app.viewEnd - app.viewStart;
    var interval = niceTickInterval(span, app.chartWidth);
    var first = Math.ceil(app.viewStart / interval) * interval;
    var ticks = [];
    for (var value = first; value <= app.viewEnd && ticks.length < 250; value += interval) {
      ticks.push(value);
    }
    app.axisTicks = ticks;
  }

  function renderAxis() {
    computeTicks();
    dom.axis.setAttribute("viewBox", "0 0 " + app.width + " 34");
    dom.axis.setAttribute("width", String(app.width));
    dom.axis.setAttribute("height", "34");
    var children = [];
    children.push(svgElement("rect", {
      x: 0, y: 0, width: app.labelWidth, height: 34, fill: "#111823"
    }));
    children.push(svgElement("line", {
      x1: app.labelWidth, y1: 0, x2: app.labelWidth, y2: 34, class: "label-divider"
    }));
    children.push(svgElement("text", {
      x: 14, y: 21, class: "axis-title"
    }, app.perAgentTracks ? "AGENT TRACKS" : "PACKED LANES"));
    var span = app.viewEnd - app.viewStart;
    app.axisTicks.forEach(function (tick) {
      var x = timeToX(tick);
      children.push(svgElement("line", {
        x1: x, y1: 21, x2: x, y2: 34, class: "axis-line"
      }));
      children.push(svgElement("text", {
        x: x + 4, y: 14, class: "axis-label"
      }, formatTick(tick, span)));
    });
    dom.axis.replaceChildren.apply(dom.axis, children);
    dom.viewRange.textContent =
      formatRange(app.viewStart, app.viewEnd) +
      " · " + app.timezone +
      " · " + formatDuration(span);
  }

  function formatDuration(milliseconds) {
    var seconds = Math.max(0, milliseconds / 1000);
    if (seconds < 90) {
      return Math.round(seconds) + " sec";
    }
    var minutes = seconds / 60;
    if (minutes < 90) {
      return Math.round(minutes) + " min";
    }
    var hours = minutes / 60;
    if (hours < 48) {
      return hours.toFixed(hours < 10 ? 1 : 0) + " hr";
    }
    var days = hours / 24;
    if (days < 120) {
      return days.toFixed(days < 10 ? 1 : 0) + " days";
    }
    return (days / 365.25).toFixed(1) + " years";
  }

  function phaseColor(agentId) {
    var hash = 0;
    for (var index = 0; index < agentId.length; index += 1) {
      hash = ((hash << 5) - hash + agentId.charCodeAt(index)) | 0;
    }
    return PHASE_COLORS[Math.abs(hash) % PHASE_COLORS.length];
  }

  function addDefs(svg) {
    var defs = svgElement("defs");
    var clip = svgElement("clipPath", { id: "chart-clip" });
    clip.appendChild(svgElement("rect", {
      x: app.labelWidth,
      y: 0,
      width: app.chartWidth,
      height: Math.max(1, app.laneCount * ROW_HEIGHT)
    }));
    defs.appendChild(clip);
    Object.keys(EDGE_COLORS).forEach(function (kind) {
      var marker = svgElement("marker", {
        id: "arrow-" + kind,
        markerWidth: 7,
        markerHeight: 7,
        refX: 6,
        refY: 3.5,
        orient: "auto",
        markerUnits: "strokeWidth"
      });
      marker.appendChild(svgElement("path", {
        d: "M0,0 L7,3.5 L0,7 Z",
        fill: EDGE_COLORS[kind]
      }));
      defs.appendChild(marker);
    });
    svg.appendChild(defs);
  }

  function edgeKind(value) {
    return normalizeKind(value, ["spawn", "message", "result"], "other");
  }

  function stateKind(value) {
    return normalizeKind(value, ["active", "tool", "waiting", "idle", "blocked"], "idle");
  }

  function visibleRowBounds() {
    var top = Math.max(0, dom.scroll.scrollTop - ROW_HEIGHT * 2);
    var bottom = dom.scroll.scrollTop + dom.scroll.clientHeight + ROW_HEIGHT * 2;
    return {
      first: Math.max(0, Math.floor(top / ROW_HEIGHT)),
      last: Math.min(app.laneCount - 1, Math.ceil(bottom / ROW_HEIGHT)),
      top: top,
      bottom: bottom
    };
  }

  function edgePath(x1, y1, x2, y2) {
    var direction = x2 >= x1 ? 1 : -1;
    var bend = Math.max(20, Math.abs(x2 - x1) * 0.42);
    var control1 = x1 + direction * bend;
    var control2 = x2 - direction * bend;
    if (Math.abs(x2 - x1) < 32 && Math.abs(y2 - y1) > 2) {
      var verticalDirection = y2 > y1 ? 1 : -1;
      var hookDirection = x1 + 34 < app.width ? 1 : -1;
      var hookX = x1 + hookDirection * 28;
      var approachY = y2 - verticalDirection * 9;
      return "M " + x1.toFixed(2) + " " + y1.toFixed(2) +
        " C " + hookX.toFixed(2) + " " + y1.toFixed(2) +
        ", " + hookX.toFixed(2) + " " + approachY.toFixed(2) +
        ", " + x2.toFixed(2) + " " + approachY.toFixed(2) +
        " L " + x2.toFixed(2) + " " + y2.toFixed(2);
    }
    if (Math.abs(x2 - x1) < 32) {
      control1 = x1 + direction * 28;
      control2 = x2 + direction * 28;
    }
    return "M " + x1.toFixed(2) + " " + y1.toFixed(2) +
      " C " + control1.toFixed(2) + " " + y1.toFixed(2) +
      ", " + control2.toFixed(2) + " " + y2.toFixed(2) +
      ", " + x2.toFixed(2) + " " + y2.toFixed(2);
  }

  function edgeAgent(edge, side) {
    return app.agentsById.get(text(edge[side + "_id"]));
  }

  function edgeRouteShort(edge) {
    var source = edgeAgent(edge, "source");
    var target = edgeAgent(edge, "target");
    var sourceName = source ? agentShortName(source) : text(edge.source_id, "Unknown source");
    var targetName = target ? agentShortName(target) : text(edge.target_id, "Unknown target");
    return sourceName + " → " + targetName;
  }

  function edgeRouteDetail(edge) {
    var source = edgeAgent(edge, "source");
    var target = edgeAgent(edge, "target");
    var lines = ["Route: " + edgeRouteShort(edge)];
    if (source) {
      lines.push("From — " + agentAccessibleName(source));
    }
    if (target) {
      lines.push("To — " + agentAccessibleName(target));
    }
    return lines.join("\n");
  }

  function selectedAgentId() {
    if (!app.selection) {
      return "";
    }
    if (app.selection.kind === "agent" || app.selection.kind === "phase") {
      return text(app.selection.agent_id);
    }
    return "";
  }

  function agentsAreImmediateFamily(leftId, rightId) {
    var left = app.agentsById.get(leftId);
    var right = app.agentsById.get(rightId);
    return Boolean(
      left && right &&
      (text(left.parent_id) === rightId || text(right.parent_id) === leftId)
    );
  }

  function selectionClass(agentId, phase) {
    var selection = app.selection;
    if (!selection) {
      return "";
    }
    if (selection.kind === "rollup") {
      if (!phase) {
        return "";
      }
      return rangesOverlap(
        number(phase.start_ms, 0),
        number(phase.end_ms, 0),
        number(selection.start_ms, 0),
        number(selection.end_ms, 0)
      ) ? " is-selected" : " is-dimmed";
    }
    if (selection.kind === "edge") {
      if (agentId === text(selection.source_id) || agentId === text(selection.target_id)) {
        return " is-related";
      }
      return " is-dimmed";
    }
    var selectedId = selectedAgentId();
    if (agentId === selectedId) {
      if (selection.kind === "phase") {
        if (!phase) {
          return " is-related";
        }
        return text(phase.id) === text(selection.phase_id) ? " is-selected" : " is-dimmed";
      }
      return " is-selected";
    }
    if (agentsAreImmediateFamily(agentId, selectedId)) {
      return " is-related";
    }
    return " is-dimmed";
  }

  function setSelection(selection) {
    app.selection = selection;
    hideContextMenu();
    hideLaneAgentMenu(false);
    scheduleRender();
  }

  function selectAgent(agent) {
    setSelection({ kind: "agent", agent_id: text(agent.id) });
  }

  function selectPhase(phase) {
    var agentId = text(phase.agent_id);
    var next = timelineCore
      ? timelineCore.nextPhaseSelection(
          app.selection,
          agentId,
          text(phase.id),
          number(phase.start_ms, 0),
          number(phase.end_ms, 0)
        )
      : { kind: "agent", agent_id: agentId };
    setSelection(next);
  }

  function hideContextMenu() {
    dom.contextMenu.hidden = true;
    dom.contextMenuActions.replaceChildren();
  }

  function hideLaneAgentMenu(restoreFocus) {
    var anchor = app.laneMenuAnchor;
    dom.laneMenu.hidden = true;
    dom.laneMenuActions.replaceChildren();
    app.laneMenuAnchor = null;
    if (anchor && anchor.isConnected) {
      anchor.setAttribute("aria-expanded", "false");
      if (restoreFocus) {
        anchor.focus();
      }
    }
  }

  function showLaneAgentMenu(event, laneIndex, anchor) {
    event.preventDefault();
    event.stopPropagation();
    hideTooltip();
    hideContextMenu();
    hideLaneAgentMenu(false);
    var laneAgents = app.rows
      .filter(function (row) { return row.index === laneIndex; })
      .map(function (row) { return row.agent; })
      .sort(function (left, right) {
        return number(left.start_ms, 0) - number(right.start_ms, 0) ||
          compareAgents(left, right);
      });
    if (!laneAgents.length) {
      return;
    }
    var laneName = laneIndex === 0 ? "Coordinator lane" : "Lane " + laneIndex;
    dom.laneMenuTitle.textContent = laneName + " · " + laneAgents.length +
      (laneAgents.length === 1 ? " agent" : " agents");
    var buttons = laneAgents.map(function (agent) {
      var button = htmlElement("button", "lane-agent-menu-action");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      button.setAttribute("aria-label", "Select " + agentAccessibleName(agent));
      button.title = agentAccessibleName(agent);
      button.append(
        htmlElement("span", "lane-agent-menu-name", agentShortName(agent)),
        htmlElement(
          "span",
          "lane-agent-menu-official",
          truncateLabel(agentOfficialName(agent), 270, true)
        )
      );
      button.addEventListener("click", function () {
        hideLaneAgentMenu(false);
        selectAgent(agent);
      });
      return button;
    });
    dom.laneMenuActions.replaceChildren.apply(dom.laneMenuActions, buttons);
    app.laneMenuAnchor = anchor;
    anchor.setAttribute("aria-expanded", "true");
    dom.laneMenu.hidden = false;
    var anchorBox = anchor.getBoundingClientRect();
    var menuWidth = dom.laneMenu.offsetWidth;
    var menuHeight = dom.laneMenu.offsetHeight;
    var left = clamp(anchorBox.left, 8, window.innerWidth - menuWidth - 8);
    var top = anchorBox.bottom + 4;
    if (top + menuHeight > window.innerHeight - 8) {
      top = Math.max(8, anchorBox.top - menuHeight - 4);
    }
    dom.laneMenu.style.left = left + "px";
    dom.laneMenu.style.top = top + "px";
    if (buttons[0]) {
      buttons[0].focus();
    }
  }

  function showContextMenu(event, title, actions) {
    event.preventDefault();
    event.stopPropagation();
    hideTooltip();
    hideLaneAgentMenu(false);
    dom.contextMenuTitle.textContent = title;
    var buttons = actions.map(function (action) {
      var button = htmlElement("button", "context-menu-action", action.label);
      button.type = "button";
      button.setAttribute("role", "menuitem");
      button.addEventListener("click", function () {
        hideContextMenu();
        action.run();
      });
      return button;
    });
    dom.contextMenuActions.replaceChildren.apply(dom.contextMenuActions, buttons);
    dom.contextMenu.hidden = false;
    var menuWidth = Math.max(180, dom.contextMenu.offsetWidth);
    var menuHeight = dom.contextMenu.offsetHeight;
    dom.contextMenu.style.left = clamp(event.clientX, 8, window.innerWidth - menuWidth - 8) + "px";
    dom.contextMenu.style.top = clamp(event.clientY, 8, window.innerHeight - menuHeight - 8) + "px";
    if (buttons[0]) {
      buttons[0].focus();
    }
  }

  function zoomToRange(start, end) {
    var safeStart = number(start, app.viewStart);
    var safeEnd = Math.max(safeStart + 1, number(end, app.viewEnd));
    setView(safeStart, safeEnd);
  }

  function phaseContextMenu(event, phase, agent) {
    showContextMenu(event, text(phase.phrase, "Work phase"), [
      {
        label: "Zoom to work phase",
        run: function () { zoomToRange(phase.start_ms, phase.end_ms); }
      },
      {
        label: "Zoom to agent lifetime",
        run: function () { zoomToRange(agent.start_ms, agent.end_ms); }
      }
    ]);
  }

  function agentContextMenu(event, agent) {
    showContextMenu(event, agentShortName(agent), [
      {
        label: "Zoom to agent lifetime",
        run: function () { zoomToRange(agent.start_ms, agent.end_ms); }
      }
    ]);
  }

  function renderEdge(edge, layer, bounds, bufferStart, bufferEnd) {
    var sourceRow = app.rowByAgent.get(text(edge.source_id));
    var targetRow = app.rowByAgent.get(text(edge.target_id));
    if (!sourceRow || !targetRow) {
      return;
    }
    var sourceTime = number(edge.source_ms, NaN);
    var targetTime = number(edge.target_ms, NaN);
    if (!Number.isFinite(sourceTime) || !Number.isFinite(targetTime)) {
      return;
    }
    if (!rangesOverlap(
      Math.min(sourceTime, targetTime),
      Math.max(sourceTime, targetTime),
      bufferStart,
      bufferEnd
    )) {
      return;
    }
    var y1 = sourceRow.index * ROW_HEIGHT + ROW_HEIGHT / 2;
    var y2 = targetRow.index * ROW_HEIGHT + ROW_HEIGHT / 2;
    if (Math.max(y1, y2) < bounds.top || Math.min(y1, y2) > bounds.bottom) {
      return;
    }
    var x1 = timeToX(sourceTime);
    var x2 = timeToX(targetTime);
    var kind = edgeKind(edge.kind);
    var displayState = timelineCore
      ? timelineCore.edgeDisplayState(
          edge,
          app.selection,
          app.showGlobalMessages,
          app.showHighlightedMessages
        )
      : "normal";
    if (displayState === "hidden") {
      return;
    }
    var pathData = edgePath(x1, y1, x2, y2);
    var group = svgElement("g", {
      class: "edge-group edge-state-" + displayState,
      tabindex: "0",
      role: "button",
      "data-edge-id": text(edge.id),
      "data-edge-state": displayState,
      "aria-label": text(edge.phrase, kind + " interaction") + ". " + edgeRouteDetail(edge)
    });
    var visible = svgElement("path", {
      d: pathData,
      class: "edge-visible edge-" + kind,
      "marker-end": "url(#arrow-" + kind + ")"
    });
    var hit = svgElement("path", { d: pathData, class: "edge-hit" });
    group.append(visible, hit);
    group.addEventListener("pointerenter", function (event) {
      showTooltip(
        event,
        text(edge.phrase, kind + " interaction") + " · " + edgeRouteShort(edge),
        text(edge.paragraph, text(edge.content_status, "No paragraph summary available.")) +
          "\n\n" + edgeRouteDetail(edge),
        text(edge.kind, kind) + " interaction"
      );
    });
    group.addEventListener("pointermove", positionTooltip);
    group.addEventListener("pointerleave", hideTooltip);
    group.addEventListener("click", function (event) {
      if (Date.now() < app.suppressClickUntil || event.detail !== 1) {
        return;
      }
      event.stopPropagation();
      setSelection({
        kind: "edge",
        edge_id: text(edge.id),
        source_id: text(edge.source_id),
        target_id: text(edge.target_id)
      });
    });
    group.addEventListener("dblclick", function (event) {
      event.preventDefault();
      event.stopPropagation();
      openEdgeModal(edge);
    });
    group.addEventListener("keydown", function (event) {
      if (event.key === "Enter") {
        event.preventDefault();
        openEdgeModal(edge);
      } else if (event.key === " ") {
        event.preventDefault();
        setSelection({
          kind: "edge",
          edge_id: text(edge.id),
          source_id: text(edge.source_id),
          target_id: text(edge.target_id)
        });
      }
    });
    layer.appendChild(group);
  }

  function truncatePhrase(phrase, width) {
    var maxCharacters = Math.max(0, Math.floor((width - 14) / 6.2));
    if (maxCharacters < 4) {
      return "";
    }
    if (phrase.length <= maxCharacters) {
      return phrase;
    }
    return phrase.slice(0, Math.max(1, maxCharacters - 1)).trimEnd() + "…";
  }

  function phaseMatchesQuery(phase, row) {
    if (!app.query) {
      return true;
    }
    return row.agentTextMatch ||
      phaseSearchText(phase).indexOf(app.query.toLocaleLowerCase()) >= 0;
  }

  function renderPhase(phase, row, layer, bufferStart, bufferEnd) {
    var start = number(phase.start_ms, NaN);
    var end = number(phase.end_ms, NaN);
    if (!Number.isFinite(start) || !Number.isFinite(end) || end < start) {
      return;
    }
    if (!rangesOverlap(start, end, app.viewStart, app.viewEnd) ||
        !rangesOverlap(start, end, bufferStart, bufferEnd) ||
        !phaseMatchesQuery(phase, row)) {
      return;
    }
    var clippedStart = Math.max(start, app.viewStart);
    var clippedEnd = Math.min(end, app.viewEnd);
    var x = timeToX(clippedStart);
    var width = Math.max(2, timeToX(clippedEnd) - x);
    var phaseBlockHeight = phaseHeight();
    var y = row.index * ROW_HEIGHT + phaseTop();
    var agentId = text(phase.agent_id);
    var agent = row.agent;
    var group = svgElement("g", {
      class: "phase-group" + selectionClass(agentId, phase),
      tabindex: "0",
      role: "button",
      "data-phase-id": text(phase.id),
      "data-agent-id": agentId,
      "data-start-ms": String(start),
      "data-end-ms": String(end),
      "aria-pressed": app.selection && app.selection.kind === "phase" &&
        text(app.selection.phase_id) === text(phase.id) ? "true" : "false",
      "aria-label": text(phase.phrase, "Agent phase") + ". " + agentAccessibleName(agent)
    });
    group.appendChild(svgElement("rect", {
      x: x,
      y: y,
      width: width,
      height: phaseBlockHeight,
      rx: 4,
      class: "phase-block",
      fill: phaseColor(agentId)
    }));

    array(phase.states).forEach(function (state) {
      var stateStart = number(state.start_ms, start);
      var stateEnd = number(state.end_ms, end);
      if (!rangesOverlap(stateStart, stateEnd, clippedStart, clippedEnd)) {
        return;
      }
      var stripStart = Math.max(stateStart, clippedStart);
      var stripEnd = Math.min(stateEnd, clippedEnd);
      var stripX = timeToX(stripStart);
      var stripWidth = Math.max(1, timeToX(stripEnd) - stripX);
      var kind = stateKind(state.kind);
      group.appendChild(svgElement("rect", {
        x: stripX,
        y: y + phaseBlockHeight - STATE_HEIGHT,
        width: stripWidth,
        height: STATE_HEIGHT,
        class: "state-strip",
        fill: STATE_COLORS[kind]
      }));
    });

    var phrase = truncatePhrase(text(phase.phrase, "Unlabelled phase"), width);
    if (phrase) {
      group.appendChild(svgElement("text", {
        x: x + 7,
        y: y + 19,
        class: "phase-label"
      }, phrase));
    }
    group.addEventListener("pointerenter", function (event) {
      showTooltip(
        event,
        text(phase.phrase, "Agent phase") + " · " + agentShortName(agent),
        text(phase.paragraph, "No paragraph summary available.") +
          "\n\n" + agentTooltipIdentity(agent, false),
        formatStatsInline(phase.stats)
      );
    });
    group.addEventListener("pointermove", positionTooltip);
    group.addEventListener("pointerleave", hideTooltip);
    group.addEventListener("click", function (event) {
      if (Date.now() < app.suppressClickUntil || event.detail !== 1) {
        return;
      }
      event.stopPropagation();
      selectPhase(phase);
    });
    group.addEventListener("dblclick", function (event) {
      event.preventDefault();
      event.stopPropagation();
      openPhaseModal(phase, row.agent);
    });
    group.addEventListener("contextmenu", function (event) {
      phaseContextMenu(event, phase, row.agent);
    });
    group.addEventListener("keydown", function (event) {
      if (event.key === "Enter") {
        event.preventDefault();
        openPhaseModal(phase, row.agent);
      } else if (event.key === " ") {
        event.preventDefault();
        selectPhase(phase);
      }
    });
    layer.appendChild(group);
  }

  function renderTrackLabel(row, layer) {
    var agent = row.agent;
    var y = row.index * ROW_HEIGHT;
    var depth = Math.max(0, row.treeDepth);
    var maximumIndent = Math.max(15, app.labelWidth - 72);
    var indent = Math.min(15 + depth * 17, maximumIndent);
    var color = phaseColor(text(agent.id));
    var shortName = agentShortName(agent);
    var textX = indent + 13;
    var textWidth = Math.max(0, app.labelWidth - textX - 8);
    var secondaryName = fitAgentSecondaryName(agent, textWidth);
    var labelGroup = svgElement("g", {
      class: "agent-label-group" + selectionClass(text(agent.id), null),
      role: "button",
      tabindex: "0",
      "data-agent-id": text(agent.id),
      "aria-label": agentAccessibleName(agent) +
        ". Hierarchy depth: " + depth + ". Status: " + text(agent.status, "unknown")
    });
    labelGroup.appendChild(svgElement("title", null, agentAccessibleName(agent)));
    labelGroup.appendChild(svgElement("rect", {
      x: 0,
      y: y,
      width: app.labelWidth,
      height: ROW_HEIGHT,
      class: "agent-label-hit"
    }));

    if (depth > 0) {
      var branchX = indent - 10;
      labelGroup.appendChild(svgElement("path", {
        d: "M " + branchX + " " + y +
          " L " + branchX + " " + (y + ROW_HEIGHT / 2) +
          " L " + (indent - 2) + " " + (y + ROW_HEIGHT / 2),
        class: "hierarchy-line"
      }));
    }
    labelGroup.appendChild(svgElement("circle", {
      cx: indent + 2,
      cy: y + 20,
      r: depth === 0 ? 5 : 4,
      class: "agent-dot",
      fill: color
    }));
    labelGroup.appendChild(svgElement("text", {
      x: textX,
      y: y + 17,
      class: "track-label" + (depth === 0 ? " track-label-coordinator" : "")
    }, truncateLabel(shortName, textWidth, false)));

    if (secondaryName) {
      labelGroup.appendChild(svgElement("text", {
        x: textX,
        y: y + 31,
        class: "track-official"
      }, secondaryName));
    }

    var status = text(agent.status, "unknown");
    var team = text(agent.team);
    var meta = (depth === 0 ? "COORDINATOR" : "L" + depth + " SUBAGENT") + " · " + status;
    if (team) {
      var teamLabel = app.teamBySlug.has(team)
        ? text(app.teamBySlug.get(team).label, team)
        : team;
      meta += " · " + teamLabel;
    }
    labelGroup.appendChild(svgElement("text", {
      x: textX,
      y: y + 45,
      class: "track-meta"
    }, truncateLabel(meta, textWidth, false)));
    labelGroup.addEventListener("pointerenter", function (event) {
      showTooltip(
        event,
        shortName,
        agentTooltipIdentity(agent, true),
        (depth === 0 ? "Coordinator" : "Hierarchy depth " + depth) + " · " + status
      );
    });
    labelGroup.addEventListener("pointermove", positionTooltip);
    labelGroup.addEventListener("pointerleave", hideTooltip);
    labelGroup.addEventListener("click", function (event) {
      if (event.detail !== 1 || Date.now() < app.suppressClickUntil) {
        return;
      }
      event.stopPropagation();
      selectAgent(agent);
    });
    labelGroup.addEventListener("contextmenu", function (event) {
      agentContextMenu(event, agent);
    });
    labelGroup.addEventListener("keydown", function (event) {
      if (event.key === " ") {
        event.preventDefault();
        selectAgent(agent);
      }
    });
    layer.appendChild(labelGroup);
  }

  function renderLaneLabel(index, layer) {
    var y = index * ROW_HEIGHT;
    var count = app.rows.filter(function (row) { return row.index === index; }).length;
    var displayName = index === 0 ? "COORD" : "LANE " + index;
    var accessibleName = index === 0 ? "Coordinator lane" : "Lane " + index;
    var group = svgElement("g", {
      class: "lane-label-group",
      role: "button",
      tabindex: "0",
      "data-lane-index": String(index),
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      "aria-label": accessibleName + ", " + count +
        (count === 1 ? " named agent" : " named agents")
    });
    group.appendChild(svgElement("rect", {
      x: 0,
      y: y,
      width: app.labelWidth,
      height: ROW_HEIGHT,
      class: "lane-label-hit"
    }));
    group.appendChild(svgElement("text", {
      x: 12,
      y: y + 28,
      class: "lane-label"
    }, displayName));
    group.addEventListener("click", function (event) {
      if (event.detail !== 1 || Date.now() < app.suppressClickUntil) {
        return;
      }
      showLaneAgentMenu(event, index, group);
    });
    group.addEventListener("keydown", function (event) {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        showLaneAgentMenu(event, index, group);
      }
    });
    layer.appendChild(group);
  }

  function renderAgentLifetime(row, layer) {
    var agent = row.agent;
    var start = number(agent.start_ms, app.data.range.start_ms);
    var end = number(agent.end_ms, app.data.range.end_ms);
    if (!rangesOverlap(start, end, app.viewStart, app.viewEnd)) {
      return;
    }
    var clippedStart = Math.max(start, app.viewStart);
    var clippedEnd = Math.min(end, app.viewEnd);
    var x = timeToX(clippedStart);
    var width = Math.max(2, timeToX(clippedEnd) - x);
    var y = row.index * ROW_HEIGHT;
    var group = svgElement("g", {
      class: "agent-lifetime-group" + selectionClass(text(agent.id), null),
      role: "button",
      tabindex: "0",
      "data-agent-id": text(agent.id),
      "data-start-ms": String(start),
      "data-end-ms": String(end),
      "aria-label": "Agent lifetime: " + agentAccessibleName(agent)
    });
    group.appendChild(svgElement("rect", {
      x: x,
      y: y + 2,
      width: width,
      height: ROW_HEIGHT - 5,
      rx: 3,
      class: "agent-lifetime-hit"
    }));
    group.appendChild(svgElement("rect", {
      x: x,
      y: y + ROW_HEIGHT / 2 - 2,
      width: width,
      height: 4,
      rx: 2,
      class: "lifetime-line"
    }));
    if (!app.perAgentTracks && width >= 22) {
      group.appendChild(svgElement("text", {
        x: x + 4,
        y: y + 12,
        class: "agent-inline-name"
      }, truncatePhrase(agentShortName(agent), width)));
    }
    group.addEventListener("pointerenter", function (event) {
      showTooltip(
        event,
        agentShortName(agent),
        agentTooltipIdentity(agent, true),
        "Agent lifetime · " + formatRange(start, end)
      );
    });
    group.addEventListener("pointermove", positionTooltip);
    group.addEventListener("pointerleave", hideTooltip);
    group.addEventListener("click", function (event) {
      if (event.detail !== 1 || Date.now() < app.suppressClickUntil) {
        return;
      }
      event.stopPropagation();
      selectAgent(agent);
    });
    group.addEventListener("dblclick", function (event) {
      event.preventDefault();
      event.stopPropagation();
      openAgentLifetimeModal(agent);
    });
    group.addEventListener("contextmenu", function (event) {
      agentContextMenu(event, agent);
    });
    group.addEventListener("keydown", function (event) {
      if (event.key === "Enter") {
        event.preventDefault();
        openAgentLifetimeModal(agent);
      } else if (event.key === " ") {
        event.preventDefault();
        selectAgent(agent);
      }
    });
    layer.appendChild(group);
  }

  function renderTracks() {
    var totalHeight = Math.max(dom.scroll.clientHeight, app.laneCount * ROW_HEIGHT, 1);
    dom.svg.setAttribute("viewBox", "0 0 " + app.width + " " + totalHeight);
    dom.svg.setAttribute("width", String(app.width));
    dom.svg.setAttribute("height", String(totalHeight));
    dom.svg.setAttribute("data-lane-count", String(app.laneCount));
    dom.svg.replaceChildren();
    addDefs(dom.svg);
    dom.empty.hidden = app.rows.length > 0;

    if (!app.rows.length) {
      return;
    }

    var bounds = visibleRowBounds();
    var backgroundLayer = svgElement("g");
    var gridLayer = svgElement("g", { "clip-path": "url(#chart-clip)" });
    var contentLayer = svgElement("g", { "clip-path": "url(#chart-clip)" });
    var edgeLayer = svgElement("g");
    var lifetimeLayer = svgElement("g");
    var phaseLayer = svgElement("g");
    var labelLayer = svgElement("g");
    contentLayer.append(edgeLayer, lifetimeLayer, phaseLayer);

    for (var index = bounds.first; index <= bounds.last; index += 1) {
      var y = index * ROW_HEIGHT;
      backgroundLayer.appendChild(svgElement("rect", {
        x: 0,
        y: y,
        width: app.width,
        height: ROW_HEIGHT,
        class: "track-row " + (index % 2 ? "track-row-odd" : "track-row-even")
      }));
      backgroundLayer.appendChild(svgElement("line", {
        x1: 0,
        y1: y + ROW_HEIGHT,
        x2: app.width,
        y2: y + ROW_HEIGHT,
        class: "track-divider"
      }));
    }
    app.axisTicks.forEach(function (tick) {
      var x = timeToX(tick);
      gridLayer.appendChild(svgElement("line", {
        x1: x,
        y1: bounds.top,
        x2: x,
        y2: bounds.bottom,
        class: "grid-line"
      }));
    });

    var span = app.viewEnd - app.viewStart;
    var bufferStart = app.viewStart - span * 0.08;
    var bufferEnd = app.viewEnd + span * 0.08;
    app.data.edges.forEach(function (edge) {
      if (!app.query || edgeSearchText(edge).indexOf(app.query.toLocaleLowerCase()) >= 0 ||
          app.rowByAgent.has(text(edge.source_id)) || app.rowByAgent.has(text(edge.target_id))) {
        renderEdge(edge, edgeLayer, bounds, bufferStart, bufferEnd);
      }
    });

    var visibleRows = app.rows.filter(function (row) {
      return row.index >= bounds.first && row.index <= bounds.last;
    });
    visibleRows.forEach(function (row) {
      var agent = row.agent;
      renderAgentLifetime(row, lifetimeLayer);
      (app.phasesByAgent.get(text(agent.id)) || []).forEach(function (phase) {
        renderPhase(phase, row, phaseLayer, bufferStart, bufferEnd);
      });
      if (app.perAgentTracks) {
        renderTrackLabel(row, labelLayer);
      }
    });
    if (!app.perAgentTracks) {
      for (var laneIndex = bounds.first; laneIndex <= bounds.last; laneIndex += 1) {
        renderLaneLabel(laneIndex, labelLayer);
      }
    }
    labelLayer.appendChild(svgElement("rect", {
      x: app.labelWidth - 1,
      y: bounds.top,
      width: 1,
      height: Math.max(1, bounds.bottom - bounds.top),
      fill: "#344057"
    }));
    dom.svg.append(backgroundLayer, gridLayer, contentLayer, labelLayer);
  }

  function renderRollups() {
    if (!app.data.rollups.length) {
      dom.rollupRow.hidden = true;
      dom.rollupTrack.replaceChildren();
      return;
    }
    dom.rollupRow.hidden = false;
    var span = app.viewEnd - app.viewStart;
    var children = [];
    app.data.rollups.forEach(function (rollup) {
      var start = number(rollup.start_ms, NaN);
      var end = number(rollup.end_ms, start);
      if (!Number.isFinite(start) || !Number.isFinite(end) ||
          !rangesOverlap(start, end, app.viewStart, app.viewEnd)) {
        return;
      }
      var left = ((Math.max(start, app.viewStart) - app.viewStart) / span) * app.chartWidth;
      var right = ((Math.min(end, app.viewEnd) - app.viewStart) / span) * app.chartWidth;
      var width = Math.max(4, right - left);
      var kind = normalizeKind(
        rollup.kind,
        ["daily", "weekly", "monthly", "quarterly"],
        "daily"
      );
      var selected = app.selection && app.selection.kind === "rollup" &&
        text(app.selection.path) === text(rollup.path);
      var button = htmlElement(
        "button",
        "rollup-marker rollup-" + kind + (selected ? " is-selected" : "")
      );
      button.type = "button";
      button.style.left = left.toFixed(2) + "px";
      button.style.width = width.toFixed(2) + "px";
      button.textContent = width >= 36 ? text(rollup.label, kind) : "";
      button.setAttribute(
        "aria-label",
        kind + " summary: " + text(rollup.label, formatRange(start, end))
      );
      button.title = text(rollup.label, kind + " summary");
      button.dataset.startMs = String(start);
      button.dataset.endMs = String(end);
      button.dataset.rollupKind = kind;
      button.setAttribute("aria-pressed", selected ? "true" : "false");
      button.addEventListener("click", function (event) {
        if (event.detail !== 1) {
          return;
        }
        setSelection({
          kind: "rollup",
          path: text(rollup.path),
          start_ms: start,
          end_ms: end
        });
      });
      button.addEventListener("dblclick", function (event) {
        event.preventDefault();
        openRollupModal(rollup, kind, start, end);
      });
      button.addEventListener("contextmenu", function (event) {
        var rangeName = {
          daily: "day",
          weekly: "week",
          monthly: "month",
          quarterly: "quarter"
        }[kind] || "summary range";
        showContextMenu(event, text(rollup.label, kind + " summary"), [
          {
            label: "Zoom to " + rangeName,
            run: function () {
              var activityRange = timelineCore.rollupActivityRange(
                rollup,
                app.data,
                MIN_VIEW_MS
              );
              zoomToRange(
                activityRange ? activityRange.start_ms : start,
                activityRange ? activityRange.end_ms : end
              );
            }
          }
        ]);
      });
      children.push(button);
    });
    dom.rollupTrack.replaceChildren.apply(dom.rollupTrack, children);
  }

  function eventBelongsToVisibleAgent(event, visibleIds) {
    var agentId = text(event.agent_id);
    if (!agentId) {
      return !app.selectedTeam && !app.query && visibleIds.size > 0;
    }
    return visibleIds.has(agentId);
  }

  function countVisibleStats() {
    var visibleIds = new Set(app.rows.map(function (row) {
      return text(row.agent.id);
    }));
    var result = {
      user_prompts: app.data.events.length ? 0 : null,
      agent_responses: app.data.events.length ? 0 : null,
      inter_agent_messages: app.data.events.length ? 0 : null,
      tool_calls: app.data.events.length ? 0 : null,
      active_agents: 0,
      event_counts_available: app.data.events.length > 0
    };
    app.rows.forEach(function (row) {
      var agent = row.agent;
      if (rangesOverlap(
        number(agent.start_ms, app.data.range.start_ms),
        number(agent.end_ms, app.data.range.end_ms),
        app.viewStart,
        app.viewEnd
      )) {
        result.active_agents += 1;
      }
    });

    var eventsInRange = app.data.events.filter(function (event) {
      var at = number(event.at_ms, NaN);
      return Number.isFinite(at) &&
        at >= app.viewStart &&
        at <= app.viewEnd &&
        eventBelongsToVisibleAgent(event, visibleIds);
    });
    if (app.data.events.length) {
      eventsInRange.forEach(function (event) {
        var kind = text(event.kind).toLowerCase().replace(/[- ]/g, "_");
        if (kind === "user_prompt" || kind === "user_prompts" || kind === "prompt") {
          result.user_prompts += 1;
        } else if (
          kind === "agent_response" ||
          kind === "agent_responses" ||
          kind === "assistant_message" ||
          kind === "coordinator_response"
        ) {
          result.agent_responses += 1;
        } else if (
          kind === "inter_agent_message" ||
          kind === "inter_agent_messages" ||
          kind === "agent_message"
        ) {
          result.inter_agent_messages += 1;
        } else if (kind === "tool_call" || kind === "tool_calls" || kind === "tool") {
          result.tool_calls += 1;
        }
      });
    }
    return result;
  }

  function statNode(value, label) {
    var node = htmlElement("div", "stat");
    node.append(
      htmlElement("strong", "stat-value", value === null ? "—" : formatCount(value)),
      htmlElement("span", "stat-label", label)
    );
    return node;
  }

  function renderStats() {
    var stats = countVisibleStats();
    dom.statsRange.textContent =
      formatRange(app.viewStart, app.viewEnd) +
      (stats.event_counts_available ? "" : " · event counts unavailable");
    dom.statsValues.replaceChildren(
      statNode(stats.user_prompts, "User prompts"),
      statNode(stats.agent_responses, "Agent responses"),
      statNode(stats.inter_agent_messages, "Inter-agent msgs"),
      statNode(stats.tool_calls, "Tool calls"),
      statNode(stats.active_agents, "Active agents")
    );
  }

  function render() {
    app.renderQueued = false;
    if (!app.data) {
      return;
    }
    configureTrackMode();
    measure();
    buildRows();
    dom.card.dataset.viewStartMs = String(app.viewStart);
    dom.card.dataset.viewEndMs = String(app.viewEnd);
    dom.card.dataset.trackMode = app.perAgentTracks ? "per-agent" : "packed";
    if (app.selection) {
      dom.card.dataset.selectionScope = text(app.selection.kind);
    } else {
      dom.card.removeAttribute("data-selection-scope");
    }
    if (app.selection && (app.selection.kind === "agent" || app.selection.kind === "phase")) {
      dom.card.dataset.selectedAgentId = text(app.selection.agent_id);
    } else {
      dom.card.removeAttribute("data-selected-agent-id");
    }
    if (app.selection && app.selection.kind === "phase") {
      dom.card.dataset.selectedPhaseId = text(app.selection.phase_id);
    } else {
      dom.card.removeAttribute("data-selected-phase-id");
    }
    renderAxis();
    renderRollups();
    renderTracks();
    renderStats();
  }

  function scheduleRender() {
    if (app.renderQueued) {
      return;
    }
    app.renderQueued = true;
    window.requestAnimationFrame(render);
  }

  function setView(start, end) {
    if (!app.data || !Number.isFinite(start) || !Number.isFinite(end)) {
      return;
    }
    var next = timelineCore.boundedViewRange(
      start,
      end,
      app.navigationRange,
      MIN_VIEW_MS
    );
    app.viewStart = next.start_ms;
    app.viewEnd = next.end_ms;
    scheduleRender();
  }

  function fitTimeline() {
    if (!app.data) {
      return;
    }
    setView(app.data.range.start_ms, app.data.range.end_ms);
  }

  function panTimelineByPixels(pixels) {
    var span = app.viewEnd - app.viewStart;
    var shift = (pixels / Math.max(1, app.chartWidth)) * span;
    setView(app.viewStart + shift, app.viewEnd + shift);
  }

  function wheelZoom(event) {
    if (!app.data) {
      return;
    }
    var horizontalDelta = event.shiftKey && Math.abs(event.deltaX) < 1
      ? event.deltaY
      : event.deltaX;
    var horizontalGesture = Math.abs(horizontalDelta) > 1 &&
      (event.shiftKey || Math.abs(horizontalDelta) >= Math.abs(event.deltaY) * 0.55);
    if (!event.ctrlKey && !event.metaKey && horizontalGesture) {
      event.preventDefault();
      hideTooltip();
      hideContextMenu();
      panTimelineByPixels(horizontalDelta);
      return;
    }
    var needsVerticalScroll =
      dom.scroll.scrollHeight > dom.scroll.clientHeight + 1;
    if (event.currentTarget === dom.svg &&
        needsVerticalScroll &&
        !event.ctrlKey &&
        !event.metaKey) {
      return;
    }
    event.preventDefault();
    hideTooltip();
    hideContextMenu();
    var rect = event.currentTarget.getBoundingClientRect();
    var ratio;
    if (event.currentTarget === dom.rollupTrack) {
      ratio = clamp((event.clientX - rect.left) / Math.max(1, rect.width), 0, 1);
    } else {
      var x = clamp(event.clientX - rect.left, app.labelWidth, app.width);
      ratio = clamp((x - app.labelWidth) / Math.max(1, app.chartWidth), 0, 1);
    }
    var oldSpan = app.viewEnd - app.viewStart;
    var scale = Math.exp(clamp(event.deltaY, -500, 500) * 0.0016);
    var fullSpan = app.navigationRange.end_ms - app.navigationRange.start_ms;
    var newSpan = clamp(oldSpan * scale, Math.min(MIN_VIEW_MS, fullSpan), fullSpan);
    var anchor = app.viewStart + ratio * oldSpan;
    var start = anchor - ratio * newSpan;
    setView(start, start + newSpan);
  }

  function beginPan(event) {
    if (!app.data || event.button !== 0) {
      return;
    }
    app.drag = {
      pointerId: event.pointerId,
      startX: event.clientX,
      lastX: event.clientX,
      viewStart: app.viewStart,
      viewEnd: app.viewEnd,
      moved: false,
      captured: false
    };
  }

  function continuePan(event) {
    if (!app.drag || event.pointerId !== app.drag.pointerId) {
      return;
    }
    var difference = event.clientX - app.drag.startX;
    app.drag.lastX = event.clientX;
    if (!app.drag.moved && Math.abs(difference) <= 3) {
      return;
    }
    if (!app.drag.moved) {
      app.drag.moved = true;
      hideTooltip();
      if (!app.drag.captured) {
        dom.svg.setPointerCapture(event.pointerId);
        dom.svg.classList.add("is-panning");
        app.drag.captured = true;
      }
    }
    var span = app.drag.viewEnd - app.drag.viewStart;
    var shift = -(difference / Math.max(1, app.chartWidth)) * span;
    setView(app.drag.viewStart + shift, app.drag.viewEnd + shift);
  }

  function endPan(event) {
    if (!app.drag || event.pointerId !== app.drag.pointerId) {
      return;
    }
    if (app.drag.moved) {
      app.suppressClickUntil = Date.now() + 180;
    }
    if (app.drag.captured && dom.svg.hasPointerCapture(event.pointerId)) {
      dom.svg.releasePointerCapture(event.pointerId);
    }
    app.drag = null;
    dom.svg.classList.remove("is-panning");
  }

  function clearSelectionFromEmptyTrack(event) {
    if (!app.selection || event.detail !== 1 || Date.now() < app.suppressClickUntil) {
      return;
    }
    var target = event.target;
    if (target instanceof Element && target.classList.contains("track-row")) {
      setSelection(null);
    }
  }

  function keyboardNavigate(event) {
    if (!app.data || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    var target = event.target;
    if (target instanceof HTMLInputElement ||
        target instanceof HTMLSelectElement ||
        target instanceof HTMLTextAreaElement) {
      return;
    }
    var span = app.viewEnd - app.viewStart;
    if (event.key === "+" || event.key === "=") {
      event.preventDefault();
      var zoomedSpan = span * 0.6;
      var midpoint = app.viewStart + span / 2;
      setView(midpoint - zoomedSpan / 2, midpoint + zoomedSpan / 2);
    } else if (event.key === "-" || event.key === "_") {
      event.preventDefault();
      var expandedSpan = span / 0.6;
      var center = app.viewStart + span / 2;
      setView(center - expandedSpan / 2, center + expandedSpan / 2);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      setView(app.viewStart - span * 0.12, app.viewEnd - span * 0.12);
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      setView(app.viewStart + span * 0.12, app.viewEnd + span * 0.12);
    } else if (event.key === "Home") {
      event.preventDefault();
      fitTimeline();
    }
  }

  function keyboardScrollTracks(event) {
    var amount = 0;
    if (event.key === "ArrowUp") {
      amount = -ROW_HEIGHT;
    } else if (event.key === "ArrowDown") {
      amount = ROW_HEIGHT;
    } else if (event.key === "PageUp") {
      amount = -Math.max(ROW_HEIGHT, dom.scroll.clientHeight - ROW_HEIGHT);
    } else if (event.key === "PageDown") {
      amount = Math.max(ROW_HEIGHT, dom.scroll.clientHeight - ROW_HEIGHT);
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    dom.scroll.scrollBy({ top: amount, behavior: "auto" });
  }

  function showTooltip(event, title, body, stats) {
    dom.tooltipTitle.textContent = title;
    dom.tooltipBody.textContent = body;
    dom.tooltipStats.textContent = stats;
    dom.tooltipStats.hidden = !stats;
    dom.tooltip.hidden = false;
    positionTooltip(event);
  }

  function positionTooltip(event) {
    if (dom.tooltip.hidden) {
      return;
    }
    var gap = 14;
    var left = event.clientX + gap;
    var top = event.clientY + gap;
    var width = dom.tooltip.offsetWidth;
    var height = dom.tooltip.offsetHeight;
    if (left + width > window.innerWidth - 10) {
      left = event.clientX - width - gap;
    }
    if (top + height > window.innerHeight - 10) {
      top = event.clientY - height - gap;
    }
    dom.tooltip.style.left = Math.max(8, left) + "px";
    dom.tooltip.style.top = Math.max(8, top) + "px";
  }

  function hideTooltip() {
    dom.tooltip.hidden = true;
  }

  function modalStats(stats) {
    var row = htmlElement("div", "modal-stats");
    var values = [
      ["user_prompts", "prompts"],
      ["agent_responses", "responses"],
      ["inter_agent_messages", "inter-agent messages"],
      ["tool_calls", "tool calls"]
    ];
    values.forEach(function (entry) {
      row.appendChild(
        htmlElement(
          "span",
          "modal-stat",
          formatCount(stats && stats[entry[0]]) + " " + entry[1]
        )
      );
    });
    return row;
  }

  function setModalSummary(paragraph, stats) {
    var children = [];
    if (paragraph) {
      children.push(markdownElement(paragraph, "modal-paragraph", true));
    }
    if (stats && typeof stats === "object") {
      children.push(modalStats(stats));
    }
    dom.modalSummary.replaceChildren.apply(dom.modalSummary, children);
    dom.modalSummary.hidden = children.length === 0;
  }

  function agentIdentityNode(agent, prefix) {
    var container = htmlElement("div", "modal-agent-identity");
    var heading = htmlElement("div", "modal-agent-heading");
    if (prefix) {
      heading.appendChild(htmlElement("span", "modal-agent-prefix", prefix));
    }
    heading.appendChild(htmlElement("strong", "modal-agent-short", agentShortName(agent)));
    container.appendChild(heading);

    var officialName = agentOfficialName(agent);
    if (officialName) {
      container.appendChild(
        htmlElement("div", "modal-agent-official", "Official: " + officialName)
      );
    }
    var nickname = text(agent.nickname);
    if (nickname && !namesEqual(nickname, officialName)) {
      container.appendChild(
        htmlElement("div", "modal-agent-nickname", "Coordinator nickname: " + nickname)
      );
    }
    container.setAttribute("aria-label", agentAccessibleName(agent));
    return container;
  }

  function showModalAgentIdentity(agent) {
    dom.modalSummary.prepend(agentIdentityNode(agent, "AGENT"));
    dom.modalSummary.hidden = false;
  }

  function showModalEdgeRoute(edge) {
    var route = htmlElement("div", "modal-edge-route");
    var source = edgeAgent(edge, "source");
    var target = edgeAgent(edge, "target");
    if (source) {
      route.appendChild(agentIdentityNode(source, "FROM"));
    }
    route.appendChild(htmlElement("span", "modal-route-arrow", "→"));
    if (target) {
      route.appendChild(agentIdentityNode(target, "TO"));
    }
    if (!source && !target) {
      route.replaceChildren(htmlElement("div", "modal-agent-official", edgeRouteShort(edge)));
    }
    route.setAttribute("aria-label", edgeRouteDetail(edge));
    dom.modalSummary.prepend(route);
    dom.modalSummary.hidden = false;
  }

  function openModalBase(eyebrow, title, paragraph, stats) {
    hideTooltip();
    var activeElement = document.activeElement;
    app.modalRestoreFocus =
      activeElement && typeof activeElement.focus === "function" ? activeElement : null;
    dom.modalEyebrow.textContent = eyebrow;
    dom.modalEyebrow.removeAttribute("title");
    dom.modalTitle.textContent = title;
    setModalSummary(paragraph, stats);
    dom.modalTabs.replaceChildren();
    dom.modalTabs.hidden = true;
    dom.modalContent.replaceChildren();
    dom.modalBackdrop.hidden = false;
    document.body.classList.add("modal-open");
    dom.modalClose.focus();
  }

  function closeModal() {
    if (dom.modalBackdrop.hidden) {
      return;
    }
    app.detailRequest += 1;
    dom.modalBackdrop.hidden = true;
    document.body.classList.remove("modal-open");
    if (app.modalRestoreFocus && document.contains(app.modalRestoreFocus)) {
      app.modalRestoreFocus.focus();
    }
    app.modalRestoreFocus = null;
  }

  function showLoading(container, message) {
    container.replaceChildren(
      htmlElement("div", "loading-message", message || "Loading details…")
    );
  }

  function showContentError(container, error) {
    var message = error instanceof Error ? error.message : String(error);
    container.replaceChildren(
      htmlElement("div", "error-message", "Could not load this view: " + message)
    );
  }

  function activateTabs(tabs, initialIndex) {
    dom.modalTabs.hidden = false;
    var buttons = [];
    var selected = -1;

    function activate(index) {
      if (index === selected || !tabs[index]) {
        return;
      }
      selected = index;
      buttons.forEach(function (button, buttonIndex) {
        var active = buttonIndex === index;
        button.setAttribute("aria-selected", active ? "true" : "false");
        button.tabIndex = active ? 0 : -1;
      });
      var panel = htmlElement("div", "modal-tab-panel");
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", "modal-tab-" + index);
      dom.modalContent.replaceChildren(panel);
      try {
        var result = tabs[index].render(panel);
        if (result && typeof result.catch === "function") {
          result.catch(function (error) {
            if (panel.isConnected && selected === index) {
              showContentError(panel, error);
            }
          });
        }
      } catch (error) {
        showContentError(panel, error);
      }
    }

    tabs.forEach(function (tab, index) {
      var button = htmlElement("button", "modal-tab", tab.label);
      button.type = "button";
      button.id = "modal-tab-" + index;
      button.setAttribute("role", "tab");
      button.setAttribute("aria-selected", "false");
      button.addEventListener("click", function () {
        activate(index);
      });
      button.addEventListener("keydown", function (event) {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
          return;
        }
        event.preventDefault();
        var direction = event.key === "ArrowRight" ? 1 : -1;
        var next = (index + direction + buttons.length) % buttons.length;
        buttons[next].focus();
        activate(next);
      });
      buttons.push(button);
    });
    dom.modalTabs.replaceChildren.apply(dom.modalTabs, buttons);
    activate(clamp(initialIndex || 0, 0, Math.max(0, tabs.length - 1)));
  }

  function candidateUrls(path, preferredBase) {
    if (!path) {
      throw new Error("No file path was recorded for this item.");
    }
    var bases = [
      window.location.href,
      preferredBase,
      new URL(DATA_URL, window.location.href).href
    ].filter(Boolean);
    var candidates = [];
    var seen = new Set();
    bases.forEach(function (base) {
      var candidate = new URL(path, base);
      var sameOrigin = candidate.origin === window.location.origin;
      var allowedProtocol =
        candidate.protocol === "http:" ||
        candidate.protocol === "https:";
      if (!sameOrigin || !allowedProtocol || candidate.username || candidate.password) {
        return;
      }
      if (!seen.has(candidate.href)) {
        seen.add(candidate.href);
        candidates.push(candidate);
      }
    });
    if (!candidates.length) {
      throw new Error("Refusing to load a path outside this timeline site.");
    }
    return candidates;
  }

  async function fetchPath(path, format, preferredBase) {
    var candidates = candidateUrls(path, preferredBase);
    var lastError = null;
    for (var index = 0; index < candidates.length; index += 1) {
      try {
        var response = await fetch(candidates[index].href, {
          credentials: "same-origin",
          cache: "no-store"
        });
        if (!response.ok) {
          throw new Error("HTTP " + response.status + " for " + candidates[index].pathname);
        }
        return {
          content: format === "json" ? await response.json() : await response.text(),
          url: candidates[index].href
        };
      } catch (error) {
        lastError = error;
      }
    }
    throw lastError || new Error("Unable to load " + path);
  }

  function linkForUrl(url, label) {
    var link = htmlElement("a", "button button-primary", label);
    try {
      link.href = candidateUrls(url)[0].href;
      link.target = "_blank";
      link.rel = "noopener";
    } catch (_error) {
      link.removeAttribute("href");
      link.setAttribute("aria-disabled", "true");
    }
    return link;
  }

  function uniqueArtifactIds(value) {
    var seen = new Set();
    var result = [];
    array(value).forEach(function (rawId) {
      var id = text(rawId);
      if (/^artifact-[a-z0-9-]+$/.test(id) && !seen.has(id)) {
        seen.add(id);
        result.push(id);
      }
    });
    return result;
  }

  function artifactAssociation(owner, fallback) {
    var source = owner && typeof owner === "object" ? owner : {};
    var backup = fallback && typeof fallback === "object" ? fallback : {};
    var allIds = uniqueArtifactIds(
      Array.isArray(source.artifact_ids) ? source.artifact_ids : backup.artifact_ids
    );
    var outputIds = uniqueArtifactIds(
      Array.isArray(source.output_artifact_ids)
        ? source.output_artifact_ids
        : backup.output_artifact_ids
    );
    var allSet = new Set(allIds);
    outputIds.forEach(function (id) {
      if (!allSet.has(id)) {
        allSet.add(id);
        allIds.push(id);
      }
    });
    var outputSet = new Set(outputIds);
    return {
      all: allIds,
      outputs: allIds.filter(function (id) { return outputSet.has(id); }),
      references: allIds.filter(function (id) { return !outputSet.has(id); })
    };
  }

  function safeArtifactTarget(value) {
    var raw = text(value).trim();
    if (!raw || raw.startsWith("//")) {
      return null;
    }
    try {
      var url = new URL(raw, window.location.href);
      if (url.username || url.password) {
        return null;
      }
      var sameOrigin = url.origin === window.location.origin;
      if ((!sameOrigin && url.protocol !== "https:") ||
          (sameOrigin && url.protocol !== "http:" && url.protocol !== "https:")) {
        return null;
      }
      return { href: url.href, external: !sameOrigin };
    } catch (_error) {
      return null;
    }
  }

  function artifactLink(url, label, className) {
    var target = safeArtifactTarget(url);
    if (!target) {
      return htmlElement("span", (className || "") + " artifact-link-disabled", label);
    }
    var link = htmlElement("a", className || "", label);
    link.href = target.href;
    link.dataset.linkScope = target.external ? "external" : "internal";
    if (target.external) {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    }
    return link;
  }

  function artifactKindLabel(value) {
    var labels = {
      build_artifact: "Build artifact",
      commit: "Commit",
      diff: "Diff",
      gist: "Gist",
      issue: "Issue",
      merge_request: "Merge request",
      paste: "Paste",
      pull_request: "Pull request",
      repository: "Repository",
      task: "Task",
      uploaded_file: "Uploaded file",
      url: "Link"
    };
    return labels[text(value)] || "Artifact";
  }

  function artifactRelationLabel(value, output) {
    var labels = {
      produced: "Produced",
      published: "Published",
      updated: "Updated",
      referenced: "Referenced"
    };
    return labels[text(value)] || (output ? "Output" : "Referenced");
  }

  function artifactEvidence(artifact, context) {
    var records = array(artifact && artifact.evidence);
    if (!context || typeof context !== "object") {
      return records;
    }
    var start = number(context.start_ms, -Infinity);
    var end = number(context.end_ms, Infinity);
    var agentId = text(context.agent_id);
    return records.filter(function (evidence) {
      var at = number(evidence && evidence.timestamp_ms, NaN);
      return Number.isFinite(at) && at >= start && at < end &&
        (!agentId || text(evidence.thread_id) === agentId);
    });
  }

  function artifactRelations(artifact, context, output) {
    var outputRelations = new Set(["produced", "published", "updated"]);
    var seen = new Set();
    var result = [];
    artifactEvidence(artifact, context).forEach(function (evidence) {
      var relation = text(evidence.relation);
      if ((output && outputRelations.has(relation)) ||
          (!output && relation === "referenced")) {
        if (!seen.has(relation)) {
          seen.add(relation);
          result.push(relation);
        }
      }
    });
    return result.length ? result : [output ? "output" : "referenced"];
  }

  function phaseForArtifactEvidence(evidence) {
    var at = number(evidence && evidence.timestamp_ms, NaN);
    var agentId = text(evidence && evidence.thread_id);
    if (!Number.isFinite(at) || !agentId) {
      return null;
    }
    var phases = app.phasesByAgent.get(agentId) || [];
    for (var index = 0; index < phases.length; index += 1) {
      var phase = phases[index];
      if (at >= number(phase.start_ms, Infinity) && at < number(phase.end_ms, -Infinity)) {
        return phase;
      }
    }
    return null;
  }

  function artifactEvidenceLinks(artifact, context) {
    var container = htmlElement("div", "artifact-evidence-links");
    var seen = new Set();
    artifactEvidence(artifact, context).forEach(function (evidence) {
      var phase = phaseForArtifactEvidence(evidence);
      if (!phase || seen.has(text(phase.id))) {
        return;
      }
      var agent = app.agentsById.get(text(phase.agent_id));
      if (!agent) {
        return;
      }
      seen.add(text(phase.id));
      var button = htmlElement(
        "button",
        "artifact-phase-link",
        agentShortName(agent) + " · " + text(phase.phrase, "Work phase")
      );
      button.type = "button";
      button.dataset.phaseId = text(phase.id);
      button.addEventListener("click", function () {
        openPhaseModal(phase, agent);
      });
      container.appendChild(button);
    });
    return container;
  }

  function artifactCard(artifact, output, context) {
    var id = text(artifact.artifact_id);
    var card = htmlElement("article", "artifact-card");
    card.dataset.artifactId = id;
    card.dataset.artifactRole = output ? "output" : "reference";

    var heading = htmlElement("div", "artifact-card-heading");
    heading.appendChild(htmlElement("span", "artifact-kind", artifactKindLabel(artifact.kind)));
    artifactRelations(artifact, context, output).forEach(function (relation) {
      var badge = htmlElement(
        "span",
        "artifact-relation artifact-relation-" + normalizeKind(
          relation,
          ["produced", "published", "updated", "referenced", "output"],
          output ? "output" : "referenced"
        ),
        artifactRelationLabel(relation, output)
      );
      heading.appendChild(badge);
    });
    card.appendChild(heading);

    var label = text(artifact.label, text(artifact.title, artifactKindLabel(artifact.kind)));
    card.appendChild(artifactLink(artifact.url, label, "artifact-primary-link"));
    var title = text(artifact.title);
    if (title && title !== label) {
      card.appendChild(htmlElement("div", "artifact-title", title));
    }

    var projectLabel = text(artifact.project_slug);
    if (projectLabel) {
      var project = htmlElement("div", "artifact-project");
      project.append(
        document.createTextNode("Project: "),
        artifactLink(artifact.project_url, projectLabel, "artifact-project-link")
      );
      card.appendChild(project);
    }

    var evidence = artifactEvidence(artifact, context);
    if (evidence.length) {
      var actions = [];
      var seenActions = new Set();
      evidence.forEach(function (record) {
        var action = text(record.action).replace(/_/g, " ");
        if (action && !seenActions.has(action)) {
          seenActions.add(action);
          actions.push(action);
        }
      });
      var firstAt = Math.min.apply(null, evidence.map(function (record) {
        return number(record.timestamp_ms, Infinity);
      }));
      var evidenceText = formatCount(evidence.length) +
        (evidence.length === 1 ? " evidence record" : " evidence records");
      if (Number.isFinite(firstAt)) {
        evidenceText += " · " + formatFullTime(firstAt);
      }
      if (actions.length) {
        evidenceText += " · " + actions.join(", ");
      }
      card.appendChild(htmlElement("div", "artifact-evidence", evidenceText));
      var phaseLinks = artifactEvidenceLinks(artifact, context);
      if (phaseLinks.childElementCount) {
        card.appendChild(phaseLinks);
      }
    }
    return card;
  }

  function artifactSection(title, role, ids, output, context) {
    var section = htmlElement("section", "artifact-section");
    section.dataset.artifactSection = role;
    section.appendChild(
      htmlElement("h3", "artifact-section-title", title + " (" + formatCount(ids.length) + ")")
    );
    var cards = [];
    ids.forEach(function (id) {
      var artifact = app.artifactsById.get(id);
      if (artifact) {
        cards.push(artifactCard(artifact, output, context));
      }
    });
    if (cards.length) {
      var grid = htmlElement("div", "artifact-grid");
      grid.replaceChildren.apply(grid, cards);
      section.appendChild(grid);
    } else {
      section.appendChild(htmlElement(
        "p",
        "artifact-section-empty",
        output
          ? "No output-changing artifact evidence was captured in this range."
          : "No separately referenced artifacts were captured in this range."
      ));
    }
    return section;
  }

  function renderArtifacts(container, owner, fallback, context) {
    var association = artifactAssociation(owner, fallback);
    container.dataset.artifactCount = String(association.all.length);
    if (app.artifactCatalogState === "error") {
      container.appendChild(htmlElement(
        "div",
        "artifact-catalog-warning",
        "Artifact links are unavailable: " + app.artifactCatalogError
      ));
    }
    if (!association.all.length) {
      container.appendChild(htmlElement(
        "div",
        "empty-message",
        "No evidence-backed work artifacts were associated with this range."
      ));
      return;
    }
    container.append(
      artifactSection("Work outputs", "outputs", association.outputs, true, context),
      artifactSection(
        "Referenced artifacts",
        "references",
        association.references,
        false,
        context
      )
    );
    var knownCount = association.all.filter(function (id) {
      return app.artifactsById.has(id);
    }).length;
    if (knownCount < association.all.length && app.artifactCatalogState !== "error") {
      container.appendChild(htmlElement(
        "div",
        "artifact-catalog-warning",
        formatCount(association.all.length - knownCount) +
          " associated artifact record(s) were not present in this catalog."
      ));
    }
  }

  function artifactTab(owner, fallback, context) {
    var count = artifactAssociation(owner, fallback).all.length;
    if (!count) {
      return null;
    }
    return {
      label: "Artifacts (" + formatCount(count) + ")",
      render: function (container) {
        renderArtifacts(container, owner, fallback, context);
      }
    };
  }

  function renderWorkSummary(container, entries) {
    var items = array(entries);
    if (!items.length) {
      container.appendChild(
        htmlElement("div", "empty-message", "No work-summary entries fall in this phase.")
      );
      return;
    }
    var list = htmlElement("ol", "work-summary-list");
    items.forEach(function (entry) {
      var item = htmlElement("li", "work-summary-item");
      item.append(
        htmlElement("time", "entry-time", formatFullTime(number(entry.at_ms, NaN))),
        array(entry.pull_requests).length
          ? referenceTextElement(
              text(entry.text, "No summary text."),
              entry.pull_requests,
              "entry-text"
            )
          : markdownElement(
              text(entry.text, "No summary text."),
              "entry-text",
              true
            )
      );
      list.appendChild(item);
    });
    container.appendChild(list);
  }

  function toolsLine(tools) {
    var items = array(tools)
      .map(function (tool) {
        var count = Math.max(1, Number(tool.count) || 1);
        return { name: text(tool.name, "tool"), count: count };
      })
      .filter(function (tool) { return tool.name; });
    if (!items.length) {
      return "";
    }
    var total = items.reduce(function (sum, tool) { return sum + tool.count; }, 0);
    var details = items.map(function (tool) {
      return formatCount(tool.count) + " " + tool.name;
    });
    return formatCount(total) + (total === 1 ? " tool used: " : " tools used: ") + details.join(", ");
  }

  function roleClass(role) {
    return "transcript-entry-" + transcriptRole(role);
  }

  function transcriptRole(role) {
    return normalizeKind(
      role,
      ["user", "assistant", "agent", "system", "tool"],
      "other"
    );
  }

  function renderTranscript(container, transcript, selectedRoles) {
    var entries = array(transcript);
    if (!entries.length) {
      container.appendChild(
        htmlElement("div", "empty-message", "No transcript entries fall in this phase.")
      );
      return;
    }
    var roles = [];
    entries.forEach(function (entry) {
      var role = transcriptRole(entry.role);
      if (roles.indexOf(role) < 0) {
        roles.push(role);
      }
    });
    var roleOrder = ["user", "assistant", "agent", "tool", "system", "other"];
    roles.sort(function (left, right) {
      return roleOrder.indexOf(left) - roleOrder.indexOf(right);
    });
    var activeRoles = selectedRoles || new Set(roles);
    if (!selectedRoles) {
      roles.forEach(function (role) { activeRoles.add(role); });
    }
    var controls = htmlElement("div", "transcript-filters");
    controls.setAttribute("aria-label", "Transcript message filters");
    controls.dataset.testid = "transcript-role-filters";
    var shortcuts = htmlElement("div", "transcript-filter-shortcuts");
    var choices = htmlElement("div", "transcript-filter-roles");
    var list = htmlElement("div", "transcript-list");
    function renderEntries() {
      var cards = [];
      entries.forEach(function (entry) {
        var normalizedRole = transcriptRole(entry.role);
        if (!activeRoles.has(normalizedRole)) {
          return;
        }
        var roleLabel = text(entry.role, "unknown");
        var card = htmlElement("article", "transcript-entry " + roleClass(roleLabel));
        card.dataset.role = normalizedRole;
        var header = htmlElement("header", "transcript-entry-head");
        header.append(
          htmlElement("span", "transcript-role", roleLabel),
          htmlElement("time", "entry-time", formatFullTime(number(entry.at_ms, NaN)))
        );
        card.appendChild(header);
        var condensed = toolsLine(entry.tools);
        if (condensed) {
          card.appendChild(htmlElement("div", "tool-condensation", condensed));
        } else {
          card.appendChild(
            referenceTextElement(text(entry.text, ""), entry.pull_requests, "entry-text")
          );
        }
        cards.push(card);
      });
      if (!cards.length) {
        cards.push(htmlElement("div", "empty-message", "No transcript messages match these filters."));
      }
      list.replaceChildren.apply(list, cards);
      choices.querySelectorAll("input[data-role]").forEach(function (checkbox) {
        checkbox.checked = activeRoles.has(checkbox.dataset.role);
      });
    }
    function shortcut(label, update) {
      var button = htmlElement("button", "transcript-filter-button", label);
      button.type = "button";
      button.addEventListener("click", function () {
        update();
        renderEntries();
      });
      shortcuts.appendChild(button);
    }
    shortcut("User only", function () {
      activeRoles.clear();
      activeRoles.add("user");
    });
    shortcut("Select all", function () {
      roles.forEach(function (role) { activeRoles.add(role); });
    });
    shortcut("Select none", function () { activeRoles.clear(); });
    roles.forEach(function (role) {
      var label = htmlElement("label", "transcript-filter-choice");
      var checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.dataset.role = role;
      checkbox.checked = activeRoles.has(role);
      checkbox.addEventListener("change", function () {
        if (checkbox.checked) {
          activeRoles.add(role);
        } else {
          activeRoles.delete(role);
        }
        renderEntries();
      });
      label.append(checkbox, document.createTextNode(role));
      choices.appendChild(label);
    });
    controls.append(shortcuts, choices);
    container.append(controls, list);
    renderEntries();
  }

  async function renderRawSummary(container, path, detailUrl) {
    showLoading(container, "Loading raw summary…");
    var loaded = await fetchPath(path, "text", detailUrl);
    var toolbar = htmlElement("div", "raw-summary-toolbar");
    toolbar.appendChild(linkForUrl(loaded.url, "Open Markdown file"));
    container.replaceChildren(
      toolbar,
      markdownElement(loaded.content, "markdown-document", false)
    );
  }

  function phaseFallbackDetail(phase) {
    return {
      phrase: text(phase.phrase, "Agent phase"),
      paragraph: text(phase.paragraph),
      stats: phase.stats || {},
      work_summary: [],
      transcript: [],
      raw_summary_path: ""
    };
  }

  function renderAgentLifetimeOverview(container, agent) {
    var phases = app.phasesByAgent.get(text(agent.id)) || [];
    container.appendChild(
      htmlElement("h3", "agent-lifetime-section-title", "Work phases (" +
        formatCount(phases.length) + ")")
    );
    if (!phases.length) {
      container.appendChild(htmlElement(
        "div",
        "empty-message",
        "No summarized work phases were retained for this agent."
      ));
      return;
    }
    var list = htmlElement("div", "agent-lifetime-phase-list");
    phases.forEach(function (phase) {
      var button = htmlElement("button", "agent-lifetime-phase");
      button.type = "button";
      button.dataset.phaseId = text(phase.id);
      button.append(
        htmlElement(
          "time",
          "agent-lifetime-phase-time",
          formatRange(number(phase.start_ms, 0), number(phase.end_ms, 0))
        ),
        htmlElement("strong", "agent-lifetime-phase-title", text(phase.phrase, "Work phase")),
        htmlElement("span", "agent-lifetime-phase-summary", text(phase.paragraph))
      );
      button.addEventListener("click", function () {
        openPhaseModal(phase, agent);
      });
      list.appendChild(button);
    });
    container.appendChild(list);
  }

  function openAgentLifetimeModal(agent) {
    app.detailRequest += 1;
    var start = number(agent.start_ms, app.data.range.start_ms);
    var end = number(agent.end_ms, app.data.range.end_ms);
    openModalBase(
      "Agent lifetime · " + formatRange(start, end),
      agentShortName(agent),
      agentLifetimeSummary(agent),
      null
    );
    dom.modalEyebrow.title = agentAccessibleName(agent);
    showModalAgentIdentity(agent);
    var tabs = [
      {
        label: "Work Phases",
        render: function (container) {
          renderAgentLifetimeOverview(container, agent);
        }
      }
    ];
    var artifacts = artifactTab(agent, null, {
      start_ms: start,
      end_ms: end,
      agent_id: agent.id
    });
    if (artifacts) {
      tabs.push(artifacts);
    }
    activateTabs(tabs, 0);
  }

  function showPhaseDetail(detail, phase, agent, detailUrl) {
    var phrase = text(detail.phrase, text(phase.phrase, "Agent phase"));
    var paragraph = text(detail.paragraph, text(phase.paragraph));
    var stats = detail.stats && typeof detail.stats === "object"
      ? detail.stats
      : (phase.stats || {});
    dom.modalTitle.textContent = phrase;
    dom.modalEyebrow.textContent =
      agentShortName(agent) +
      " · " + formatRange(number(phase.start_ms, 0), number(phase.end_ms, 0));
    dom.modalEyebrow.title = agentAccessibleName(agent);
    setModalSummary(paragraph, stats);
    showModalAgentIdentity(agent);
    var transcriptRoles = new Set(array(detail.transcript).map(function (entry) {
      return transcriptRole(entry.role);
    }));
    var tabs = [
      {
        label: "Agent Work Summary",
        render: function (container) {
          renderWorkSummary(container, detail.work_summary);
        }
      },
      {
        label: "Full Transcript",
        render: function (container) {
          renderTranscript(container, detail.transcript, transcriptRoles);
        }
      },
      {
        label: "Markdown Summary",
        render: function (container) {
          var path = text(detail.raw_summary_path);
          if (!path) {
            container.appendChild(
              htmlElement("div", "empty-message", "No raw summary file was recorded for this phase.")
            );
            return undefined;
          }
          return renderRawSummary(container, path, detailUrl);
        }
      }
    ];
    var artifacts = artifactTab(detail, phase, {
      start_ms: phase.start_ms,
      end_ms: phase.end_ms,
      agent_id: phase.agent_id
    });
    if (artifacts) {
      tabs.splice(tabs.length - 1, 0, artifacts);
    }
    activateTabs(tabs, 0);
  }

  async function openPhaseModal(phase, agent) {
    var request = ++app.detailRequest;
    openModalBase(
      agentShortName(agent) + " · phase",
      text(phase.phrase, "Agent phase"),
      text(phase.paragraph),
      phase.stats || {}
    );
    dom.modalEyebrow.title = agentAccessibleName(agent);
    showModalAgentIdentity(agent);
    showLoading(dom.modalContent, "Loading phase transcript and summaries…");
    var path = text(phase.detail_path);
    if (!path) {
      showPhaseDetail(phaseFallbackDetail(phase), phase, agent);
      return;
    }
    try {
      var loaded = await fetchPath(path, "json");
      if (request !== app.detailRequest || dom.modalBackdrop.hidden) {
        return;
      }
      var detail = loaded.content;
      if (!detail || typeof detail !== "object") {
        throw new Error("Phase detail is not a JSON object.");
      }
      showPhaseDetail(detail, phase, agent, loaded.url);
    } catch (error) {
      if (request !== app.detailRequest || dom.modalBackdrop.hidden) {
        return;
      }
      dom.modalTabs.hidden = true;
      showContentError(dom.modalContent, error);
    }
  }

  function openEdgeModal(edge) {
    app.detailRequest += 1;
    var kind = edgeKind(edge.kind);
    openModalBase(
      kind + " interaction · " +
        formatFullTime(number(edge.source_ms, number(edge.target_ms, NaN))),
      text(edge.phrase, kind + " interaction"),
      text(edge.paragraph),
      null
    );
    dom.modalEyebrow.title = edgeRouteDetail(edge);
    showModalEdgeRoute(edge);
    dom.modalTabs.hidden = true;
    var content = htmlElement("div", "edge-detail");
    var fullText = text(edge.full_text);
    if (fullText) {
      content.appendChild(htmlElement("pre", "edge-full-text", fullText));
    } else {
      content.appendChild(
        htmlElement(
          "div",
          "content-limitation",
          text(
            edge.content_status,
            "The full interaction text was not retained in this timeline dataset."
          )
        )
      );
    }
    dom.modalContent.replaceChildren(content);
  }

  async function openMarkdownModal(options) {
    var request = ++app.detailRequest;
    openModalBase(options.eyebrow, options.title, "", null);
    dom.modalTabs.hidden = true;
    showLoading(dom.modalContent, "Loading summary file…");
    try {
      var loaded = await fetchPath(options.path, "text");
      if (request !== app.detailRequest || dom.modalBackdrop.hidden) {
        return;
      }
      var toolbar = htmlElement("div", "raw-summary-toolbar");
      toolbar.appendChild(linkForUrl(loaded.url, "Open Markdown file"));
      dom.modalContent.replaceChildren(
        toolbar,
        markdownElement(loaded.content, "markdown-document", false)
      );
    } catch (error) {
      if (request === app.detailRequest && !dom.modalBackdrop.hidden) {
        showContentError(dom.modalContent, error);
      }
    }
  }

  function setGlossaryHash(id) {
    var suffix = id ? "/" + id : "";
    window.history.pushState(null, "", "#glossary" + suffix);
  }

  function openGlossaryEntry(entry, updateLocation) {
    var id = glossaryId(entry && entry.id);
    if (!id || app.glossaryById.get(id) !== entry) {
      return;
    }
    app.detailRequest += 1;
    if (updateLocation) {
      setGlossaryHash(id);
    }
    openModalBase(
      "Project glossary · " + text(entry.week, "term"),
      text(entry.term, "Glossary entry"),
      "",
      null
    );
    var metadata = htmlElement("div", "glossary-entry-meta");
    metadata.appendChild(
      htmlElement("span", "", formatCount(entry.occurrences) + " source occurrence(s)")
    );
    if (Number.isFinite(number(entry.introduced_at_ms, NaN))) {
      metadata.appendChild(
        htmlElement("span", "", "Introduced " + formatFullTime(number(entry.introduced_at_ms, 0)))
      );
    }
    var permalink = htmlElement("a", "glossary-permalink", "Permanent glossary link");
    permalink.href = "#glossary/" + id;
    metadata.appendChild(permalink);
    dom.modalContent.replaceChildren(
      metadata,
      markdownElement(
        "## Source-backed explanation\n\n" + text(entry.context, "No source context was retained."),
        "glossary-entry",
        false
      )
    );
  }

  function openGlossaryCatalog(updateLocation) {
    if (updateLocation) {
      setGlossaryHash("");
    }
    var path = text(app.data && app.data.glossary_path);
    if (path) {
      openMarkdownModal({
        eyebrow: "Project terminology",
        title: "Project glossary",
        path: path
      });
      return;
    }
    openModalBase("Project terminology", "Project glossary", "", null);
    var list = htmlElement("div", "glossary-list");
    app.glossaryById.forEach(function (entry) {
      var link = htmlElement("a", "glossary-term-link", text(entry.term));
      link.href = "#glossary/" + glossaryId(entry.id);
      link.addEventListener("click", function (event) {
        event.preventDefault();
        openGlossaryEntry(entry, true);
      });
      list.appendChild(link);
    });
    dom.modalContent.replaceChildren(list);
  }

  function openGlossaryFromHash() {
    if (!app.data || !window.location.hash.startsWith("#glossary")) {
      return;
    }
    var prefix = "#glossary/";
    if (!window.location.hash.startsWith(prefix)) {
      openGlossaryCatalog(false);
      return;
    }
    var encoded = window.location.hash.slice(prefix.length);
    var id;
    try {
      id = decodeURIComponent(encoded);
    } catch (_error) {
      return;
    }
    var entry = app.glossaryById.get(glossaryId(id));
    if (entry) {
      openGlossaryEntry(entry, false);
    }
  }

  function openRollupModal(rollup, kind, start, end) {
    app.detailRequest += 1;
    openModalBase(
      kind + " rollup · " + formatRange(start, end),
      text(rollup.label, kind + " summary"),
      "",
      null
    );
    var technicalPath = text(rollup.technical_path, text(rollup.path));
    var plainPath = text(rollup.plain_language_path);
    var tabs = [
      {
        label: "Technical",
        render: function (container) {
          return renderRawSummary(container, technicalPath, "");
        }
      }
    ];
    if (plainPath) {
      tabs.push({
        label: "Plain Language",
        render: function (container) {
          return renderRawSummary(container, plainPath, "");
        }
      });
    }
    var artifacts = artifactTab(rollup, null, {
      start_ms: start,
      end_ms: end
    });
    if (artifacts) {
      tabs.push(artifacts);
    }
    activateTabs(tabs, 0);
  }

  function showLoadError(error) {
    var message = error instanceof Error ? error.message : String(error);
    dom.meta.textContent = "Timeline could not be loaded";
    dom.loadError.textContent =
      "Could not load " + DATA_URL + ": " + message +
      ". Serve the generated site over HTTP so its data files are available.";
    dom.loadError.hidden = false;
  }

  async function loadTimeline() {
    try {
      var response = await fetch(DATA_URL, {
        credentials: "same-origin",
        cache: "no-store"
      });
      if (!response.ok) {
        throw new Error("HTTP " + response.status + " " + response.statusText);
      }
      var raw = await response.json();
      var data = normalizeData(raw);
      await loadArtifactCatalog(data);
      initializeData(data);
    } catch (error) {
      showLoadError(error);
    }
  }

  dom.teamFilter.addEventListener("change", function () {
    app.selectedTeam = dom.teamFilter.value;
    dom.scroll.scrollTop = 0;
    scheduleRender();
  });

  dom.search.addEventListener("input", function () {
    app.query = dom.search.value.trim();
    dom.scroll.scrollTop = 0;
    scheduleRender();
  });

  dom.fit.addEventListener("click", fitTimeline);
  dom.glossaryOpen.addEventListener("click", function () {
    openGlossaryCatalog(true);
  });
  dom.perAgentTracks.addEventListener("change", function () {
    app.perAgentTracks = dom.perAgentTracks.checked;
    dom.scroll.scrollTop = 0;
    scheduleRender();
  });
  dom.showGlobalMessages.addEventListener("change", function () {
    app.showGlobalMessages = dom.showGlobalMessages.checked;
    scheduleRender();
  });
  dom.showHighlightedMessages.addEventListener("change", function () {
    app.showHighlightedMessages = dom.showHighlightedMessages.checked;
    scheduleRender();
  });
  dom.card.addEventListener("keydown", keyboardNavigate);
  dom.svg.addEventListener("wheel", wheelZoom, { passive: false });
  dom.axis.addEventListener("wheel", wheelZoom, { passive: false });
  dom.rollupTrack.addEventListener("wheel", wheelZoom, { passive: false });
  dom.svg.addEventListener("pointerdown", beginPan);
  dom.svg.addEventListener("pointermove", continuePan);
  dom.svg.addEventListener("pointerup", endPan);
  dom.svg.addEventListener("pointercancel", endPan);
  dom.svg.addEventListener("click", clearSelectionFromEmptyTrack);
  dom.scroll.addEventListener("scroll", scheduleRender, { passive: true });
  dom.scroll.addEventListener("keydown", keyboardScrollTracks);
  window.addEventListener("hashchange", openGlossaryFromHash);

  dom.modalClose.addEventListener("click", closeModal);
  dom.modalBackdrop.addEventListener("click", function (event) {
    if (event.target === dom.modalBackdrop) {
      closeModal();
    }
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") {
      if (!dom.laneMenu.hidden) {
        hideLaneAgentMenu(true);
      } else if (!dom.contextMenu.hidden) {
        hideContextMenu();
      } else if (!dom.modalBackdrop.hidden) {
        closeModal();
      } else if (app.selection) {
        setSelection(null);
      }
      dom.summaryMenu.open = false;
    }
    if (!dom.modalBackdrop.hidden && event.key === "Tab") {
      var focusable = Array.from(dom.modalBackdrop.querySelectorAll(
        "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex='-1'])"
      ));
      if (!focusable.length) {
        return;
      }
      var first = focusable[0];
      var last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  });
  document.addEventListener("pointerdown", function (event) {
    if (!dom.contextMenu.hidden && !dom.contextMenu.contains(event.target)) {
      hideContextMenu();
    }
    if (!dom.laneMenu.hidden && !dom.laneMenu.contains(event.target) &&
        event.target !== app.laneMenuAnchor &&
        !(app.laneMenuAnchor && app.laneMenuAnchor.contains(event.target))) {
      hideLaneAgentMenu(false);
    }
  });
  window.addEventListener("blur", function () {
    hideContextMenu();
    hideLaneAgentMenu(false);
  });

  if (typeof ResizeObserver === "function") {
    new ResizeObserver(scheduleRender).observe(dom.card);
  } else {
    window.addEventListener("resize", scheduleRender, { passive: true });
  }

  loadTimeline();
}());
