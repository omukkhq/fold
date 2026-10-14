import { describe, expect, it } from "vitest";
import { importsFixtures, mayImportFixtures } from "../lib/fixtureGuard";

const sources = import.meta.glob<string>("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("fixture boundary", () => {
  it("scans the sources", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(10);
  });

  it("keeps fixtures out of components and routes", () => {
    const offenders = Object.entries(sources)
      .filter(
        ([path, source]) => !mayImportFixtures(path) && importsFixtures(source),
      )
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
