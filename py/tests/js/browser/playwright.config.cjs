"use strict";

const path = require("path");
const { defineConfig } = require("@playwright/test");

// No fixed port (#1252): derive the port from this checkout's path, so
// parallel gates in different checkouts never collide on one fixed port,
// while every process that loads this config in one run agrees on it.
// The derived port is a preference, not a promise (#1687): when an
// unrelated program already holds it, the picker falls back to an ephemeral
// port instead of failing every check at bind time. The picked port is
// written back to AGENT_TIMELINE_E2E_PORT so spawned processes that load
// this config agree on it. Pin AGENT_TIMELINE_E2E_PORT to choose a port.
const { derivedPort, pickPort } = require("./port-picker.cjs");

const port = pickPort({
  pinned: process.env.AGENT_TIMELINE_E2E_PORT,
  preferred: derivedPort(__dirname),
});
process.env.AGENT_TIMELINE_E2E_PORT = String(port);
const baseURL = "http://127.0.0.1:" + port;

module.exports = defineConfig({
  testDir: __dirname,
  testMatch: "**/*.spec.js",
  fullyParallel: false,
  workers: 1,
  timeout: 15_000,
  expect: { timeout: 5_000 },
  reporter: process.env.CI ? "line" : "list",
  outputDir: path.join(__dirname, "test-results"),
  use: {
    baseURL: baseURL,
    browserName: "chromium",
    headless: true,
    viewport: { width: 1280, height: 820 },
    trace: "retain-on-failure"
  },
  webServer: {
    command: "node fixture-server.cjs --port " + port,
    cwd: __dirname,
    url: baseURL + "/__health",
    reuseExistingServer: false,
    timeout: 10_000
  }
});

