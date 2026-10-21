import { describe, expect, it } from "vitest";
import { readSources } from "../../test/sources";

const sources = readSources(import.meta.url, "", ["ts", "tsx"]);

/** An `attention` list written out, or an item pushed onto one. A type that names a kind builds nothing. */
export const ATTENTION_BUILT =
  /attention\s*[:=]\s*\[\s*\{|attention(?:\?)?\.push\(/;

describe("attention", () => {
  it("is never built in the web application: only the daemon's list and the tests name a kind", () => {
    const offenders = Object.entries(sources)
      .filter(
        ([path, source]) =>
          !/\.test\.tsx?$/.test(path) &&
          !path.startsWith("/src/test/") &&
          !path.endsWith("/api/schema.d.ts") &&
          ATTENTION_BUILT.test(source),
      )
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });

  it("scans the sources", () => {
    expect(Object.keys(sources).length).toBeGreaterThan(10);
  });
});

describe("the derivation scan", () => {
  it("flags a list written out and a pushed item", () => {
    expect(
      ATTENTION_BUILT.test(
        'card.attention = [{ kind: "stalled", closable: false }]',
      ),
    ).toBe(true);
    expect(
      ATTENTION_BUILT.test('attention: [{ kind: "question", closable: true }]'),
    ).toBe(true);
    expect(
      ATTENTION_BUILT.test(
        'attention.push({ kind: "stalled", closable: false })',
      ),
    ).toBe(true);
    expect(ATTENTION_BUILT.test('kind: "stalled" | "stale_claim";')).toBe(
      false,
    );
  });
});
