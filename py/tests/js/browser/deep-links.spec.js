"use strict";

const { test, expect } = require("@playwright/test");
const { BASE_MS, DATA_END_MS, DATA_START_MS, TIMELINE } = require("./fixture-data.cjs");

const teamSlug = TIMELINE.teams[0].slug;
const projectLabel = TIMELINE.teams[0].projects.find(function (project) {
  return project.primary;
}).label;
const minute = 60 * 1000;

async function readView(timeline) {
  return {
    start: Number(await timeline.getAttribute("data-view-start-ms")),
    end: Number(await timeline.getAttribute("data-view-end-ms"))
  };
}

test("deep links restore window and team, push deliberate jumps, and Back returns", async function ({
  page
}) {
  const timeline = page.locator('[data-testid="timeline"]');
  const linkStart = BASE_MS;
  const linkEnd = BASE_MS + 60 * minute;
  const deepLink =
    "/#view=day&t=2026-03-08&start=" + linkStart + "&end=" + linkEnd +
    "&team=" + encodeURIComponent(teamSlug) +
    "&project=" + encodeURIComponent(projectLabel);

  await page.goto(deepLink);
  await expect(page.locator("#dataset-meta")).not.toContainText("Loading timeline");
  await expect(timeline).toHaveAttribute("data-view-start-ms", String(linkStart));
  await expect(timeline).toHaveAttribute("data-view-end-ms", String(linkEnd));
  await expect(page.locator('[data-testid="team-filter"]')).toHaveValue(teamSlug);
  expect(await page.evaluate(function () { return location.hash; })).toBe(
    deepLink.slice(1),
    "restoring a link must not rewrite it"
  );
  const loadedHistory = await page.evaluate(function () { return history.length; });

  // Fit is a deliberate jump: it pushes, so Back and Forward move between
  // the linked window and the fitted one.
  await page.locator('[data-testid="fit"]').click();
  await expect(timeline).toHaveAttribute("data-view-start-ms", String(DATA_START_MS));
  await expect(timeline).toHaveAttribute("data-view-end-ms", String(DATA_END_MS));
  await expect.poll(async function () {
    return page.evaluate(function () { return location.hash; });
  }).toContain("start=" + DATA_START_MS);
  expect(await page.evaluate(function () { return history.length; }))
    .toBe(loadedHistory + 1);

  await page.goBack();
  await expect(timeline).toHaveAttribute("data-view-start-ms", String(linkStart));
  await expect(timeline).toHaveAttribute("data-view-end-ms", String(linkEnd));
  await expect(page.locator('[data-testid="team-filter"]')).toHaveValue(teamSlug);
  await page.goForward();
  await expect(timeline).toHaveAttribute("data-view-start-ms", String(DATA_START_MS));

  // Zoom buttons step like the wheel: the address tracks the new window in
  // place instead of stacking a history entry per step.
  const beforeZoom = await readView(timeline);
  const zoomHistory = await page.evaluate(function () { return history.length; });
  await page.locator('[data-testid="zoom-in"]').click();
  await expect.poll(async function () {
    const current = await readView(timeline);
    return current.end - current.start;
  }).toBeLessThan(beforeZoom.end - beforeZoom.start);
  const zoomed = await readView(timeline);
  await expect.poll(async function () {
    return page.evaluate(function () { return location.hash; });
  }).toContain("start=" + zoomed.start);
  expect(await page.evaluate(function () { return history.length; })).toBe(zoomHistory);

  // Team selection is deliberate too, and Back restores the team it left.
  await page.locator('[data-testid="team-filter"]').selectOption("");
  await expect.poll(async function () {
    return page.evaluate(function () { return location.hash; });
  }).not.toContain("team=");
  expect(await page.evaluate(function () { return history.length; }))
    .toBe(zoomHistory + 1);
  await page.goBack();
  await expect(page.locator('[data-testid="team-filter"]')).toHaveValue(teamSlug);
  await expect(timeline).toHaveAttribute("data-view-start-ms", String(zoomed.start));

  // A hand-written calendar link lands on that archive-timezone day, and a
  // glossary hash still opens the glossary without resetting the timeline.
  await page.goto("/#view=day&t=2026-03-09");
  await expect(timeline).toHaveAttribute(
    "data-view-start-ms",
    String(Date.UTC(2026, 2, 9, 4))
  );
  await expect(timeline).toHaveAttribute(
    "data-view-end-ms",
    String(Date.UTC(2026, 2, 10, 4))
  );
  const dayView = await readView(timeline);
  await page.goto("/#glossary/term-malformed-input-123456789abc");
  await expect(page.getByTestId("modal")).toBeVisible();
  await expect(page.locator("#modal-title")).toHaveText("malformed-input");
  expect(await readView(timeline)).toEqual(dayView);
});
