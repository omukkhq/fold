import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  test: {
    environment: "node",
    // Checks run beside parallel builds on one machine; every wait ends on a condition, so a longer cap hides no sleep.
    testTimeout: 15_000,
    setupFiles: ["./src/vitest.setup.ts"],
    css: true,
    include: ["src/**/*.test.{ts,tsx}"],
    passWithNoTests: false,
    reporters: [
      "default",
      {
        onCoverage(coverage) {
          // The v8 provider reports an empty map as "Unknown%", which passes every threshold.
          if (JSON.stringify(coverage) === "{}") {
            throw new Error("coverage measured no file");
          }
        },
      },
    ],
    coverage: {
      provider: "v8",
      include: ["src/**/*.ts"],
      exclude: ["**/*.d.ts", "**/*.test.ts", "src/routeTree.gen.ts"],
      thresholds: { lines: 85, branches: 85 },
    },
  },
});
