"use strict";

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const appPath = path.resolve(__dirname, "../../agent_team_timeline/static/app.js");
const source = fs.readFileSync(appPath, "utf8");

function functionSource(name) {
  const marker = "  function " + name + "(";
  const start = source.indexOf(marker);
  assert.notStrictEqual(start, -1, "missing function " + name);
  const bodyStart = source.indexOf("{", start);
  let depth = 0;
  let quote = "";
  let escaped = false;
  for (let index = bodyStart; index < source.length; index += 1) {
    const character = source[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = "";
      }
      continue;
    }
    if (character === "\"" || character === "'" || character === "`") {
      quote = character;
    } else if (character === "{") {
      depth += 1;
    } else if (character === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(start, index + 1);
      }
    }
  }
  throw new Error("unterminated function " + name);
}

const context = {
  app: {
    selectedTeam: "",
    data: {
      teams: [
        { slug: "claude-coord-176" },
        { slug: "codex-coord-030" }
      ]
    },
    glossaryById: new Map()
  }
};
vm.createContext(context);
vm.runInContext(
  ["array", "text", "selectedTeamAllows", "glossaryTermIsAmbiguous"]
    .map(functionSource).join("\n"),
  context
);

assert.strictEqual(context.selectedTeamAllows({ team: "codex-coord-030" }), true);
context.app.selectedTeam = "claude-coord-176";
assert.strictEqual(context.selectedTeamAllows({ team: "claude-coord-176" }), true);
assert.strictEqual(context.selectedTeamAllows({ team: "codex-coord-030" }), false);
assert.strictEqual(context.selectedTeamAllows({}), false);
context.app.data.teams = [{ slug: "claude-coord-176" }];
assert.strictEqual(context.selectedTeamAllows({}), true);
context.app.selectedTeam = "";
context.app.glossaryById = new Map([
  ["term-codex-parser", { term: "parser", team: "codex-coord-030" }],
  ["term-claude-parser", { term: "parser", team: "claude-coord-176" }]
]);
assert.strictEqual(context.glossaryTermIsAmbiguous("parser"), true);
assert.strictEqual(context.glossaryTermIsAmbiguous("scheduler"), false);
context.app.selectedTeam = "codex-coord-030";
assert.strictEqual(context.glossaryTermIsAmbiguous("parser"), false);

assert.match(source, /item\["team"\]|item\.team/);
assert.match(source, /visibleTeams\.indexOf\(rollupTeam\)/);
assert.match(source, /\["hourly", "daily", "weekly", "monthly", "quarterly"\]/);
assert.match(source, /hourly: "hour"/);
assert.match(source, /populateSummaryFiles\(\);\s*scheduleRender\(\);/);

console.log("multi-team UI tests passed");
