"use strict";

const { test, expect } = require("@playwright/test");
const {
  AGENT_COUNT,
  PHASE_A_START_MS,
  PHASE_A_END_MS
} = require("./fixture-data.cjs");

const phaseSelector = '[data-phase-id="phase-a-1"]';
const secondPhaseSelector = '[data-phase-id="phase-a-2"]';

async function requireContract(locator, reason) {
  test.skip((await locator.count()) === 0, reason);
  return locator.first();
}

async function readView(timeline) {
  const start = Number(await timeline.getAttribute("data-view-start-ms"));
  const end = Number(await timeline.getAttribute("data-view-end-ms"));
  expect(Number.isFinite(start)).toBeTruthy();
  expect(Number.isFinite(end)).toBeTruthy();
  expect(end).toBeGreaterThan(start);
  return { start: start, end: end, span: end - start };
}

async function waitForViewChange(timeline, previousStart, previousEnd) {
  await expect.poll(async function () {
    const current = await readView(timeline);
    return current.start !== previousStart || current.end !== previousEnd;
  }).toBeTruthy();
  return readView(timeline);
}

test.beforeEach(async function ({ page }) {
  await page.goto("/");
  await expect(page.locator("#dataset-meta")).not.toContainText("Loading timeline");
  await expect(page.locator("#load-error")).toBeHidden();
  await expect(page.locator(phaseSelector)).toBeVisible();
});

test("a horizontal trackpad gesture pans the zoomed timeline", async function ({ page }) {
  const timeline = await requireContract(
    page.locator('[data-testid="timeline"][data-view-start-ms][data-view-end-ms]'),
    "view-range data attributes are not implemented yet"
  );
  const axis = page.locator("#time-axis");
  const axisBox = await axis.boundingBox();
  expect(axisBox).not.toBeNull();
  await page.mouse.move(
    axisBox.x + axisBox.width * 0.7,
    axisBox.y + axisBox.height / 2
  );
  const full = await readView(timeline);
  await page.mouse.wheel(0, -480);
  const zoomed = await waitForViewChange(timeline, full.start, full.end);
  expect(zoomed.span).toBeLessThan(full.span);

  const tracks = page.locator("#timeline-svg");
  const tracksBox = await tracks.boundingBox();
  expect(tracksBox).not.toBeNull();
  await page.mouse.move(
    tracksBox.x + tracksBox.width * 0.75,
    tracksBox.y + Math.min(100, tracksBox.height / 2)
  );
  await page.mouse.wheel(260, 0);
  const panned = await waitForViewChange(timeline, zoomed.start, zoomed.end);
  expect(Math.abs(panned.span - zoomed.span)).toBeLessThan(zoomed.span * 0.02);
});

test("single clicks select and a double click opens phase detail", async function ({ page }) {
  const timeline = await requireContract(
    page.locator('[data-testid="timeline"][data-selection-scope]'),
    "selection state data attributes are not implemented yet"
  );
  const phase = page.locator(phaseSelector);
  const modal = page.getByTestId("modal");

  await phase.click();
  await expect(modal).toBeHidden();
  await expect(timeline).toHaveAttribute("data-selection-scope", "agent");
  await expect(timeline).toHaveAttribute("data-selected-agent-id", "agent-a");

  await page.waitForTimeout(600);
  await phase.click();
  await expect(modal).toBeHidden();
  await expect(timeline).toHaveAttribute("data-selection-scope", "phase");
  await expect(timeline).toHaveAttribute("data-selected-phase-id", "phase-a-1");

  await page.waitForTimeout(600);
  await phase.dblclick();
  await expect(modal).toBeVisible();
  await expect(page.locator("#modal-title")).toContainText("Audit parser invariants");
});

test("different work phases select directly while the same phase toggles scope", async function ({ page }) {
  const timeline = page.getByTestId("timeline");
  const firstPhase = page.locator(phaseSelector);
  const secondPhase = page.locator(secondPhaseSelector);

  await firstPhase.click();
  await page.waitForTimeout(600);
  await firstPhase.click();
  await expect(timeline).toHaveAttribute("data-selected-phase-id", "phase-a-1");

  await page.waitForTimeout(600);
  await secondPhase.click();
  await expect(timeline).toHaveAttribute("data-selection-scope", "phase");
  await expect(timeline).toHaveAttribute("data-selected-phase-id", "phase-a-2");

  await page.waitForTimeout(600);
  await secondPhase.click();
  await expect(timeline).toHaveAttribute("data-selection-scope", "agent");
  await expect(timeline).not.toHaveAttribute("data-selected-phase-id");
});

test("packed lane labels list their agents and empty background clears selection", async function ({ page }) {
  const timeline = page.getByTestId("timeline");
  const lane = page.locator('[data-lane-index="1"]');
  await expect(lane).toHaveAttribute("aria-label", /2 named agents/);
  await lane.click();

  const menu = page.getByTestId("lane-agent-menu");
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: /Parser audit/ })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: /Documentation audit/ })).toBeVisible();
  await menu.getByRole("menuitem", { name: /Documentation audit/ }).click();
  await expect(menu).toBeHidden();
  await expect(timeline).toHaveAttribute("data-selected-agent-id", "agent-c");

  const svg = page.locator("#timeline-svg");
  const box = await svg.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.click(box.x + box.width - 12, box.y + 54 + 27);
  await expect(timeline).not.toHaveAttribute("data-selection-scope");

  await lane.click();
  await expect(menu).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(lane).toHaveAttribute("aria-expanded", "false");
});

