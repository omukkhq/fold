import { describe, expect, it } from "vitest";
import { readSources } from "../test/sources";

const sources = {
  ...readSources(import.meta.url, "live", ["ts", "tsx"], false),
  ...readSources(import.meta.url, "features", ["ts", "tsx"]),
};

/** The display clock reads the time only; every other timer in the live layer or a screen would be polling. */
const DISPLAY_CLOCK = "/src/live/useNow.ts";

function pollingOffenders(files: Record<string, string>): string[] {
  return Object.entries(files)
    .filter(([path]) => !/\.test\.tsx?$/.test(path) && path !== DISPLAY_CLOCK)
    .filter(([, source]) => /refetchInterval|setInterval/.test(source))
    .map(([path]) => path);
}

describe("live layer and screens", () => {
  it("scans its sources, the display clock among them", () => {
    const paths = Object.keys(sources);
    expect(paths.length).toBeGreaterThan(5);
    expect(paths.some((path) => path.startsWith("/src/features/"))).toBe(true);
    expect(paths).toContain(DISPLAY_CLOCK);
  });

  it("never polls", () => {
    expect(pollingOffenders(sources)).toEqual([]);
  });

  it("allows a timer only in the display clock", () => {
    const timer = "setInterval(() => {}, 1000);";
    expect(
      pollingOffenders({
        [DISPLAY_CLOCK]: timer,
        "/src/live/clock.ts": timer,
        "/src/features/board/useTick.ts": "refetchInterval: 5000",
      }),
    ).toEqual(["/src/live/clock.ts", "/src/features/board/useTick.ts"]);
  });
});
