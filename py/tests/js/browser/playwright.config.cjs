"use strict";

const path = require("path");
const { defineConfig } = require("@playwright/test");

// No fixed port (#1252): derive the port from this checkout's path, so
// parallel gates in different checkouts never collide on one fixed port,
// while every process that loads this config in one run agrees on it.
// Pin AGENT_TIMELINE_E2E_PORT to choose a specific port.
const crypto = require("crypto");

function derivedPort() {
  const digest = crypto.createHash("sha256").update(__dirname).digest();
  return 21000 + (digest.readUInt16BE(0) % 18000);
}

const port = Number(process.env.AGENT_TIMELINE_E2E_PORT || derivedPort());
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