test("agent lifetime hover shows the hindsight summary and tail-truncated official path", async function ({ page }) {
  const lifetime = page.locator('.agent-lifetime-group[data-agent-id="agent-a"]');
  await expect(lifetime).toHaveAttribute(
    "aria-label",
    /\/root\/transcript_auditor\/owner_turn_miner\/plugin_layout_audit\/parser_boundary_regression_audit/
  );
  await lifetime.hover({ position: { x: 4, y: 4 } });
  const tooltip = page.getByTestId("tooltip");
  await expect(tooltip).toBeVisible();
  await expect(page.locator("#tooltip-title")).toHaveText("Parser audit");
  await expect(page.locator("#tooltip-body")).not.toContainText("Short name:");
  await expect(page.locator("#tooltip-body")).toContainText("Official: …");
  await expect(page.locator("#tooltip-body")).toContainText("parser_boundary_regression_audit");
  await expect(page.locator("#tooltip-body")).toContainText("Found the malformed-input parser boundary");
});

test("the phase context menu can zoom exactly to the work phase", async function ({ page }) {
  const timeline = await requireContract(
    page.locator('[data-testid="timeline"][data-view-start-ms][data-view-end-ms]'),
    "view-range data attributes are not implemented yet"
  );
  const menu = await requireContract(
    page.locator('[data-testid="timeline-context-menu"]'),
    "the timeline context-menu test id is not implemented yet"
  );

  await page.locator(phaseSelector).click({ button: "right" });
  await expect(menu).toBeVisible();
  const zoomPhase = await requireContract(
    menu.getByRole("menuitem", { name: /zoom to (work )?phase/i }),
    "the Zoom to phase menu action is not implemented yet"
  );
  await zoomPhase.click();
  await expect(menu).toBeHidden();

  await expect.poll(async function () {
    const view = await readView(timeline);
    return Math.abs(view.start - PHASE_A_START_MS) <= 1 &&
      Math.abs(view.end - PHASE_A_END_MS) <= 1;
  }).toBeTruthy();
});

test("packed tracks are the default and per-agent tracks remain available", async function ({ page }) {
  const svg = await requireContract(
    page.locator("#timeline-svg[data-track-mode][data-lane-count]"),
    "track-mode and lane-count data attributes are not implemented yet"
  );
  const toggle = page.locator("#per-agent-tracks");

  await expect(toggle).not.toBeChecked();
  await expect(svg).toHaveAttribute("data-track-mode", "packed");
  const packedCount = Number(await svg.getAttribute("data-lane-count"));
  expect(packedCount).toBeGreaterThan(0);
  expect(packedCount).toBeLessThan(AGENT_COUNT);

  await toggle.check();
  await expect(svg).toHaveAttribute("data-track-mode", "per-agent");
  await expect(svg).toHaveAttribute("data-lane-count", String(AGENT_COUNT));
});

test("full-transcript role filters support user-only, none, and all", async function ({ page }) {
  await page.locator(phaseSelector).dblclick();
  await expect(page.getByTestId("modal")).toBeVisible();
  await page.getByRole("tab", { name: "Full Transcript" }).click();
  const filters = await requireContract(
    page.locator('[data-testid="transcript-role-filters"]'),
    "transcript role filters are not implemented yet"
  );
  const entries = await requireContract(
    page.locator(".transcript-entry[data-role]"),
    "transcript entries do not expose normalized data-role values yet"
  );
  expect(await entries.count()).toBe(5);
  const pullRequest = page.locator('.pr-reference[href="https://github.com/rrnewton/dev-hermit/pull/38"]');
  await expect(pullRequest).toHaveAttribute("title", "Repair malformed-input handling");

  await filters.getByRole("button", { name: /user only/i }).click();
  await expect(page.locator('.transcript-entry[data-role="user"]:visible')).toHaveCount(1);
  await expect(page.locator('.transcript-entry:not([data-role="user"]):visible')).toHaveCount(0);

  await filters.getByRole("button", { name: /select none/i }).click();
  await expect(page.locator(".transcript-entry[data-role]:visible")).toHaveCount(0);

  await filters.getByRole("button", { name: /select all/i }).click();
  await expect(page.locator(".transcript-entry[data-role]:visible")).toHaveCount(5);
});

test("rollups switch audiences and verified glossary terms open stable entries", async function ({ page }) {
  const daily = page.locator(".rollup-marker.rollup-daily").first();
  await daily.dblclick();
  await expect(page.getByTestId("modal")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Technical" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Plain Language" })).toBeVisible();
  await expect(page.locator("#modal-content")).toContainText("technical summary");

  await page.getByRole("tab", { name: "Plain Language" }).click();
  await expect(page.locator("#modal-content")).toContainText("plain-language summary");
  const term = page.locator(
    '.glossary-term-link[data-glossary-id="term-malformed-input-123456789abc"]'
  ).first();
  await expect(term).toHaveAttribute(
    "href",
    "#glossary/term-malformed-input-123456789abc"
  );
  await term.click();
  await expect(page.locator("#modal-title")).toHaveText("malformed-input");
  await expect(page).toHaveURL(/#glossary\/term-malformed-input-123456789abc$/);
  await expect(page.locator("#modal-content")).toContainText("required structure");

  await page.locator("#modal-close").click();
  await page.getByTestId("glossary-open").click();
  await expect(page.locator("#modal-title")).toHaveText("Project glossary");
  await expect(page.locator("#modal-content")).toContainText("Data that does not satisfy");
});
