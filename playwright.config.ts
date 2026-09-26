import { defineConfig, devices } from "@playwright/test";

// Browser tests against the mock backend (src/lib/mock.ts): the UI served by Vite in plain
// Chromium, no Tauri. Two projects:
//  - e2e:    behaviour, keyboard, reduced motion and accessibility (e2e/*.spec.ts)
//  - visual: screenshot baselines (e2e/visual/*.spec.ts, images in e2e/__screenshots__)
// Screenshots depend on the OS font stack, so baselines carry the platform in their name
// (…-win32.png); run `npm run test:visual` on the same OS that made them.

const PORT = 1421;
const CI = !!process.env.CI;

export default defineConfig({
  testDir: "e2e",
  // Under /target/ (git-ignored), so the privacy grep and `git status` never see run output.
  outputDir: "target/playwright/test-results",
  snapshotPathTemplate: "e2e/__screenshots__/{arg}-{platform}{ext}",
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  // Few workers: each is a Chromium plus the shared Vite server (a 14 GB dev machine runs out).
  workers: CI ? 2 : 3,
  reporter: CI
    ? [["list"], ["html", { outputFolder: "target/playwright/report", open: "never" }], ["github"]]
    : [["list"], ["html", { outputFolder: "target/playwright/report", open: "never" }]],
  expect: {
    toHaveScreenshot: { maxDiffPixelRatio: 0.002, animations: "disabled", caret: "hide", scale: "css" },
  },
  use: {
    baseURL: `http://localhost:${PORT}`,
    locale: "en-US",
    timezoneId: "UTC",
    colorScheme: "dark",
    viewport: { width: 640, height: 640 },
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "e2e",
      testMatch: /e2e[\\/][^\\/]+\.spec\.ts$/,
      use: { ...devices["Desktop Chrome"], viewport: { width: 640, height: 640 }, deviceScaleFactor: 1 },
    },
    {
      name: "visual",
      testMatch: /e2e[\\/]visual[\\/].+\.spec\.ts$/,
      retries: 0,
      use: { ...devices["Desktop Chrome"], viewport: { width: 640, height: 640 }, deviceScaleFactor: 1 },
    },
  ],
  webServer: {
    command: `npx vite --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !CI,
    timeout: 60_000,
  },
});
