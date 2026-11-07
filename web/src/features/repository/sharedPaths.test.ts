import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { pathForm, sharedFailure, sharedNote } from "./sharedPaths";

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
      shared: ["design/", "docs/spec.pdf"],
      unseen: null,
      hint: null,
    });
  });

  it("names the ignored paths workers cannot see and counts those it leaves out", () => {
    expect(
      sharedNote({
        shared: [],
        unseen: [".env", "notes/"],
        unseen_count: 3,
      }),
    ).toEqual({
      shared: null,
      unseen: {
        lead: "Workers cannot see 3 ignored paths:",
        paths: [".env", "notes/"],
        more: 1,
      },
      hint: "fld share <path>",
    });
    expect(
      sharedNote({ shared: ["design/"], unseen: [".env"], unseen_count: 1 })
        ?.unseen,
    ).toEqual({
      lead: "Workers cannot see 1 ignored path:",
      paths: [".env"],
      more: 0,
    });
  });

  it("shows a folder with one trailing slash and a file without one", () => {
    expect(pathForm("notes/")).toBe("notes/");
    expect(pathForm("notes//")).toBe("notes/");
    expect(pathForm(".env")).toBe(".env");
    expect(
      sharedNote({ shared: ["design//"], unseen: ["a//"], unseen_count: 1 }),
    ).toMatchObject({ shared: ["design/"], unseen: { paths: ["a/"] } });
  });
});

describe("sharedFailure", () => {
  it("words the failed read by its code", () => {
    expect(sharedFailure(new ApiError("workspace_io", "x"))).toBe(
      "fold could not read .fold/shared or the ignored files of the project folder.",
    );
    expect(sharedFailure(new ApiError("internal_error", "x"))).toBe(
      "The daemon failed. Try again.",
    );
    expect(sharedFailure(new ApiError("project_unavailable", "x"))).toBe(
      "The project is not available now.",
    );
    expect(sharedFailure(new ApiError("other", "x"))).toBe(
      "The daemon refused the request.",
    );
    expect(sharedFailure(new Error("offline"))).toBe(
      "The daemon did not answer.",
    );
  });
});
