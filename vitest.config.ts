import { defineConfig } from "vitest/config";

// Pure-logic tests only (formatting, scheduling, geometry, mock data); no DOM needed.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
