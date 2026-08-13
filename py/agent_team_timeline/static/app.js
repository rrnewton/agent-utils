(function () {
  "use strict";

  var DATA_URL = "data/timeline.json";
  var SCHEMA_2_URL = "data/timeline-v2.json";
  var SVG_NS = "http://www.w3.org/2000/svg";
  var ROW_HEIGHT = 54;
  var PHASE_TOP = 7;
  var PHASE_HEIGHT = 38;
  var COMPACT_PHASE_TOP = 16;
  var COMPACT_PHASE_HEIGHT = 31;
  var COMPACT_LABEL_WIDTH = 72;
  var AGGREGATE_LABEL_WIDTH = 128;
  var AGGREGATE_TEAM_HEIGHT = 78;
  var AGGREGATE_WORKER_SCALE = 10;
  var AGGREGATE_WORKER_MAX_HEIGHT = 48;
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
    continuation: "#f6c453",
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
    zoomOut: byId("zoom-out"),
    zoomIn: byId("zoom-in"),
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
    teamActivityScores: new Map(),
    summaryRangesByTeam: new Map(),
    glossaryById: new Map(),
    artifactsById: new Map(),
    artifactCatalogState: "legacy",
    artifactCatalogError: "",
    artifactCatalogPromise: null,
    axisTicks: [],
    selectedTeam: "",
    query: "",
    perAgentTracks: false,
    showGlobalMessages: false,
    showHighlightedMessages: true,
    selection: null,
    rangeSelection: null,
    drag: null,
    suppressClickUntil: 0,
    renderQueued: false,
    renderLod: "detail",
    renderRevision: 0,
    detailRequest: 0,
    modalRestoreFocus: null,
    laneMenuAnchor: null,
    timezone: "UTC",
    timezoneStatus: "archive fallback",
    schemaMode: "schema1",
    shardCatalog: [],
    resourcePromises: new Map(),
    detailPromises: new Map(),
    loadedShardUrls: new Set(),
    phaseIds: new Set(),
    edgeIds: new Set(),
    searchShardState: "legacy",
    phaseIndexReference: null,
    phaseIndexPromise: null,
    phaseIndexByAgent: new Map(),
    detailErrorActive: false
  };

  var timelineCore = window.AgentTimelineCore;

  var markdownRenderer = typeof window.markdownit === "function"
    ? window.markdownit({
        html: false,
        linkify: false,
        typographer: false,
        breaks: false
      })
    : null;

  if (markdownRenderer) {
    markdownRenderer.renderer.rules.link_open = function () { return ""; };
    markdownRenderer.renderer.rules.link_close = function () { return ""; };
    markdownRenderer.renderer.rules.image = function (tokens, index) {
      return markdownRenderer.utils.escapeHtml(text(tokens[index].content));
    };
  }

  function array(value) {
    return Array.isArray(value) ? value : [];
  }

  function number(value, fallback) {
    return Number.isFinite(value) ? Number(value) : fallback;
  }

  function text(value, fallback) {
    return typeof value === "string" ? value : (fallback || "");
  }

  function hasField(record, field) {
    return Boolean(record) && typeof record === "object" &&
      Object.prototype.hasOwnProperty.call(record, field);
  }

  function summaryFlagAvailable(record, field) {
    return !hasField(record, field) || record[field] !== false;
  }

  function agentSummaryAvailable(agent) {
    return summaryFlagAvailable(agent, "summary_available");
  }

  function phaseSummaryAvailable(phase) {
    return summaryFlagAvailable(phase, "summary_available");
  }

  function phaseDetailSummaryAvailable(detail, phase) {
    if (hasField(detail, "summary_available")) {
      return detail.summary_available !== false;
    }
    return phaseSummaryAvailable(phase);
  }

  function rollupSummaryAvailable(rollup) {
    if (hasField(rollup, "summary_available")) {
      return rollup.summary_available !== false;
    }
    if (hasField(rollup, "technical_summary_available") ||
        hasField(rollup, "plain_language_summary_available")) {
      return rollup.technical_summary_available !== false ||
        rollup.plain_language_summary_available !== false;
    }
    return true;
  }

  function rollupAudienceAvailable(rollup, field) {
    return rollupSummaryAvailable(rollup) && summaryFlagAvailable(rollup, field);
  }

  function recordedAgentSummaryAvailable(agent) {
    if (hasField(agent, "summary_available")) {
      return agent.summary_available !== false;
    }
    return Boolean(text(agent.lifetime_summary));
  }

  function recordedPhaseSummaryAvailable(phase) {
    if (hasField(phase, "summary_available")) {
      return phase.summary_available !== false;
    }
    return Boolean(text(phase.raw_summary_path) || text(phase.summary_path));
  }

  function recordedRollupSummaryAvailable(rollup) {
    if (hasField(rollup, "summary_available") ||
        hasField(rollup, "technical_summary_available") ||
        hasField(rollup, "plain_language_summary_available")) {
      return rollupSummaryAvailable(rollup);
    }
    return Boolean(
      text(rollup.path) ||
      text(rollup.technical_path) ||
      text(rollup.plain_language_path)
    );
  }

  function rebuildSummaryRanges() {
    var byTeam = new Map();
    function add(team, startValue, endValue) {
      var start = number(startValue, NaN);
      var end = number(endValue, NaN);
      if (!team || !Number.isFinite(start) || !Number.isFinite(end) || end <= start) {
        return;
      }
      if (!byTeam.has(team)) {
        byTeam.set(team, []);
      }
      byTeam.get(team).push({ start_ms: start, end_ms: end });
    }
    app.data.rollups.forEach(function (rollup) {
      if (recordedRollupSummaryAvailable(rollup)) {
        add(text(rollup.team), rollup.start_ms, rollup.end_ms);
      }
    });
    app.data.agents.forEach(function (agent) {
      // A coordinator lifetime can span the entire archive and would falsely imply
      // fine-grained coverage everywhere. Subagent lifetime summaries remain useful.
      if (text(agent.parent_id) && recordedAgentSummaryAvailable(agent)) {
        add(text(agent.team), agent.start_ms, agent.end_ms);
      }
    });
    app.data.phases.forEach(function (phase) {
      if (!recordedPhaseSummaryAvailable(phase)) {
        return;
      }
      var agent = app.agentsById.get(text(phase.agent_id));
      add(text(agent && agent.team), phase.start_ms, phase.end_ms);
    });
    byTeam.forEach(function (ranges) {
      ranges.sort(function (left, right) {
        return left.start_ms - right.start_ms || left.end_ms - right.end_ms;
      });
    });
    app.summaryRangesByTeam = byTeam;
  }

  function summaryAvailableInRange(team, start, end) {
    var ranges = app.summaryRangesByTeam.get(team) || [];
    for (var index = 0; index < ranges.length; index += 1) {
      var range = ranges[index];
      if (range.start_ms >= end) {
        return false;
      }
      if (range.end_ms > start) {
        return true;
      }
    }
    return false;
  }

  function rebuildTeamActivityScores() {
    var scoresByResolution = new Map();
    app.data.activity_bins.forEach(function (bin) {
      var team = text(bin.team);
      var resolution = text(bin.resolution);
      var start = number(bin.start_ms, NaN);
      var end = number(bin.end_ms, NaN);
      if (!team || !resolution || !Number.isFinite(start) ||
          !Number.isFinite(end) || end <= start) {
        return;
      }
      if (!scoresByResolution.has(team)) {
        scoresByResolution.set(team, new Map());
      }
      var scores = scoresByResolution.get(team);
      var concurrency = Math.max(0, number(
        bin.avg_present_concurrency,
        number(
          bin.avg_active_concurrency,
          number(bin.activity_evidence_fraction,
            number(bin.activity_coverage_fraction, 0))
        )
      ));
      scores.set(
        resolution,
        number(scores.get(resolution), 0) + concurrency * (end - start)
      );
    });
    var totals = new Map();
    scoresByResolution.forEach(function (scores, team) {
      var resolution = scores.has("daily")
        ? "daily"
        : (scores.has("hourly") ? "hourly" : "weekly");
      totals.set(team, number(scores.get(resolution), 0));
    });
    app.teamActivityScores = totals;
  }

  function compareTeamsByActivity(left, right) {
    var leftSlug = text(left && left.slug);
    var rightSlug = text(right && right.slug);
    return teamActivitySortScore(rightSlug) - teamActivitySortScore(leftSlug) ||
      text(left && left.label, leftSlug).localeCompare(
        text(right && right.label, rightSlug),
        undefined,
        { numeric: true }
      );
  }

  function teamActivitySortScore(slug) {
    var team = app.teamBySlug.get(slug);
    var eventCount = number(team && team.stats && team.stats.events, NaN);
    if (Number.isFinite(eventCount)) {
      return eventCount;
    }
    return number(app.teamActivityScores.get(slug), 0);
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

  function removeUnexpectedMarkdownLinks(container) {
    container.querySelectorAll("a").forEach(function (link) {
      var parent = link.parentNode;
      if (!parent) {
        return;
      }
      while (link.firstChild) {
        parent.insertBefore(link.firstChild, link);
      }
      parent.removeChild(link);
    });
    container.querySelectorAll("img").forEach(function (image) {
      image.replaceWith(document.createTextNode(text(image.alt, "[image omitted]")));
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
    removeUnexpectedMarkdownLinks(container);
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
      if (!selectedTeamAllows(entry)) {
        return;
      }
      var term = text(entry.term);
      if (!term || glossaryTermIsAmbiguous(term)) {
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

  function glossaryTermIsAmbiguous(term) {
    if (app.selectedTeam) {
      return false;
    }
    var matches = 0;
    app.glossaryById.forEach(function (entry) {
      if (text(entry.term) === term) {
        matches += 1;
      }
    });
    return matches > 1;
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
        link.title = text(match.entry.definition, text(match.entry.context, "Open project glossary entry"));
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
    var values = [
      formatCount(stats.user_prompts) + " prompts",
      formatCount(stats.agent_responses) + " responses",
      formatCount(stats.inter_agent_messages) + " inter-agent",
      formatCount(stats.tool_calls) + " tools"
    ];
    if (Number(stats.external_messages) > 0) {
      values.splice(3, 0, formatCount(stats.external_messages) + " external");
    }
    return values.join(" · ");
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
      activity_bins: array(raw.activity_bins),
      edges: array(raw.edges),
      events: array(raw.events),
      rollups: array(raw.rollups),
      summary_files: array(raw.summary_files),
      glossary: array(raw.glossary),
      glossary_path: text(raw.glossary_path),
      artifact_catalog_path: text(raw.artifact_catalog_path),
      stats: raw.stats && typeof raw.stats === "object" ? raw.stats : null
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

  function prepareArtifactCatalog(data) {
    app.artifactsById.clear();
    app.artifactCatalogError = "";
    app.artifactCatalogPromise = null;
    var path = text(data && data.artifact_catalog_path);
    if (!path) {
      app.artifactCatalogState = "legacy";
    } else {
      app.artifactCatalogState = "unloaded";
    }
  }

  async function ensureArtifactCatalog(data) {
    if (app.artifactCatalogState === "legacy" ||
        app.artifactCatalogState === "ready" ||
        app.artifactCatalogState === "error") {
      return;
    }
    if (app.artifactCatalogPromise) {
      return app.artifactCatalogPromise;
    }
    var path = text(data && data.artifact_catalog_path);
    if (!path) {
      app.artifactCatalogState = "legacy";
      return;
    }
    app.artifactCatalogState = "loading";
    app.artifactCatalogPromise = (async function () {
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
    }());
    return app.artifactCatalogPromise;
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

  function indexPhase(phase) {
    if (!phase || typeof phase !== "object") {
      return;
    }
    var id = text(phase.agent_id);
    if (!app.phasesByAgent.has(id)) {
      app.phasesByAgent.set(id, []);
    }
    app.phasesByAgent.get(id).push(phase);
  }

  function sortPhaseIndexes() {
    app.phasesByAgent.forEach(function (phases) {
      phases.sort(function (left, right) {
        return number(left.start_ms, 0) - number(right.start_ms, 0) ||
          text(left.id).localeCompare(text(right.id));
      });
    });
  }

  function updateShardDiagnostics() {
    dom.card.dataset.timelineSchemaMode = app.schemaMode;
    dom.card.dataset.detailShardCount = String(app.shardCatalog.length);
    dom.card.dataset.loadedShardCount = String(app.loadedShardUrls.size);
    dom.card.dataset.searchShardState = app.searchShardState;
    dom.card.dataset.phaseIndexState = app.phaseIndexReference
      ? (app.phaseIndexPromise ? "requested" : "unloaded")
      : "legacy";
  }

  function initializeData(raw) {
    var data = normalizeData(raw);
    app.data = data;
    prepareArtifactCatalog(data);
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
    app.phaseIds.clear();
    app.edgeIds.clear();

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
      var phaseId = text(phase.id);
      if (phaseId) {
        app.phaseIds.add(phaseId);
      }
      indexPhase(phase);
    });
    sortPhaseIndexes();
    data.edges.forEach(function (edge) {
      var edgeId = text(edge && edge.id);
      if (edgeId) {
        app.edgeIds.add(edgeId);
      }
    });
    data.glossary.forEach(function (entry) {
      if (!entry || typeof entry !== "object") {
        throw new Error("glossary entries must be objects");
      }
      var id = glossaryId(entry.id);
      var term = text(entry.term);
      if (!id || !term || text(entry.url) !== "#glossary/" + id) {
        throw new Error("glossary entry has an invalid stable target");
      }
      if (app.glossaryById.has(id)) {
        throw new Error("glossary contains a duplicate ID");
      }
      app.glossaryById.set(id, entry);
    });
    rebuildTeamActivityScores();
    rebuildSummaryRanges();
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
    updateShardDiagnostics();
    scheduleRender();
    openGlossaryFromHash();
  }

  function immutableTimelineObjectUrl(reference, where) {
    if (!reference || typeof reference !== "object") {
      throw new Error(where + " must be an object reference.");
    }
    var url = text(reference.url);
    var digest = text(reference.sha256);
    var match = /^data\/timeline-v2\/objects\/([0-9a-f]{64})\.json$/.exec(url);
    if (!match || match[1] !== digest) {
      throw new Error(where + " must use its complete SHA-256 object URL.");
    }
    return url;
  }

  function mergeDetailShard(raw, url) {
    if (!raw || typeof raw !== "object" || number(raw.schema_version, NaN) !== 2 ||
        text(raw.kind) !== "timeline-detail-day") {
      throw new Error("Unsupported timeline detail shard: " + url);
    }
    if (app.loadedShardUrls.has(url)) {
      return;
    }
    var phases = array(raw.phases);
    var edges = array(raw.edges);
    var events = array(raw.events);
    var shardPhaseIds = new Set();
    var shardEdgeIds = new Set();
    phases.forEach(function (phase) {
      if (!phase || typeof phase !== "object") {
        throw new Error("Timeline detail phases must be objects.");
      }
      var id = text(phase.id);
      if (!id || shardPhaseIds.has(id)) {
        throw new Error("Timeline detail phase has a missing or duplicate stable ID.");
      }
      shardPhaseIds.add(id);
    });
    edges.forEach(function (edge) {
      if (!edge || typeof edge !== "object") {
        throw new Error("Timeline detail edges must be objects.");
      }
      var id = text(edge.id);
      if (!id || shardEdgeIds.has(id)) {
        throw new Error("Timeline detail edge has a missing or duplicate stable ID.");
      }
      shardEdgeIds.add(id);
    });
    events.forEach(function (event) {
      if (!event || typeof event !== "object") {
        throw new Error("Timeline detail events must be objects.");
      }
    });

    // Validation above is deliberately atomic: a failed response must not leave partial phases,
    // edges, or id-less events that a later retry would duplicate.
    phases.forEach(function (phase) {
      var id = text(phase.id);
      if (!app.phaseIds.has(id)) {
        app.phaseIds.add(id);
        app.data.phases.push(phase);
        indexPhase(phase);
      }
    });
    edges.forEach(function (edge) {
      var id = text(edge.id);
      if (!app.edgeIds.has(id)) {
        app.edgeIds.add(id);
        app.data.edges.push(edge);
      }
    });
    // Events belong to exactly one UTC-day object, so URL-level application is sufficient to
    // retain legitimate duplicate events while rejecting repeat fetch/application.
    events.forEach(function (event) {
      app.data.events.push(event);
    });
    sortPhaseIndexes();
    app.data.phases.sort(function (left, right) {
      return number(left.start_ms, 0) - number(right.start_ms, 0) ||
        text(left.id).localeCompare(text(right.id));
    });
    app.data.edges.sort(function (left, right) {
      return number(left.source_ms, 0) - number(right.source_ms, 0) ||
        text(left.id).localeCompare(text(right.id));
    });
    app.data.events.sort(function (left, right) {
      return number(left.at_ms, 0) - number(right.at_ms, 0) ||
        text(left.agent_id).localeCompare(text(right.agent_id));
    });
    rebuildSummaryRanges();
    app.loadedShardUrls.add(url);
    if (app.detailErrorActive) {
      app.detailErrorActive = false;
      dom.loadError.hidden = true;
      dom.loadError.textContent = "";
    }
    updateShardDiagnostics();
    scheduleRender();
  }

  function loadDetailShard(catalogEntry) {
    var url = immutableTimelineObjectUrl(catalogEntry, "detail shard");
    if (!app.detailPromises.has(url)) {
      var request = fetchJsonCached(url).then(function (raw) {
        mergeDetailShard(raw, url);
        return raw;
      });
      var cached = request.catch(function (error) {
        if (app.detailPromises.get(url) === cached) {
          app.detailPromises.delete(url);
        }
        throw error;
      });
      app.detailPromises.set(url, cached);
    }
    return app.detailPromises.get(url);
  }

  function detailShardsForRange(start, end, bufferFraction) {
    var span = Math.max(1, end - start);
    var buffer = Math.max(60 * 60 * 1000, span * bufferFraction);
    var bufferedStart = start - buffer;
    var bufferedEnd = end + buffer;
    return app.shardCatalog.filter(function (shard) {
      return number(shard.start_ms, Infinity) < bufferedEnd &&
        number(shard.end_ms, -Infinity) > bufferedStart;
    });
  }

  function requestDetailShards(shards) {
    var started = false;
    var promises = shards.map(function (shard) {
      var url = immutableTimelineObjectUrl(shard, "detail shard");
      if (!app.detailPromises.has(url)) {
        started = true;
      }
      return loadDetailShard(shard);
    });
    var promise = Promise.all(promises);
    if (started) {
      promise.then(scheduleRender, function () { return undefined; });
    }
    return { started: started, promise: promise };
  }

  function showDetailLoadError(error) {
    var message = error instanceof Error ? error.message : String(error);
    dom.loadError.textContent =
      "Could not load timeline detail: " + message +
      ". The aggregate timeline remains available.";
    dom.loadError.hidden = false;
    app.detailErrorActive = true;
  }

  function requestVisibleDetails() {
    if (app.schemaMode !== "schema2" || app.renderLod !== "detail") {
      return;
    }
    var request = requestDetailShards(
      detailShardsForRange(app.viewStart, app.viewEnd, 0.08)
    );
    request.promise.catch(showDetailLoadError);
  }

  function requestSearchCorpus() {
    if (app.schemaMode !== "schema2") {
      return;
    }
    app.searchShardState = "loading";
    updateShardDiagnostics();
    var request = requestDetailShards(app.shardCatalog);
    request.promise.then(function () {
      app.searchShardState = "ready";
      updateShardDiagnostics();
      scheduleRender();
    }).catch(function (error) {
      app.searchShardState = "error";
      updateShardDiagnostics();
      showDetailLoadError(error);
    });
  }

  function loadPhaseIndex() {
    if (!app.phaseIndexReference) {
      return Promise.resolve(null);
    }
    if (!app.phaseIndexPromise) {
      var url = immutableTimelineObjectUrl(
        app.phaseIndexReference,
        "timeline phase index"
      );
      var request = fetchJsonCached(url).then(function (raw) {
        if (!raw || typeof raw !== "object" || number(raw.schema_version, NaN) !== 2 ||
            text(raw.kind) !== "timeline-phase-index") {
          throw new Error("Unsupported timeline phase index: " + url);
        }
        var byAgent = new Map();
        var ids = new Set();
        array(raw.phases).forEach(function (phase) {
          if (!phase || typeof phase !== "object") {
            throw new Error("Timeline phase-index entries must be objects.");
          }
          var id = text(phase.id);
          var agentId = text(phase.agent_id);
          if (!id || !agentId || ids.has(id) || Array.isArray(phase.states)) {
            throw new Error("Timeline phase index has an invalid or duplicate entry.");
          }
          ids.add(id);
          if (!byAgent.has(agentId)) {
            byAgent.set(agentId, []);
          }
          byAgent.get(agentId).push(phase);
        });
        byAgent.forEach(function (phases) {
          phases.sort(function (left, right) {
            return number(left.start_ms, 0) - number(right.start_ms, 0) ||
              text(left.id).localeCompare(text(right.id));
          });
        });
        app.phaseIndexByAgent = byAgent;
        return byAgent;
      });
      var cached = request.catch(function (error) {
        if (app.phaseIndexPromise === cached) {
          app.phaseIndexPromise = null;
        }
        throw error;
      });
      app.phaseIndexPromise = cached;
    }
    return app.phaseIndexPromise;
  }

  function populateTeamFilter() {
    var current = app.selectedTeam;
    var all = htmlElement("option", "", "All teams");
    all.value = "";
    var options = [all];
    app.data.teams.slice().sort(compareTeamsByActivity).forEach(function (team) {
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
    var files = app.data.summary_files.filter(function (file) {
      return selectedTeamAllows(file);
    });
    var children = [];
    if (!files.length) {
      children.push(htmlElement("p", "summary-files-empty", "No summary files in this dataset."));
    } else {
      files.forEach(function (file) {
        var button = htmlElement("button", "summary-file-button");
        button.type = "button";
        var team = text(file.team);
        var kindLabel = text(file.kind, "summary");
        if (!app.selectedTeam && team) {
          kindLabel = team + " · " + kindLabel;
        }
        var kind = htmlElement("span", "summary-file-kind", kindLabel);
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
      .filter(function (value) { return value.length > 0; })
      .join(" ")
      .toLocaleLowerCase();
  }

  function selectedTeamAllows(item) {
    if (!app.selectedTeam) {
      return true;
    }
    var itemTeam = text(item && item.team);
    if (itemTeam) {
      return itemTeam === app.selectedTeam;
    }
    var teams = app.data ? array(app.data.teams) : [];
    return teams.length === 1 && text(teams[0] && teams[0].slug) === app.selectedTeam;
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
    if (!agentSummaryAvailable(agent)) {
      parts.push("Lifetime summary: Summary not generated");
    } else if (text(agent.lifetime_summary)) {
      parts.push("Lifetime summary: " + text(agent.lifetime_summary));
    }
    return parts.join(". ");
  }

  function agentLifetimeSummary(agent) {
    if (!agentSummaryAvailable(agent)) {
      return "Summary not generated for this agent lifetime.";
    }
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
      agentSummaryAvailable(agent) ? agent.lifetime_summary : "",
      agent.status
    ]);
  }

  function phaseSearchText(phase) {
    return lowerSearchText([
      phase.id,
      phaseSummaryAvailable(phase) ? phase.phrase : "",
      phaseSummaryAvailable(phase) ? phase.paragraph : ""
    ]);
  }

  function phaseDisplayPhrase(phase) {
    return phaseSummaryAvailable(phase)
      ? text(phase.phrase, "Agent phase")
      : "Activity window";
  }

  function phaseDisplayParagraph(phase) {
    return phaseSummaryAvailable(phase)
      ? text(phase.paragraph, "No paragraph summary available.")
      : "Summary not generated for this activity window. Raw transcript and statistics remain available where recorded.";
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
    var leftTeam = text(left.team);
    var rightTeam = text(right.team);
    if (leftTeam !== rightTeam) {
      var activityDifference = teamActivitySortScore(rightTeam) -
        teamActivitySortScore(leftTeam);
      if (activityDifference) {
        return activityDifference;
      }
    }
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

    app.rowByAgent.clear();
    var packed = null;
    if (!app.perAgentTracks && timelineCore) {
      var lifetimeItems = rows.map(function (row, inputIndex) {
        var agent = row.agent;
        return {
          id: text(agent.id),
          start_ms: number(agent.start_ms, app.data.range.start_ms),
          end_ms: number(agent.end_ms, app.data.range.end_ms),
          official_name: agentOfficialName(agent),
          input_index: inputIndex,
          dedicated: !text(agent.parent_id)
        };
      });
      if (app.renderLod !== "aggregate") {
        lifetimeItems = timelineCore.lifetimesWithin(
          lifetimeItems,
          app.viewStart,
          app.viewEnd
        );
        var visibleIds = new Set(lifetimeItems.map(function (item) {
          return text(item.id);
        }));
        rows = rows.filter(function (row) {
          return visibleIds.has(text(row.agent.id));
        });
      }
      packed = timelineCore.packLifetimes(lifetimeItems);
    }
    app.rows = rows;
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
    }, app.renderLod === "aggregate"
      ? "TEAM ACTIVITY"
      : (app.perAgentTracks ? "AGENT TRACKS" : "PACKED LANES")));
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
    return normalizeKind(value, ["spawn", "continuation", "message", "result"], "other");
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

  function edgePath(x1, y1, x2, y2, kind) {
    var structuralDirection =
      kind === "spawn" || kind === "continuation" ? -1 : kind === "result" ? 1 : 0;
    if (structuralDirection !== 0 && Math.abs(y2 - y1) > 2) {
      var structuralBend = 28;
      var verticalDirection = y2 > y1 ? 1 : -1;
      var approachY = y2 - verticalDirection * 9;
      var outerX = structuralDirection < 0
        ? Math.min(x1, x2) - structuralBend
        : Math.max(x1, x2) + structuralBend;
      return "M " + x1.toFixed(2) + " " + y1.toFixed(2) +
        " C " + outerX.toFixed(2) + " " + y1.toFixed(2) +
        ", " + outerX.toFixed(2) + " " + approachY.toFixed(2) +
        ", " + x2.toFixed(2) + " " + approachY.toFixed(2) +
        " L " + x2.toFixed(2) + " " + y2.toFixed(2);
    }
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
    clearRangeSelectionState();
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

  function zoomToActivityRange(bounds, scope) {
    function finishZoom() {
      var activityRange = timelineCore.activityRangeWithin(
        bounds,
        app.data,
        MIN_VIEW_MS,
        scope
      );
      zoomToRange(
        activityRange ? activityRange.start_ms : bounds.start_ms,
        activityRange ? activityRange.end_ms : bounds.end_ms
      );
    }
    var recordedStart = number(bounds && bounds.activity_start_ms, NaN);
    var recordedEnd = number(bounds && bounds.activity_end_ms, NaN);
    if (app.schemaMode === "schema2" && Number.isFinite(recordedStart) &&
        Number.isFinite(recordedEnd) && recordedEnd > recordedStart) {
      zoomToRange(recordedStart, recordedEnd);
      return;
    }
    if (app.schemaMode !== "schema2") {
      finishZoom();
      return;
    }
    var request = requestDetailShards(
      detailShardsForRange(
        number(bounds.start_ms, app.viewStart),
        number(bounds.end_ms, app.viewEnd),
        0
      )
    );
    request.promise.then(finishZoom).catch(function (error) {
      showDetailLoadError(error);
      finishZoom();
    });
  }

  function phaseContextMenu(event, phase, agent) {
    var agentScope = { agent_id: text(agent.id) };
    showContextMenu(event, text(phase.phrase, "Work phase"), [
      {
        label: "Zoom to work phase",
        run: function () {
          zoomToActivityRange(phase, {
            agent_id: agentScope.agent_id,
            phase_id: text(phase.id)
          });
        }
      },
      {
        label: "Zoom to agent lifetime",
        run: function () { zoomToActivityRange(agent, agentScope); }
      }
    ]);
  }

  function agentContextMenu(event, agent) {
    showContextMenu(event, agentShortName(agent), [
      {
        label: "Zoom to agent lifetime",
        run: function () {
          zoomToActivityRange(agent, { agent_id: text(agent.id) });
        }
      }
    ]);
  }

  function renderEdge(edge, layer, bounds, bufferStart, bufferEnd) {
    var kind = edgeKind(edge.kind);
    if (app.renderLod === "aggregate" ||
        (app.renderLod === "lifetime" && kind !== "spawn" &&
         kind !== "continuation" && kind !== "result")) {
      return;
    }
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
    var rowBuffer = 2;
    var bothEndpointsVisible =
      sourceRow.index >= bounds.first - rowBuffer &&
      sourceRow.index <= bounds.last + rowBuffer &&
      targetRow.index >= bounds.first - rowBuffer &&
      targetRow.index <= bounds.last + rowBuffer;
    if (displayState !== "highlighted" && !bothEndpointsVisible) {
      return;
    }
    var y1 = sourceRow.index * ROW_HEIGHT + ROW_HEIGHT / 2;
    var y2 = targetRow.index * ROW_HEIGHT + ROW_HEIGHT / 2;
    if (Math.max(y1, y2) < bounds.top || Math.min(y1, y2) > bounds.bottom) {
      return;
    }
    var x1 = timeToX(sourceTime);
    var x2 = timeToX(targetTime);
    // At lifetime density even one fork and join per agent becomes a thicket. Preserve
    // structural context for the highlighted family, but leave the unselected overview clean.
    if (app.renderLod === "lifetime" && displayState !== "highlighted") {
      return;
    }
    var pathData = edgePath(x1, y1, x2, y2, kind);
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
    var summaryAvailable = phaseSummaryAvailable(phase);
    var displayPhrase = phaseDisplayPhrase(phase);
    var group = svgElement("g", {
      class: "phase-group" + (summaryAvailable ? "" : " summary-not-generated") +
        selectionClass(agentId, phase),
      tabindex: "0",
      role: "button",
      "data-phase-id": text(phase.id),
      "data-agent-id": agentId,
      "data-start-ms": String(start),
      "data-end-ms": String(end),
      "aria-pressed": app.selection && app.selection.kind === "phase" &&
        text(app.selection.phase_id) === text(phase.id) ? "true" : "false",
      "aria-label": displayPhrase +
        (summaryAvailable ? "" : ". Summary not generated") +
        ". " + agentAccessibleName(agent)
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

    var phrase = truncatePhrase(displayPhrase, width);
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
        displayPhrase + " · " + agentShortName(agent),
        phaseDisplayParagraph(phase) +
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
      class: "agent-label-group" +
        (agentSummaryAvailable(agent) ? "" : " summary-not-generated") +
        selectionClass(text(agent.id), null),
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
    var summaryAvailable = agentSummaryAvailable(agent);
    var group = svgElement("g", {
      class: "agent-lifetime-group" +
        (summaryAvailable ? "" : " summary-not-generated") +
        selectionClass(text(agent.id), null),
      role: "button",
      tabindex: "0",
      "data-agent-id": text(agent.id),
      "data-start-ms": String(start),
      "data-end-ms": String(end),
      "aria-label": "Agent lifetime: " + agentAccessibleName(agent) +
        (summaryAvailable ? "" : ". Summary not generated")
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

  function aggregateTeams() {
    var matchedTeamIds = new Set(app.rows.map(function (row) {
      return text(row.agent.team);
    }));
    var teams = app.data.teams.filter(function (team) {
      var slug = text(team.slug);
      return (!app.selectedTeam || slug === app.selectedTeam) &&
        (!app.query || matchedTeamIds.has(slug));
    });
    if (teams.length) {
      return teams.sort(compareTeamsByActivity);
    }
    if (app.query) {
      return [];
    }
    var seen = new Set();
    return app.data.activity_bins.reduce(function (result, bin) {
      var slug = text(bin.team);
      if (slug && !seen.has(slug) && (!app.selectedTeam || slug === app.selectedTeam)) {
        seen.add(slug);
        result.push({ slug: slug, label: slug });
      }
      return result;
    }, []).sort(compareTeamsByActivity);
  }

  function combinedActivityBins(resolution, teamIndex) {
    var grouped = new Map();
    app.data.activity_bins.forEach(function (bin) {
      var team = text(bin.team);
      var start = number(bin.start_ms, NaN);
      var end = number(bin.end_ms, NaN);
      if (!teamIndex.has(team) || text(bin.resolution) !== resolution ||
          !Number.isFinite(start) || !Number.isFinite(end) || end <= start) {
        return;
      }
      var key = [team, String(start), String(end)].join(":");
      if (!grouped.has(key)) {
        grouped.set(key, {
          team: team,
          resolution: resolution,
          start_ms: start,
          end_ms: end,
          coordinator: null,
          workers: null
        });
      }
      var combined = grouped.get(key);
      if (text(bin.role) === "coordinator") {
        combined.coordinator = bin;
      } else if (text(bin.role) === "workers") {
        combined.workers = bin;
      }
    });
    return Array.from(grouped.values()).sort(function (left, right) {
      return teamIndex.get(left.team) - teamIndex.get(right.team) ||
        left.start_ms - right.start_ms || left.end_ms - right.end_ms;
    });
  }

  function aggregateSelectionKey(bin) {
    return [
      "activity",
      text(bin.team),
      text(bin.resolution),
      String(number(bin.start_ms, 0))
    ].join(":");
  }

  function renderActivityBin(bin, team, rowIndex, layer) {
    var start = number(bin.start_ms, NaN);
    var end = number(bin.end_ms, NaN);
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= app.viewStart ||
        start >= app.viewEnd || end <= start) {
      return;
    }
    var clippedStart = Math.max(start, app.viewStart);
    var clippedEnd = Math.min(end, app.viewEnd);
    var x = timeToX(clippedStart);
    var width = Math.max(1, timeToX(clippedEnd) - x);
    var coordinator = bin.coordinator || {};
    var workers = bin.workers || {};
    var coordinatorCoverage = clamp(
      number(
        coordinator.activity_evidence_fraction,
        number(coordinator.activity_coverage_fraction, 0)
      ),
      0,
      1
    );
    var workerCoverage = clamp(
      number(
        workers.activity_evidence_fraction,
        number(workers.activity_coverage_fraction, 0)
      ),
      0,
      1
    );
    var coverage = Math.max(coordinatorCoverage, workerCoverage);
    var average = Math.max(0, number(
      workers.avg_present_concurrency,
      number(workers.avg_active_concurrency, 0)
    ));
    var peak = Math.max(0, number(
      workers.peak_present_concurrency,
      number(workers.peak_concurrency, 0)
    ));
    var rowTop = rowIndex * AGGREGATE_TEAM_HEIGHT;
    var height = clamp(
      (coordinatorCoverage > 0 ? 10 : 4) +
        Math.log2(1 + average) * AGGREGATE_WORKER_SCALE,
      4,
      AGGREGATE_WORKER_MAX_HEIGHT + 10
    );
    var y = rowTop + 8;
    var key = aggregateSelectionKey(bin);
    var hasSummary = summaryAvailableInRange(text(bin.team), start, end);
    var selected = app.selection && app.selection.kind === "rollup" &&
      text(app.selection.path) === key;
    var group = svgElement("g", {
      class: "activity-bin-group " +
        (hasSummary ? "summary-available" : "summary-unavailable") +
        (selected ? " is-selected" : ""),
      role: "button",
      tabindex: "0",
      "data-team": text(bin.team),
      "data-activity-role": "combined",
      "data-activity-resolution": text(bin.resolution),
      "data-summary-available": hasSummary ? "true" : "false",
      "data-start-ms": String(start),
      "data-end-ms": String(end),
      "aria-pressed": selected ? "true" : "false",
      "aria-label": text(team.label, text(team.slug)) +
        " combined activity. " + formatRange(start, end) +
        (hasSummary ? ". Summary available" : ". No summary generated")
    });
    group.appendChild(svgElement("rect", {
      x: x,
      y: y,
      width: width,
      height: height,
      rx: 2,
      class: "activity-bin-block activity-bin-combined",
      "fill-opacity": (0.16 + coverage * 0.84).toFixed(3)
    }));
    group.addEventListener("pointerenter", function (event) {
      showTooltip(
        event,
        text(team.label, text(team.slug)) + " · Team activity",
        formatRange(start, end) +
          "\nCoordinator activity evidence: " +
            Math.round(coordinatorCoverage * 100) + "%" +
          "\nEstimated average present workers: " + average.toFixed(2) +
          "\nEstimated peak present workers: " + formatCount(peak) +
          "\nAny activity evidence: " + Math.round(coverage * 100) + "%" +
          "\nWorker evidence events: " +
            formatCount(workers.activity_evidence_events) +
          "\nDistinct active workers: " + formatCount(workers.distinct_active_agents) +
          "\nSummary: " + (hasSummary ? "available" : "not generated"),
        text(bin.resolution, "aggregate") +
          " combined mechanical activity bin · inferred timing"
      );
    });
    group.addEventListener("pointermove", positionTooltip);
    group.addEventListener("pointerleave", hideTooltip);
    group.addEventListener("click", function (event) {
      if (event.detail !== 1 || Date.now() < app.suppressClickUntil) {
        return;
      }
      event.stopPropagation();
      setSelection({
        kind: "rollup",
        path: key,
        start_ms: start,
        end_ms: end
      });
    });
    group.addEventListener("dblclick", function (event) {
      event.preventDefault();
      event.stopPropagation();
      zoomToRange(start, end);
    });
    group.addEventListener("contextmenu", function (event) {
      showContextMenu(event, "Mechanical activity bin", [{
        label: "Zoom to activity bin",
        run: function () { zoomToRange(start, end); }
      }]);
    });
    group.addEventListener("keydown", function (event) {
      if (event.key === "Enter") {
        event.preventDefault();
        zoomToRange(start, end);
      } else if (event.key === " ") {
        event.preventDefault();
        setSelection({
          kind: "rollup",
          path: key,
          start_ms: start,
          end_ms: end
        });
      }
    });
    layer.appendChild(group);
  }

  function renderAggregateTracks() {
    var teams = aggregateTeams();
    var totalHeight = Math.max(
      dom.scroll.clientHeight,
      teams.length * AGGREGATE_TEAM_HEIGHT,
      1
    );
    var resolution = timelineCore && typeof timelineCore.aggregateResolution === "function"
      ? timelineCore.aggregateResolution(app.viewStart, app.viewEnd, app.chartWidth)
      : "daily";
    dom.svg.setAttribute("viewBox", "0 0 " + app.width + " " + totalHeight);
    dom.svg.setAttribute("width", String(app.width));
    dom.svg.setAttribute("height", String(totalHeight));
    dom.svg.setAttribute("data-lane-count", String(teams.length));
    dom.svg.setAttribute("data-track-mode", "aggregate");
    dom.svg.setAttribute("data-render-lod", "aggregate");
    dom.svg.setAttribute("data-aggregate-resolution", resolution);
    dom.svg.setAttribute("data-activity-bin-count", "0");
    dom.svg.setAttribute("data-summary-bin-count", "0");
    dom.svg.replaceChildren();
    dom.empty.hidden = teams.length > 0;
    if (!teams.length) {
      return;
    }

    var backgroundLayer = svgElement("g");
    var gridLayer = svgElement("g");
    var contentLayer = svgElement("g");
    var labelLayer = svgElement("g");
    var rangeLayer = svgElement("g", { "aria-hidden": "true" });
    teams.forEach(function (team, index) {
      var y = index * AGGREGATE_TEAM_HEIGHT;
      backgroundLayer.appendChild(svgElement("rect", {
        x: 0,
        y: y,
        width: app.width,
        height: AGGREGATE_TEAM_HEIGHT,
        class: "aggregate-team-row " +
          (index % 2 ? "track-row-odd" : "track-row-even")
      }));
      backgroundLayer.appendChild(svgElement("line", {
        x1: 0,
        y1: y + AGGREGATE_TEAM_HEIGHT,
        x2: app.width,
        y2: y + AGGREGATE_TEAM_HEIGHT,
        class: "track-divider"
      }));
      labelLayer.appendChild(svgElement("text", {
        x: 12,
        y: y + 29,
        class: "aggregate-team-label"
      }, truncateLabel(text(team.label, text(team.slug, "Team")), app.labelWidth - 20, false)));
      labelLayer.appendChild(svgElement("text", {
        x: 12,
        y: y + 45,
        class: "aggregate-team-resolution"
      }, resolution + " activity"));
    });
    app.axisTicks.forEach(function (tick) {
      var x = timeToX(tick);
      gridLayer.appendChild(svgElement("line", {
        x1: x,
        y1: 0,
        x2: x,
        y2: totalHeight,
        class: "grid-line"
      }));
    });
    var teamIndex = new Map();
    teams.forEach(function (team, index) {
      teamIndex.set(text(team.slug), index);
    });
    var renderedBins = 0;
    var summaryBins = 0;
    combinedActivityBins(resolution, teamIndex).forEach(function (bin) {
      var index = teamIndex.get(text(bin.team));
      if (index === undefined) {
        return;
      }
      var before = contentLayer.childElementCount;
      renderActivityBin(bin, teams[index], index, contentLayer);
      if (contentLayer.childElementCount > before) {
        renderedBins += 1;
        if (summaryAvailableInRange(text(bin.team), bin.start_ms, bin.end_ms)) {
          summaryBins += 1;
        }
      }
    });
    dom.svg.setAttribute("data-activity-bin-count", String(renderedBins));
    dom.svg.setAttribute("data-summary-bin-count", String(summaryBins));
    labelLayer.appendChild(svgElement("rect", {
      x: app.labelWidth - 1,
      y: 0,
      width: 1,
      height: totalHeight,
      fill: "#344057"
    }));
    renderRangeSelection(rangeLayer, totalHeight);
    dom.svg.append(backgroundLayer, gridLayer, contentLayer, rangeLayer, labelLayer);
  }

  function renderRangeSelection(layer, height) {
    if (!app.rangeSelection) {
      dom.svg.classList.remove("is-range-selecting");
      dom.svg.removeAttribute("data-range-selection-state");
      return;
    }
    dom.svg.classList.add("is-range-selecting");
    dom.svg.setAttribute("data-range-selection-state", "active");
    var anchor = clamp(
      number(app.rangeSelection.anchor_ms, app.viewStart),
      app.viewStart,
      app.viewEnd
    );
    var cursor = clamp(
      number(app.rangeSelection.cursor_ms, anchor),
      app.viewStart,
      app.viewEnd
    );
    var anchorX = timeToX(anchor);
    var cursorX = timeToX(cursor);
    var left = Math.min(anchorX, cursorX);
    var width = Math.abs(cursorX - anchorX);
    if (width > 0.5) {
      layer.appendChild(svgElement("rect", {
        x: left,
        y: 0,
        width: width,
        height: Math.max(1, height),
        class: "range-selection-window"
      }));
    }
    layer.appendChild(svgElement("line", {
      x1: anchorX,
      y1: 0,
      x2: anchorX,
      y2: Math.max(1, height),
      class: "range-selection-line range-selection-anchor"
    }));
    layer.appendChild(svgElement("line", {
      x1: cursorX,
      y1: 0,
      x2: cursorX,
      y2: Math.max(1, height),
      class: "range-selection-line range-selection-cursor"
    }));
  }

  function renderTracks() {
    if (app.renderLod === "aggregate") {
      renderAggregateTracks();
      return;
    }
    dom.svg.removeAttribute("data-aggregate-resolution");
    dom.svg.removeAttribute("data-activity-bin-count");
    dom.svg.removeAttribute("data-summary-bin-count");
    var totalHeight = Math.max(dom.scroll.clientHeight, app.laneCount * ROW_HEIGHT, 1);
    dom.svg.setAttribute("viewBox", "0 0 " + app.width + " " + totalHeight);
    dom.svg.setAttribute("width", String(app.width));
    dom.svg.setAttribute("height", String(totalHeight));
    dom.svg.setAttribute("data-lane-count", String(app.laneCount));
    dom.svg.setAttribute("data-render-lod", app.renderLod);
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
    var rangeLayer = svgElement("g", {
      "clip-path": "url(#chart-clip)",
      "aria-hidden": "true"
    });
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
    if (app.renderLod !== "aggregate") {
      app.data.edges.forEach(function (edge) {
        if (!app.query || edgeSearchText(edge).indexOf(app.query.toLocaleLowerCase()) >= 0 ||
            app.rowByAgent.has(text(edge.source_id)) || app.rowByAgent.has(text(edge.target_id))) {
          renderEdge(edge, edgeLayer, bounds, bufferStart, bufferEnd);
        }
      });
    }

    var visibleRows = app.rows.filter(function (row) {
      return row.index >= bounds.first && row.index <= bounds.last;
    });
    visibleRows.forEach(function (row) {
      var agent = row.agent;
      renderAgentLifetime(row, lifetimeLayer);
      if (app.renderLod === "detail") {
        (app.phasesByAgent.get(text(agent.id)) || []).forEach(function (phase) {
          renderPhase(phase, row, phaseLayer, bufferStart, bufferEnd);
        });
      }
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
    renderRangeSelection(rangeLayer, totalHeight);
    dom.svg.append(backgroundLayer, gridLayer, contentLayer, rangeLayer, labelLayer);
  }

  function renderRollups() {
    var visibleRollups = app.data.rollups.filter(function (rollup) {
      return selectedTeamAllows(rollup);
    });
    if (!visibleRollups.length) {
      dom.rollupRow.hidden = true;
      dom.rollupTrack.replaceChildren();
      return;
    }
    dom.rollupRow.hidden = false;
    var span = app.viewEnd - app.viewStart;
    var children = [];
    var visibleTeams = Array.from(new Set(visibleRollups.map(function (rollup) {
      return text(rollup.team);
    }).filter(Boolean))).sort();
    app.data.rollups.forEach(function (rollup) {
      var rollupTeam = text(rollup.team);
      if (!selectedTeamAllows(rollup)) {
        return;
      }
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
        ["hourly", "daily", "weekly", "monthly", "quarterly"],
        "daily"
      );
      var selected = app.selection && app.selection.kind === "rollup" &&
        text(app.selection.path) === text(rollup.path);
      var summaryAvailable = rollupSummaryAvailable(rollup);
      var button = htmlElement(
        "button",
        "rollup-marker rollup-" + kind +
          (summaryAvailable ? "" : " summary-not-generated") +
          (selected ? " is-selected" : "")
      );
      button.type = "button";
      button.style.left = left.toFixed(2) + "px";
      button.style.width = width.toFixed(2) + "px";
      if (!app.selectedTeam && visibleTeams.length > 1 && rollupTeam) {
        var teamIndex = visibleTeams.indexOf(rollupTeam);
        var markerHeight = Math.max(4, Math.floor(18 / visibleTeams.length));
        var baseTop = {
          hourly: 3,
          daily: 29,
          weekly: 55,
          monthly: 81,
          quarterly: 107
        }[kind];
        button.style.top = (baseTop + Math.max(0, teamIndex) * markerHeight) + "px";
        button.style.height = markerHeight + "px";
        button.style.lineHeight = Math.max(4, markerHeight - 2) + "px";
        button.style.padding = "0 2px";
      }
      var teamLabel = app.teamBySlug.has(rollupTeam)
        ? text(app.teamBySlug.get(rollupTeam).label, rollupTeam)
        : rollupTeam;
      var markerLabel = text(rollup.label, kind);
      button.textContent = width >= 36 && (!teamLabel || app.selectedTeam)
        ? markerLabel
        : "";
      button.setAttribute(
        "aria-label",
        (teamLabel ? teamLabel + " " : "") + kind + " summary: " +
          text(rollup.label, formatRange(start, end)) +
          (summaryAvailable ? "" : ". Summary not generated")
      );
      button.title = (teamLabel ? teamLabel + " · " : "") + markerLabel +
        (summaryAvailable ? "" : " · Summary not generated");
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
          hourly: "hour",
          daily: "day",
          weekly: "week",
          monthly: "month",
          quarterly: "quarter"
        }[kind] || "summary range";
        showContextMenu(event, text(rollup.label, kind + " summary"), [
          {
            label: "Zoom to " + rangeName,
            run: function () {
              zoomToActivityRange(rollup, null);
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

  function detailCoverageComplete(start, end) {
    if (app.schemaMode !== "schema2") {
      return true;
    }
    return app.shardCatalog.every(function (shard) {
      var overlaps = number(shard.start_ms, Infinity) < end &&
        number(shard.end_ms, -Infinity) > start;
      return !overlaps || app.loadedShardUrls.has(
        immutableTimelineObjectUrl(shard, "detail shard")
      );
    });
  }

  function fullRangeAggregateStats() {
    function usableEventStats(value) {
      if (!value || typeof value !== "object") {
        return null;
      }
      var fields = [
        "user_prompts",
        "agent_responses",
        "inter_agent_messages",
        "external_messages",
        "tool_calls"
      ];
      return fields.every(function (field) {
        return Object.prototype.hasOwnProperty.call(value, field) &&
          Number.isFinite(Number(value[field]));
      }) ? value : null;
    }
    if (app.query || app.viewStart > app.data.range.start_ms ||
        app.viewEnd < app.data.range.end_ms) {
      return null;
    }
    if (!app.selectedTeam) {
      return usableEventStats(app.data.stats);
    }
    var team = app.teamBySlug.get(app.selectedTeam);
    return usableEventStats(team && team.stats);
  }

  function countVisibleStats() {
    var visibleIds = new Set(app.rows.map(function (row) {
      return text(row.agent.id);
    }));
    var eventCountsAvailable = detailCoverageComplete(app.viewStart, app.viewEnd);
    var aggregateStats = eventCountsAvailable ? null : fullRangeAggregateStats();
    var hasAggregateStats = aggregateStats !== null;
    var result = {
      user_prompts: eventCountsAvailable ? 0 :
        (hasAggregateStats ? number(aggregateStats.user_prompts, 0) : null),
      agent_responses: eventCountsAvailable ? 0 :
        (hasAggregateStats ? number(aggregateStats.agent_responses, 0) : null),
      inter_agent_messages: eventCountsAvailable ? 0 :
        (hasAggregateStats ? number(aggregateStats.inter_agent_messages, 0) : null),
      external_messages: eventCountsAvailable ? 0 :
        (hasAggregateStats ? number(aggregateStats.external_messages, 0) : null),
      tool_calls: eventCountsAvailable ? 0 :
        (hasAggregateStats ? number(aggregateStats.tool_calls, 0) : null),
      active_agents: 0,
      event_counts_available: eventCountsAvailable || hasAggregateStats
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
        at < app.viewEnd &&
        eventBelongsToVisibleAgent(event, visibleIds);
    });
    if (eventCountsAvailable) {
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
        } else if (kind === "external_message" || kind === "external_messages") {
          result.external_messages += 1;
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
    var nodes = [
      statNode(stats.user_prompts, "User prompts"),
      statNode(stats.agent_responses, "Agent responses"),
      statNode(stats.inter_agent_messages, "Inter-agent msgs"),
    ];
    if (Number(stats.external_messages) > 0) {
      nodes.push(statNode(stats.external_messages, "External msgs"));
    }
    nodes.push(
      statNode(stats.tool_calls, "Tool calls"),
      statNode(stats.active_agents, "Active agents")
    );
    dom.statsValues.replaceChildren.apply(dom.statsValues, nodes);
  }

  function render() {
    app.renderQueued = false;
    if (!app.data) {
      return;
    }
    var renderStarted = window.performance && typeof window.performance.now === "function"
      ? window.performance.now()
      : Date.now();
    configureTrackMode();
    measure();
    app.renderLod = timelineCore && typeof timelineCore.semanticZoomLevel === "function"
      ? timelineCore.semanticZoomLevel(app.viewStart, app.viewEnd, app.chartWidth)
      : "detail";
    if (app.renderLod === "aggregate") {
      dom.card.style.setProperty("--label-width", AGGREGATE_LABEL_WIDTH + "px");
      measure();
      app.renderLod = timelineCore && typeof timelineCore.semanticZoomLevel === "function"
        ? timelineCore.semanticZoomLevel(app.viewStart, app.viewEnd, app.chartWidth)
        : "aggregate";
    }
    requestVisibleDetails();
    dom.card.classList.toggle("aggregate-mode", app.renderLod === "aggregate");
    buildRows();
    dom.card.dataset.viewStartMs = String(app.viewStart);
    dom.card.dataset.viewEndMs = String(app.viewEnd);
    dom.card.dataset.trackMode = app.renderLod === "aggregate"
      ? "aggregate"
      : (app.perAgentTracks ? "per-agent" : "packed");
    if (app.selection) {
      dom.card.dataset.selectionScope = text(app.selection.kind);
    } else {
      dom.card.dataset.selectionScope = "none";
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
    var renderFinished = window.performance && typeof window.performance.now === "function"
      ? window.performance.now()
      : Date.now();
    app.renderRevision += 1;
    dom.card.dataset.renderLod = app.renderLod;
    dom.card.dataset.renderRevision = String(app.renderRevision);
    dom.card.dataset.renderDurationMs = Math.max(0, renderFinished - renderStarted).toFixed(2);
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
    clearRangeSelectionState();
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

  function zoomAround(ratio, scale) {
    var oldSpan = app.viewEnd - app.viewStart;
    var fullSpan = app.navigationRange.end_ms - app.navigationRange.start_ms;
    var newSpan = clamp(
      oldSpan * scale,
      Math.min(MIN_VIEW_MS, fullSpan),
      fullSpan
    );
    var anchorRatio = clamp(number(ratio, 0.5), 0, 1);
    var anchor = app.viewStart + anchorRatio * oldSpan;
    var start = anchor - anchorRatio * newSpan;
    setView(start, start + newSpan);
  }

  function wheelDeltaPixels(event) {
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
      return event.deltaY * 16;
    }
    if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
      return event.deltaY * Math.max(120, dom.scroll.clientHeight);
    }
    return event.deltaY;
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
    // Wheel hardware reports pixels, lines, or whole pages. Normalize first and cap one
    // browser event to a modest step so a single coarse notch cannot jump several zoom levels.
    var delta = clamp(wheelDeltaPixels(event), -80, 80);
    zoomAround(ratio, Math.exp(delta * 0.0015));
  }

  function eventTimeOnChart(event) {
    var rect = dom.svg.getBoundingClientRect();
    var x = clamp(event.clientX - rect.left, app.labelWidth, app.width);
    var ratio = clamp((x - app.labelWidth) / Math.max(1, app.chartWidth), 0, 1);
    return app.viewStart + ratio * (app.viewEnd - app.viewStart);
  }

  function clearRangeSelectionState() {
    app.rangeSelection = null;
    dom.svg.classList.remove("is-range-selecting");
    dom.svg.removeAttribute("data-range-selection-state");
  }

  function cancelRangeSelection() {
    if (!app.rangeSelection) {
      return;
    }
    clearRangeSelectionState();
    scheduleRender();
  }

  function updateRangeSelection(event) {
    if (!app.rangeSelection) {
      return;
    }
    app.rangeSelection.cursor_ms = eventTimeOnChart(event);
    scheduleRender();
  }

  function handleEmptyTrackClick(event) {
    if (Date.now() < app.suppressClickUntil) {
      return;
    }
    var target = event.target;
    if (!(target instanceof Element) ||
        (!target.classList.contains("track-row") &&
         !target.classList.contains("aggregate-team-row"))) {
      return;
    }
    if (event.clientX < dom.svg.getBoundingClientRect().left + app.labelWidth) {
      return;
    }
    if (app.rangeSelection) {
      var start = Math.min(
        number(app.rangeSelection.anchor_ms, app.viewStart),
        eventTimeOnChart(event)
      );
      var end = Math.max(
        number(app.rangeSelection.anchor_ms, app.viewStart),
        eventTimeOnChart(event)
      );
      clearRangeSelectionState();
      zoomToRange(start, Math.max(start + MIN_VIEW_MS, end));
      return;
    }
    if (event.detail !== 1) {
      return;
    }
    if (app.selection) {
      setSelection(null);
    }
    var at = eventTimeOnChart(event);
    app.rangeSelection = { anchor_ms: at, cursor_ms: at };
    dom.svg.classList.add("is-range-selecting");
    dom.svg.setAttribute("data-range-selection-state", "active");
    scheduleRender();
  }

  function cancelRangeSelectionOnContextMenu(event) {
    if (!app.rangeSelection) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    cancelRangeSelection();
  }

  function beginPan(event) {
    if (!app.data || event.button !== 0 || app.rangeSelection) {
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
      ["external_messages", "external messages"],
      ["tool_calls", "tool calls"]
    ];
    values.forEach(function (entry) {
      if (entry[0] === "external_messages" && !(Number(stats && stats[entry[0]]) > 0)) {
        return;
      }
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
          credentials: "same-origin"
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

  async function renderArtifacts(container, owner, fallback, context) {
    var association = artifactAssociation(owner, fallback);
    container.dataset.artifactCount = String(association.all.length);
    if (association.all.length &&
        (app.artifactCatalogState === "unloaded" ||
         app.artifactCatalogState === "loading")) {
      showLoading(container, "Loading artifact links…");
      await ensureArtifactCatalog(app.data);
      if (!container.isConnected) {
        return;
      }
      container.replaceChildren();
    }
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
        return renderArtifacts(container, owner, fallback, context);
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

  function renderSummaryNotGenerated(container, scope) {
    var suffix = scope ? " for this " + scope : "";
    container.appendChild(htmlElement(
      "div",
      "summary-not-generated-notice",
      "Summary not generated" + suffix + ". Raw transcript, statistics, and artifacts remain available where recorded."
    ));
  }

  function phaseFallbackDetail(phase) {
    return {
      phrase: phaseDisplayPhrase(phase),
      paragraph: phaseSummaryAvailable(phase) ? text(phase.paragraph) : "",
      stats: phase.stats || {},
      work_summary: [],
      transcript: [],
      raw_summary_path: "",
      summary_available: phaseSummaryAvailable(phase)
    };
  }

  function renderAgentLifetimeOverview(container, agent, indexedPhases) {
    var phases = indexedPhases || app.phasesByAgent.get(text(agent.id)) || [];
    container.appendChild(
      htmlElement("h3", "agent-lifetime-section-title", "Work phases (" +
        formatCount(phases.length) + ")")
    );
    if (!phases.length) {
      container.appendChild(htmlElement(
        "div",
        "empty-message",
        "No work phases were recorded for this agent."
      ));
      return;
    }
    var list = htmlElement("div", "agent-lifetime-phase-list");
    phases.forEach(function (phase) {
      var summaryAvailable = phaseSummaryAvailable(phase);
      var button = htmlElement(
        "button",
        "agent-lifetime-phase" + (summaryAvailable ? "" : " summary-not-generated")
      );
      button.type = "button";
      button.dataset.phaseId = text(phase.id);
      button.append(
        htmlElement(
          "time",
          "agent-lifetime-phase-time",
          formatRange(number(phase.start_ms, 0), number(phase.end_ms, 0))
        ),
        htmlElement("strong", "agent-lifetime-phase-title", phaseDisplayPhrase(phase)),
        htmlElement("span", "agent-lifetime-phase-summary", phaseDisplayParagraph(phase))
      );
      button.addEventListener("click", function () {
        openPhaseModal(phase, agent);
      });
      list.appendChild(button);
    });
    container.appendChild(list);
  }

  async function renderAgentLifetimePhases(container, agent, request) {
    if (app.schemaMode === "schema2") {
      if (app.phaseIndexReference) {
        if (!app.phaseIndexPromise) {
          showLoading(container, "Loading agent work phases…");
        }
        var index = await loadPhaseIndex();
        if (request !== app.detailRequest || dom.modalBackdrop.hidden ||
            !container.isConnected) {
          return;
        }
        container.replaceChildren();
        renderAgentLifetimeOverview(
          container,
          agent,
          (index && index.get(text(agent.id))) || []
        );
        return;
      }
      var start = number(agent.start_ms, app.data.range.start_ms);
      var end = number(agent.end_ms, app.data.range.end_ms);
      var shards = app.shardCatalog.filter(function (shard) {
        return number(shard.start_ms, Infinity) < end &&
          number(shard.end_ms, -Infinity) > start;
      });
      showLoading(container, "Loading agent work phases…");
      await requestDetailShards(shards).promise;
      if (request !== app.detailRequest || dom.modalBackdrop.hidden ||
          !container.isConnected) {
        return;
      }
      container.replaceChildren();
    }
    renderAgentLifetimeOverview(container, agent);
  }

  function openAgentLifetimeModal(agent) {
    var request = ++app.detailRequest;
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
          return renderAgentLifetimePhases(container, agent, request);
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
    var summaryAvailable = phaseDetailSummaryAvailable(detail, phase);
    var phrase = summaryAvailable
      ? text(detail.phrase, text(phase.phrase, "Agent phase"))
      : "Activity window";
    var paragraph = summaryAvailable
      ? text(detail.paragraph, text(phase.paragraph))
      : "Summary not generated for this activity window.";
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
    var tabs = [];
    if (summaryAvailable) {
      tabs.push({
        label: "Agent Work Summary",
        render: function (container) {
          renderWorkSummary(container, detail.work_summary);
        }
      });
    }
    tabs.push({
      label: "Full Transcript",
      render: function (container) {
        if (!summaryAvailable) {
          renderSummaryNotGenerated(container, "activity window");
        }
        renderTranscript(container, detail.transcript, transcriptRoles);
      }
    });
    if (summaryAvailable) {
      tabs.push({
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
      });
    }
    var artifacts = artifactTab(detail, phase, {
      start_ms: phase.start_ms,
      end_ms: phase.end_ms,
      agent_id: phase.agent_id
    });
    if (artifacts) {
      tabs.splice(summaryAvailable ? tabs.length - 1 : tabs.length, 0, artifacts);
    }
    activateTabs(tabs, 0);
  }

  async function openPhaseModal(phase, agent) {
    var request = ++app.detailRequest;
    openModalBase(
      agentShortName(agent) + " · phase",
      phaseDisplayPhrase(phase),
      phaseDisplayParagraph(phase),
      phase.stats || {}
    );
    dom.modalEyebrow.title = agentAccessibleName(agent);
    showModalAgentIdentity(agent);
    showLoading(dom.modalContent, "Loading phase details…");
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
      "Project glossary" + (text(entry.team) ? " · " + text(entry.team) : "") +
        " · " + text(entry.week, "term"),
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
        "## Definition\n\n" +
          text(entry.definition, "No model-backed definition is available.") +
          "\n\n## First-use evidence\n\n> " +
          text(entry.context, "No source context was retained."),
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
      if (!selectedTeamAllows(entry)) {
        return;
      }
      var entryLabel = text(entry.term);
      if (!app.selectedTeam && text(entry.team)) {
        entryLabel += " · " + text(entry.team);
      }
      var link = htmlElement("a", "glossary-term-link", entryLabel);
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
      (text(rollup.team) ? text(rollup.team) + " · " : "") +
        kind + " rollup · " + formatRange(start, end),
      text(rollup.label, kind + " summary"),
      rollupSummaryAvailable(rollup) ? "" : "Summary not generated for this time range.",
      rollup.stats || {}
    );
    var technicalAvailable = rollupAudienceAvailable(
      rollup,
      "technical_summary_available"
    );
    var plainAvailable = rollupAudienceAvailable(
      rollup,
      "plain_language_summary_available"
    );
    var technicalPath = technicalAvailable
      ? text(rollup.technical_path, text(rollup.path))
      : "";
    var plainPath = plainAvailable ? text(rollup.plain_language_path) : "";
    var tabs = [];
    if (technicalPath) {
      tabs.push({
        label: "Technical",
        render: function (container) {
          return renderRawSummary(container, technicalPath, "");
        }
      });
    }
    if (plainPath) {
      tabs.push({
        label: "Plain Language",
        render: function (container) {
          return renderRawSummary(container, plainPath, "");
        }
      });
    }
    if (!tabs.length) {
      tabs.push({
        label: "Summary",
        render: function (container) {
          renderSummaryNotGenerated(container, kind + " time range");
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

  function fetchJsonCached(url) {
    if (!app.resourcePromises.has(url)) {
      var request = (async function () {
        var response = await fetch(url, { credentials: "same-origin" });
        if (!response.ok) {
          var error = new Error("HTTP " + response.status + " " + response.statusText);
          error.httpStatus = response.status;
          throw error;
        }
        return response.json();
      }());
      var cached = request.catch(function (error) {
        if (app.resourcePromises.get(url) === cached) {
          app.resourcePromises.delete(url);
        }
        throw error;
      });
      app.resourcePromises.set(url, cached);
    }
    return app.resourcePromises.get(url);
  }

  async function loadSchema2() {
    var bootstrap;
    try {
      bootstrap = await fetchJsonCached(SCHEMA_2_URL);
    } catch (error) {
      if (error && error.httpStatus === 404) {
        return false;
      }
      throw error;
    }
    if (!bootstrap || typeof bootstrap !== "object" ||
        number(bootstrap.schema_version, NaN) !== 2 ||
        text(bootstrap.kind) !== "timeline-bootstrap") {
      throw new Error("Unsupported schema-2 timeline bootstrap.");
    }
    var globalUrl = immutableTimelineObjectUrl(bootstrap.global, "timeline global");
    if (bootstrap.phase_index) {
      immutableTimelineObjectUrl(bootstrap.phase_index, "timeline phase index");
    }
    var globalData = await fetchJsonCached(globalUrl);
    if (!globalData || typeof globalData !== "object" ||
        number(globalData.schema_version, NaN) !== 2 ||
        text(globalData.kind) !== "timeline-global") {
      throw new Error("Unsupported schema-2 timeline global object.");
    }
    var catalog = array(bootstrap.detail_shards).slice();
    var seenUrls = new Set();
    catalog.forEach(function (shard) {
      var url = immutableTimelineObjectUrl(shard, "detail shard");
      var start = number(shard.start_ms, NaN);
      var end = number(shard.end_ms, NaN);
      if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start ||
          seenUrls.has(url)) {
        throw new Error("Invalid or duplicate schema-2 detail shard catalog entry.");
      }
      seenUrls.add(url);
    });
    catalog.sort(function (left, right) {
      return number(left.start_ms, 0) - number(right.start_ms, 0);
    });
    app.schemaMode = "schema2";
    app.shardCatalog = catalog;
    app.detailPromises.clear();
    app.loadedShardUrls.clear();
    app.searchShardState = catalog.length ? "unloaded" : "ready";
    app.phaseIndexReference = bootstrap.phase_index || null;
    app.phaseIndexPromise = null;
    app.phaseIndexByAgent.clear();
    var merged = Object.assign({}, globalData, {
      schema_version: 2,
      generated_at: bootstrap.generated_at,
      source_digest: bootstrap.source_digest,
      display_timezone: bootstrap.display_timezone,
      display_timezone_source: bootstrap.display_timezone_source,
      range: bootstrap.range,
      teams: bootstrap.teams,
      activity_bins: bootstrap.activity_bins,
      phases: [],
      events: []
    });
    initializeData(merged);
    return true;
  }

  async function loadSchema1() {
    app.schemaMode = "schema1";
    app.shardCatalog = [];
    app.detailPromises.clear();
    app.loadedShardUrls.clear();
    app.searchShardState = "legacy";
    app.phaseIndexReference = null;
    app.phaseIndexPromise = null;
    app.phaseIndexByAgent.clear();
    initializeData(await fetchJsonCached(DATA_URL));
  }

  async function loadTimeline() {
    try {
      var schema2Error = null;
      try {
        if (await loadSchema2()) {
          return;
        }
      } catch (error) {
        schema2Error = error;
      }
      await loadSchema1();
      if (schema2Error) {
        var message = schema2Error instanceof Error
          ? schema2Error.message
          : String(schema2Error);
        dom.meta.textContent += " · schema 2 fallback (" + message + ")";
        dom.card.dataset.timelineSchemaMode = "schema1-fallback";
      }
    } catch (error) {
      showLoadError(error);
    }
  }

  dom.teamFilter.addEventListener("change", function () {
    app.selectedTeam = dom.teamFilter.value;
    dom.scroll.scrollTop = 0;
    populateSummaryFiles();
    scheduleRender();
  });

  dom.search.addEventListener("input", function () {
    app.query = dom.search.value.trim();
    dom.scroll.scrollTop = 0;
    if (app.query && app.schemaMode === "schema2" &&
        app.searchShardState !== "ready" && app.searchShardState !== "loading") {
      requestSearchCorpus();
    }
    scheduleRender();
  });

  dom.fit.addEventListener("click", fitTimeline);
  dom.zoomOut.addEventListener("click", function () {
    cancelRangeSelection();
    zoomAround(0.5, 1 / 0.72);
  });
  dom.zoomIn.addEventListener("click", function () {
    cancelRangeSelection();
    zoomAround(0.5, 0.72);
  });
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
  dom.svg.addEventListener("pointermove", function (event) {
    updateRangeSelection(event);
    continuePan(event);
  });
  dom.svg.addEventListener("pointerup", endPan);
  dom.svg.addEventListener("pointercancel", endPan);
  dom.svg.addEventListener("click", handleEmptyTrackClick);
  dom.svg.addEventListener("contextmenu", cancelRangeSelectionOnContextMenu);
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
      if (app.rangeSelection) {
        cancelRangeSelection();
      } else if (!dom.laneMenu.hidden) {
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
