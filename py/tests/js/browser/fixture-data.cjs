"use strict";

const BASE_MS = Date.UTC(2026, 7, 5, 16, 0, 0);
const minute = 60 * 1000;

function stats(overrides) {
  return Object.assign({
    user_prompts: 1,
    agent_responses: 2,
    inter_agent_messages: 1,
    tool_calls: 1
  }, overrides || {});
}

function state(kind, startMinute, endMinute) {
  return {
    kind: kind,
    start_ms: BASE_MS + startMinute * minute,
    end_ms: BASE_MS + endMinute * minute
  };
}

const agents = [
  {
    id: "coordinator",
    team: "codex-hermit",
    parent_id: "",
    depth: 0,
    path: "/root",
    official_name: "/root",
    official_leaf: "root",
    short_name: "Hermit coordinator",
    nickname: "Coordinator",
    start_ms: BASE_MS,
    end_ms: BASE_MS + 100 * minute,
    status: "complete"
  },
  {
    id: "agent-a",
    team: "codex-hermit",
    parent_id: "coordinator",
    depth: 1,
    path: "/root/parser_audit",
    official_name: "/root/parser_audit",
    official_leaf: "parser_audit",
    short_name: "Parser audit",
    nickname: "Ada",
    start_ms: BASE_MS + 5 * minute,
    end_ms: BASE_MS + 30 * minute,
    status: "complete"
  },
  {
    id: "agent-b",
    team: "codex-hermit",
    parent_id: "coordinator",
    depth: 1,
    path: "/root/ci_audit",
    official_name: "/root/ci_audit",
    official_leaf: "ci_audit",
    short_name: "CI audit",
    nickname: "Turing",
    start_ms: BASE_MS + 10 * minute,
    end_ms: BASE_MS + 40 * minute,
    status: "complete"
  },
  {
    id: "agent-c",
    team: "codex-hermit",
    parent_id: "coordinator",
    depth: 1,
    path: "/root/docs_audit",
    official_name: "/root/docs_audit",
    official_leaf: "docs_audit",
    short_name: "Documentation audit",
    nickname: "Curie",
    start_ms: BASE_MS + 30 * minute,
    end_ms: BASE_MS + 50 * minute,
    status: "complete"
  }
];

const phases = [
  {
    id: "phase-root",
    agent_id: "coordinator",
    start_ms: BASE_MS,
    end_ms: BASE_MS + 100 * minute,
    phrase: "Coordinate Hermit work",
    paragraph: "Spawned focused audits and integrated their findings.",
    detail_path: "details/phase-root.json",
    stats: stats({ user_prompts: 2, agent_responses: 4 }),
    states: [state("active", 0, 100)]
  },
  {
    id: "phase-a-1",
    agent_id: "agent-a",
    start_ms: BASE_MS + 10 * minute,
    end_ms: BASE_MS + 25 * minute,
    phrase: "Audit parser invariants",
    paragraph: "Found and tested the parser boundary that caused the regression.",
    detail_path: "details/phase-a-1.json",
    stats: stats({ tool_calls: 3 }),
    states: [
      state("active", 10, 15),
      state("tool", 15, 20),
      state("active", 20, 25)
    ]
  },
  {
    id: "phase-a-2",
    agent_id: "agent-a",
    start_ms: BASE_MS + 26 * minute,
    end_ms: BASE_MS + 29 * minute,
    phrase: "Verify parser fix",
    paragraph: "Confirmed the repaired parser behavior against the focused regression case.",
    detail_path: "details/phase-a-2.json",
    stats: stats({ tool_calls: 1 }),
    states: [state("active", 26, 29)]
  },
  {
    id: "phase-b-1",
    agent_id: "agent-b",
    start_ms: BASE_MS + 12 * minute,
    end_ms: BASE_MS + 35 * minute,
    phrase: "Verify CI coverage",
    paragraph: "Checked that the regression test runs in the required CI jobs.",
    detail_path: "details/phase-b-1.json",
    stats: stats(),
    states: [state("active", 12, 35)]
  },
  {
    id: "phase-c-1",
    agent_id: "agent-c",
    start_ms: BASE_MS + 32 * minute,
    end_ms: BASE_MS + 48 * minute,
    phrase: "Clarify operator guide",
    paragraph: "Updated the explanation of the repaired parser behavior.",
    detail_path: "details/phase-c-1.json",
    stats: stats(),
    states: [state("active", 32, 48)]
  }
];

