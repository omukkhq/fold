import { describe, expect, it } from "vitest";
import { importsFixtures } from "../lib/fixtureGuard";
import { readSources } from "../test/sources";

const sources = readSources(import.meta.url, "", ["ts", "tsx"]);

describe("fixture boundary", () => {
  it("scans the sources", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(10);
  });

  it("keeps every fixtures/ import out of every source, tests included", () => {
    const offenders = Object.entries(sources)
      .filter(
        ([path, source]) =>
          !path.endsWith("/lib/fixtureGuard.test.ts") &&
          importsFixtures(source),
      )
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
