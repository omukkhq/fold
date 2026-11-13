import { describe, expect, it } from "vitest";
import {
  boardSearchSchema,
  cleanSearch,
  isFiltered,
  toggleName,
} from "./search";

describe("boardSearchSchema", () => {
  it("reads every filter, with several labels and owners", () => {
    expect(
      boardSearchSchema.parse({
        label: ["ui", "data"],
        owner: ["human", "area:web"],
        blocked: true,
        needs: true,
        q: "login",
      }),
    ).toEqual({
      label: ["ui", "data"],
      owner: ["human", "area:web"],
      blocked: true,
      needs: true,
      q: "login",
    });
  });

  it("leaves an unfiltered board empty", () => {
    expect(boardSearchSchema.parse({})).toEqual({});
  });

  it("reads one bare value as a list of one and leaves out an agent param", () => {
    // The router reads `?label=ui&agent=frontend` as two strings and `?label=2025` as a number.
    expect(boardSearchSchema.parse({ label: "ui", agent: "frontend" })).toEqual(
      { label: ["ui"] },
    );
    expect(boardSearchSchema.parse({ label: 2025 }).label).toEqual(["2025"]);
  });

  it("drops a malformed value instead of failing", () => {
    const parsed = boardSearchSchema.parse({
      label: "",
      owner: [3, "", { x: 1 }, "human", "human", "y".repeat(201)],
      blocked: "yes",
      needs: false,
      q: "x".repeat(201),
      other: "kept out",
    });
    expect(parsed).toEqual({
      label: undefined,
      owner: ["3", "human"],
      blocked: undefined,
      needs: undefined,
      q: undefined,
    });
    expect(boardSearchSchema.parse({ label: [] }).label).toBeUndefined();
    expect(boardSearchSchema.parse({ owner: null }).owner).toBeUndefined();
  });

  it("trims names and text", () => {
    expect(boardSearchSchema.parse({ label: ["  ui "] }).label).toEqual(["ui"]);
    expect(boardSearchSchema.parse({ q: " login " }).q).toBe("login");
  });
});

describe("cleanSearch", () => {
  it("removes unset filters so the URL stays clean", () => {
    expect(
      cleanSearch({
        label: [],
        owner: undefined,
        blocked: undefined,
        needs: undefined,
        q: "  ",
      }),
    ).toEqual({});
  });

  it("keeps set filters and trims the query", () => {
    expect(
      cleanSearch({
        label: ["ui", "data"],
        owner: ["main"],
        blocked: true,
        needs: true,
        q: " login ",
      }),
    ).toEqual({
      label: ["ui", "data"],
      owner: ["main"],
      blocked: true,
      needs: true,
      q: "login",
    });
  });
});

describe("toggleName", () => {
  it("ticks an entry, unticks a ticked one and clears an empty list", () => {
    expect(toggleName(undefined, "ui")).toEqual(["ui"]);
    expect(toggleName(["ui"], "data")).toEqual(["ui", "data"]);
    expect(toggleName(["ui", "data"], "ui")).toEqual(["data"]);
    expect(toggleName(["ui"], "ui")).toBeUndefined();
  });
});

describe("isFiltered", () => {
  it("is true only with a set filter", () => {
    expect(isFiltered({})).toBe(false);
    expect(isFiltered({ q: " " })).toBe(false);
    expect(isFiltered({ label: [] })).toBe(false);
    expect(isFiltered({ blocked: true })).toBe(true);
    expect(isFiltered({ owner: ["human"] })).toBe(true);
  });
});
