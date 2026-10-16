import { describe, expect, it } from "vitest";
import { keys } from "./keys";
import { fileQuery, revisionsQuery, treeQuery } from "./repositoryQueries";

describe("the repository queries", () => {
  it("keeps the trees and files of a revision outside the repository prefix and fresh for good", () => {
    const tree = treeQuery("demo", "r3", "src");
    const file = fileQuery("demo", "r3", "src/a.ts");
    expect(tree.queryKey).toEqual(keys.revisionTree("demo", "r3", "src"));
    expect(file.queryKey).toEqual(keys.revisionFile("demo", "r3", "src/a.ts"));
    for (const query of [tree, file]) {
      expect(query.staleTime).toBe(Infinity);
      expect(query.queryKey.slice(0, 2)).not.toEqual(keys.repository("demo"));
    }
    expect(revisionsQuery("demo").queryKey.slice(0, 2)).toEqual(
      keys.repository("demo"),
    );
  });
});
