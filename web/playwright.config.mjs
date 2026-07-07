export default {
  testDir: "./tests",
  timeout: 60_000,
  globalSetup: "./tests/global-setup.mjs",
  use: { headless: true },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
};
