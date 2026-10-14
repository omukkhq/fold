import { describe, expect, it } from "vitest";
import {
  boardSearchSchema,
  cleanSearch,
  isFiltered,
  toggleValue,
} from "./search";

describe("boardSearchSchema", () => {
  it("reads every filter", () => {
    expect(
      boardSearchSchema.parse({
        agent: "frontend",
        label: "ui",
        blocked: true,
        needs: true,
        q: "login",
      }),
    ).toEqual({
      agent: "frontend",
      label: "ui",
      blocked: true,
      needs: true,
      q: "login",
    });
  });

  it("leaves an unfiltered board empty", () => {
    expect(boardSearchSchema.parse({})).toEqual({});
  });

  it("drops a malformed value instead of failing", () => {
    const parsed = boardSearchSchema.parse({
      agent: 3,
      label: "",
      blocked: "yes",
      needs: false,
      q: "x".repeat(201),
      other: "kept out",
    });
    expect(parsed).toEqual({
      agent: undefined,
      label: undefined,
      blocked: undefined,
      needs: undefined,
      q: undefined,
    });
  });

  it("trims text values", () => {
    expect(boardSearchSchema.parse({ label: "  ui " }).label).toBe("ui");
  });
});

describe("cleanSearch", () => {
  it("removes unset filters so the URL stays clean", () => {
    expect(
      cleanSearch({
        agent: undefined,
        label: "",
        blocked: undefined,
        needs: undefined,
        q: "  ",
      }),
    ).toEqual({});
  });

  it("keeps set filters and trims the query", () => {
    expect(
      cleanSearch({
        agent: "frontend",
        label: "ui",
        blocked: true,
        needs: true,
        q: " login ",
      }),
    ).toEqual({
      agent: "frontend",
      label: "ui",
      blocked: true,
      needs: true,
      q: "login",
    });
  });
});

describe("toggleValue", () => {
  it("chooses a new value and clears the chosen one", () => {
    expect(toggleValue(undefined, "frontend")).toBe("frontend");
    expect(toggleValue("backend", "frontend")).toBe("frontend");
    expect(toggleValue("frontend", "frontend")).toBeUndefined();
  });
});

describe("isFiltered", () => {
  it("is true only with a set filter", () => {
    expect(isFiltered({})).toBe(false);
    expect(isFiltered({ q: " " })).toBe(false);
    expect(isFiltered({ blocked: true })).toBe(true);
  });
});