const timeline = {
  schema_version: 1,
  generated_at: "2026-08-05T17:40:00Z",
  source_digest: "playwright-fixture",
  display_timezone: "America/New_York",
  range: {
    start_ms: BASE_MS,
    end_ms: BASE_MS + 100 * minute
  },
  teams: [{ slug: "codex-hermit", label: "Codex Hermit" }],
  agents: agents,
  phases: phases,
  edges: [
    {
      id: "spawn-a",
      kind: "spawn",
      source_id: "coordinator",
      target_id: "agent-a",
      source_ms: BASE_MS + 5 * minute,
      target_ms: BASE_MS + 5 * minute,
      phrase: "Audit the parser regression",
      paragraph: "The coordinator assigned the parser investigation to Parser audit.",
      full_text: "Audit the parser regression and return concrete test evidence."
    },
    {
      id: "message-a",
      kind: "message",
      source_id: "coordinator",
      target_id: "agent-a",
      source_ms: BASE_MS + 17 * minute,
      target_ms: BASE_MS + 17 * minute,
      phrase: "Check the error boundary",
      paragraph: "The coordinator narrowed the investigation to malformed input.",
      full_text: "Please check the malformed-input error boundary too."
    },
    {
      id: "result-a",
      kind: "result",
      source_id: "agent-a",
      target_id: "coordinator",
      source_ms: BASE_MS + 25 * minute,
      target_ms: BASE_MS + 25 * minute,
      phrase: "Parser invariant verified",
      paragraph: "Parser audit returned a fix and regression-test evidence.",
      full_text: "The parser invariant now holds and all focused tests pass."
    }
  ],
  events: [
    { agent_id: "coordinator", at_ms: BASE_MS + minute, kind: "user_prompt" },
    { agent_id: "agent-a", at_ms: BASE_MS + 16 * minute, kind: "tool_call" },
    { agent_id: "agent-a", at_ms: BASE_MS + 24 * minute, kind: "agent_response" }
  ],
  rollups: [
    {
      kind: "daily",
      label: "Aug 05",
      start_ms: BASE_MS,
      end_ms: BASE_MS + 60 * minute,
      path: "summaries/daily/2026-08-05.md",
      technical_path: "summaries/daily/2026-08-05.md",
      plain_language_path: "summaries/daily/2026-08-05-plain-language.md"
    },
    {
      kind: "weekly",
      label: "2026-W32",
      start_ms: BASE_MS,
      end_ms: BASE_MS + 100 * minute,
      path: "summaries/weekly/2026-W32.md",
      technical_path: "summaries/weekly/2026-W32.md",
      plain_language_path: "summaries/weekly/2026-W32-plain-language.md"
    }
  ],
  glossary: [
    {
      id: "term-malformed-input-123456789abc",
      term: "malformed-input",
      introduced_at_ms: BASE_MS + minute,
      occurrences: 4,
      context: "Malformed input is data that does not satisfy the parser's required structure.",
      week: "2026-W32",
      url: "#glossary/term-malformed-input-123456789abc"
    }
  ],
  glossary_path: "summaries/glossary/codex-hermit-glossary.md",
  summary_files: [
    {
      kind: "daily",
      period: "2026-08-05",
      label: "Wednesday, August 5",
      path: "summaries/daily/2026-08-05.md"
    },
    {
      kind: "weekly",
      period: "2026-W32",
      label: "Week 32",
      path: "summaries/weekly/2026-W32.md"
    }
  ]
};

