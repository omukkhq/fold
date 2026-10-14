import { describe, expect, it } from "vitest";

const sources = import.meta.glob<string>("/src/live/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("live layer", () => {
  it("scans its sources", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(5);
  });

  it("never polls", () => {
    const offenders = Object.entries(sources)
      .filter(([path]) => !path.endsWith(".test.ts"))
      .filter(([, source]) => /refetchInterval|setInterval/.test(source))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
