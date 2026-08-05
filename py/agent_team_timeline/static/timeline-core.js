(function (root, factory) {
  "use strict";

  var api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  }
  root.AgentTimelineCore = api;
})(typeof globalThis === "object" ? globalThis : this, function () {
  "use strict";

  function finite(value, fallback) {
    return Number.isFinite(value) ? Number(value) : fallback;
  }

  function string(value) {
    return typeof value === "string" ? value : "";
  }

  function compareLifetime(left, right) {
    return finite(left.start_ms, 0) - finite(right.start_ms, 0) ||
      finite(left.end_ms, finite(left.start_ms, 0)) -
        finite(right.end_ms, finite(right.start_ms, 0)) ||
      string(left.official_name).localeCompare(string(right.official_name), undefined, {
        numeric: true
      }) ||
      string(left.id).localeCompare(string(right.id), undefined, { numeric: true }) ||
      finite(left.input_index, 0) - finite(right.input_index, 0);
  }

  /**
   * Greedily pack half-open agent lifetimes into the first free lane.
   * Coordinators stay in dedicated leading lanes; subagents never share those lanes.
   */
  function packLifetimes(rawItems) {
    var items = Array.isArray(rawItems) ? rawItems.slice() : [];
    var laneById = Object.create(null);
    var dedicated = items.filter(function (item) { return Boolean(item.dedicated); });
    var packed = items.filter(function (item) { return !item.dedicated; });

    dedicated.forEach(function (item, index) {
      laneById[string(item.id)] = index;
    });

    var laneEnds = [];
    packed.sort(compareLifetime).forEach(function (item) {
      var start = finite(item.start_ms, 0);
      var end = Math.max(start, finite(item.end_ms, start));
      var lane = 0;
      while (lane < laneEnds.length && laneEnds[lane] > start) {
        lane += 1;
      }
      if (lane === laneEnds.length) {
        laneEnds.push(end);
      } else {
        laneEnds[lane] = end;
      }
      laneById[string(item.id)] = dedicated.length + lane;
    });

    return {
      lane_by_id: laneById,
      lane_count: dedicated.length + laneEnds.length
    };
  }

  /** First click selects an agent; a later single click on the same phase toggles granularity. */
  function nextPhaseSelection(current, agentId, phaseId, startMs, endMs) {
    var agent = string(agentId);
    var phase = string(phaseId);
    if (current && current.kind === "agent" && current.agent_id === agent) {
      return {
        kind: "phase",
        agent_id: agent,
        phase_id: phase,
        start_ms: finite(startMs, 0),
        end_ms: finite(endMs, finite(startMs, 0))
      };
    }
    if (current && current.kind === "phase" &&
        current.agent_id === agent && current.phase_id === phase) {
      return { kind: "agent", agent_id: agent };
    }
    return { kind: "agent", agent_id: agent };
  }

  function edgeTouchesSelection(edge, selection) {
    if (!selection) {
      return false;
    }
    if (selection.kind === "edge") {
      return string(edge.id) === string(selection.edge_id);
    }
    if (selection.kind === "rollup") {
      var rollupStart = finite(selection.start_ms, -Infinity);
      var rollupEnd = finite(selection.end_ms, Infinity);
      var sourceAt = finite(edge.source_ms, NaN);
      var targetAt = finite(edge.target_ms, NaN);
      return (Number.isFinite(sourceAt) && sourceAt >= rollupStart && sourceAt <= rollupEnd) ||
        (Number.isFinite(targetAt) && targetAt >= rollupStart && targetAt <= rollupEnd);
    }
    var agentId = string(selection.agent_id);
    var sourceMatches = string(edge.source_id) === agentId;
    var targetMatches = string(edge.target_id) === agentId;
    if (!sourceMatches && !targetMatches) {
      return false;
    }
    if (selection.kind !== "phase") {
      return true;
    }
    var start = finite(selection.start_ms, -Infinity);
    var end = finite(selection.end_ms, Infinity);
    var agentAt = sourceMatches
      ? finite(edge.source_ms, NaN)
      : finite(edge.target_ms, NaN);
    return Number.isFinite(agentAt) && agentAt >= start && agentAt <= end;
  }

  /** Return hidden/normal/dimmed/highlighted for one structural or detailed edge. */
  function edgeDisplayState(edge, selection, showGlobalDetailed, showHighlightedDetailed) {
    var kind = string(edge.kind).toLowerCase();
    var structural = kind === "spawn";
    var highlighted = edgeTouchesSelection(edge, selection);
    if (structural) {
      if (!selection) {
        return "normal";
      }
      return highlighted ? "highlighted" : "dimmed";
    }
    if (highlighted && showHighlightedDetailed) {
      return "highlighted";
    }
    if (showGlobalDetailed) {
      return selection ? "dimmed" : "normal";
    }
    return "hidden";
  }

  return {
    edgeDisplayState: edgeDisplayState,
    edgeTouchesSelection: edgeTouchesSelection,
    nextPhaseSelection: nextPhaseSelection,
    packLifetimes: packLifetimes
  };
});
