import { describe, expect, it } from "vitest";
import { comparedFile } from "@/test/diff";
import { intent } from "@/test/proposals";
import {
  OUTSIDE_INTENT,
  filterGroups,
  groupFiles,
  partGroups,
} from "./partGroups";

const files = [
  comparedFile("src/routes.rs", "modified"),
  comparedFile("src/schemas.rs", "added"),
  comparedFile("tests/routes.rs", "added"),
  comparedFile("src/settings.rs", "modified"),
];

const CASES = intent("Opens a case.", [
  ["Tests", "Covers the routes.", ["tests/routes.rs"]],
  ["Routes", "Opens a case.", ["src/schemas.rs", "src/routes.rs"]],
  ["Docs", "Describes the routes.", ["docs/cases.md"]],
]);

describe("the changed files by part", () => {
  it("groups the files by part in the intent's order, then the files no part names", () => {
    const groups = partGroups(CASES, files);
    expect(
      groups?.map((group) => [
        group.name,
        group.does,
        group.outside,
        group.files.map((file) => file.path),
      ]),
    ).toEqual([
      ["Tests", "Covers the routes.", false, ["tests/routes.rs"]],
      ["Routes", "Opens a case.", false, ["src/schemas.rs", "src/routes.rs"]],
      [OUTSIDE_INTENT, null, true, ["src/settings.rs"]],
    ]);
    expect(groupFiles(groups ?? []).map((file) => file.path)).toEqual([
      "tests/routes.rs",
      "src/schemas.rs",
      "src/routes.rs",
      "src/settings.rs",
    ]);
  });

  it("has no groups without an intent and no warning when every file has its part", () => {
    expect(partGroups(null, files)).toBeNull();
    const covered = partGroups(CASES, files.slice(0, 3));
    expect(covered?.some((group) => group.outside)).toBe(false);
    expect(partGroups(CASES, [])).toEqual([]);
  });

  it("lists a file named twice in its first part only", () => {
    const twice = intent("s", [
      ["A", "a", ["src/routes.rs"]],
      ["B", "b", ["src/routes.rs"]],
    ]);
    expect(
      partGroups(twice, files.slice(0, 1))?.map((group) => group.name),
    ).toEqual(["A"]);
  });

  it("keeps the matching files of each group and leaves out a group without a match", () => {
    const groups = partGroups(CASES, files) ?? [];
    expect(
      filterGroups(groups, "SRC routes").map((group) => [
        group.name,
        group.files.map((file) => file.path),
      ]),
    ).toEqual([["Routes", ["src/routes.rs"]]]);
    expect(filterGroups(groups, "")).toEqual(groups);
  });
});
