import { describe, expect, it } from "vitest";
import { sharedNote } from "./sharedPaths";

describe("sharedNote", () => {
  it("says nothing until the read arrives or when nothing is shared or hidden", () => {
    expect(sharedNote(undefined)).toBeNull();
    expect(sharedNote({ shared: [], unseen: [], unseen_count: 0 })).toBeNull();
  });

  it("names the shared paths alone when workers miss nothing", () => {
    expect(
      sharedNote({
        shared: ["design/", "docs/spec.pdf"],
        unseen: [],
        unseen_count: 0,
      }),
    ).toEqual({
      shared: "Shared read-only with workers: design/, docs/spec.pdf.",
      unseen: null,
      hint: null,
    });
  });

  it("names the ignored paths workers cannot see and counts those it leaves out", () => {
    expect(
      sharedNote({
        shared: [],
        unseen: [".env", "notes.md"],
        unseen_count: 3,
      }),
    ).toEqual({
      shared: null,
      unseen: "Workers cannot see 3 ignored paths: .env, notes.md and 1 more.",
      hint: "fld share <path>",
    });
    expect(
      sharedNote({ shared: ["design/"], unseen: [".env"], unseen_count: 1 })
        ?.unseen,
    ).toBe("Workers cannot see 1 ignored path: .env.");
  });
});
