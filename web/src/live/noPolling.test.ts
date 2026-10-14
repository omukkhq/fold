import { describe, expect, it } from "vitest";

const sources = import.meta.glob<string>(
  ["/src/live/*.{ts,tsx}", "/src/features/**/*.{ts,tsx}"],
  {
    query: "?raw",
    import: "default",
    eager: true,
  },
);

describe("live layer and screens", () => {
  it("scans its sources", () => {
    const paths = Object.keys(sources);
    expect(paths.length).toBeGreaterThan(5);
    expect(paths.some((path) => path.startsWith("/src/features/"))).toBe(true);
  });

  it("never polls", () => {
    const offenders = Object.entries(sources)
      .filter(([path]) => !/\.test\.tsx?$/.test(path))
      .filter(([, source]) => /refetchInterval|setInterval/.test(source))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