const pullUrl = "https://github.com/rrnewton/dev-hermit/pull/38";
const pullText = "I will trace the malformed-input path for " + pullUrl + ".";

const phaseADetail = {
  phrase: "Audit parser invariants",
  paragraph: "Found and tested the parser boundary that caused the regression.",
  stats: stats({ tool_calls: 3 }),
  work_summary: [
    {
      at_ms: BASE_MS + 12 * minute,
      text: "Reproduced the malformed-input failure."
    },
    {
      at_ms: BASE_MS + 24 * minute,
      text: "Added the boundary check and verified focused tests."
    }
  ],
  transcript: [
    {
      role: "user",
      at_ms: BASE_MS + 10 * minute,
      text: "Audit the parser invariant and preserve the original terminology."
    },
    {
      role: "assistant",
      at_ms: BASE_MS + 11 * minute,
      text: pullText,
      pull_requests: [{
        start: pullText.indexOf(pullUrl),
        end: pullText.indexOf(pullUrl) + pullUrl.length,
        text: pullUrl,
        kind: "explicit_url",
        repository: "rrnewton/dev-hermit",
        number: 38,
        url: pullUrl,
        title: "Repair malformed-input handling"
      }]
    },
    {
      role: "tool",
      at_ms: BASE_MS + 16 * minute,
      text: "",
      tools: [{ name: "bash", count: 2 }, { name: "git", count: 1 }]
    },
    {
      role: "agent",
      at_ms: BASE_MS + 20 * minute,
      text: "The coordinator clarified the expected error boundary."
    },
    {
      role: "system",
      at_ms: BASE_MS + 24 * minute,
      text: "Focused tests completed successfully."
    }
  ],
  raw_summary_path: "summaries/phases/phase-a-1.md"
};

function simpleDetail(phase) {
  return {
    phrase: phase.phrase,
    paragraph: phase.paragraph,
    stats: phase.stats,
    work_summary: [],
    transcript: [],
    raw_summary_path: ""
  };
}

const virtualFiles = new Map([
  ["/data/timeline.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(timeline)
  }],
  ["/details/phase-root.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(simpleDetail(phases[0]))
  }],
  ["/details/phase-a-1.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(phaseADetail)
  }],
  ["/details/phase-a-2.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(simpleDetail(phases[2]))
  }],
  ["/details/phase-b-1.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(simpleDetail(phases[3]))
  }],
  ["/details/phase-c-1.json", {
    contentType: "application/json; charset=utf-8",
    body: JSON.stringify(simpleDetail(phases[4]))
  }],
  ["/summaries/phases/phase-a-1.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# Parser audit\n\nThe malformed-input boundary is now covered.\n"
  }],
  ["/summaries/daily/2026-08-05.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# August 5 technical summary\n\nThe team repaired the malformed-input parser boundary.\n"
  }],
  ["/summaries/daily/2026-08-05-plain-language.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# August 5 plain-language summary\n\nThe parser now safely rejects malformed-input records.\n"
  }],
  ["/summaries/weekly/2026-W32.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# Week 32\n\nParser and CI work advanced together.\n"
  }],
  ["/summaries/weekly/2026-W32-plain-language.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# Week 32 in plain language\n\nThe team made invalid data safer to handle.\n"
  }],
  ["/summaries/glossary/codex-hermit-glossary.md", {
    contentType: "text/markdown; charset=utf-8",
    body: "# Project glossary\n\n## malformed-input\n\nData that does not satisfy the parser structure.\n"
  }]
]);

module.exports = {
  BASE_MS: BASE_MS,
  PHASE_A_START_MS: BASE_MS + 10 * minute,
  PHASE_A_END_MS: BASE_MS + 25 * minute,
  AGENT_COUNT: agents.length,
  virtualFiles: virtualFiles
};
