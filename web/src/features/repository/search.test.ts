import { describe, expect, it } from "vitest";
import { parseRepositorySearch } from "./search";

describe("parseRepositorySearch", () => {
  it("keeps a revision id", () => {
    expect(parseRepositorySearch({ revision: "r17" })).toEqual({
      revision: "r17",
    });
  });

  it("ignores anything else", () => {
    for (const revision of [
      "T-4",
      "r0",
      "r04",
      "r1 ",
      "main",
      "",
      17,
      null,
      ["r1"],
    ]) {
      expect(parseRepositorySearch({ revision })).toEqual({});
    }
    expect(parseRepositorySearch({})).toEqual({});
  });
});
