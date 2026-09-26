import { defineConfig } from "vitest/config";

// Pure-logic tests only (formatting, scheduling, geometry, mock data); no DOM needed.
// Browser tests live in e2e/ (Playwright, *.spec.ts) and never run here.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    coverage: {
      provider: "v8",
      include: ["src/lib/**/*.ts"],
      exclude: ["src/**/*.test.ts", "src/lib/types.ts"],
      // Under /target/ so it is git-ignored and never scanned by the privacy grep.
      reportsDirectory: "target/coverage",
      reporter: ["text-summary", "text", "html", "lcov", "json-summary"],
      // Just below today's numbers: a drop fails the run, new tests can raise them.
      thresholds: {
        statements: 90,
        branches: 87,
        functions: 82,
        lines: 91,
      },
    },
  },
});
