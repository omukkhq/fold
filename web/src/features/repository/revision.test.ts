import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { Revision } from "@/api/repository";
import { isMissingRevision, pickRevision } from "./revision";

const listed = [{ id: "r8" }, { id: "r7" }] as Revision[];

describe("pickRevision", () => {
  it("shows the newest revision when the address names none", () => {
    expect(pickRevision(listed, undefined)).toEqual({ kind: "newest" });
  });

  it("shows a listed revision from the listing", () => {
    expect(pickRevision(listed, "r7")).toEqual({ kind: "listed", id: "r7" });
  });

  it("reads a revision the listing did not reach on its own", () => {
    expect(pickRevision(listed, "r2")).toEqual({ kind: "unlisted", id: "r2" });
  });
});

describe("isMissingRevision", () => {
  it("reads an unknown and a malformed revision as missing, and nothing else", () => {
    expect(isMissingRevision(new ApiError("revision_not_found", ""))).toBe(
      true,
    );
    expect(isMissingRevision(new ApiError("invalid_id", ""))).toBe(true);
    expect(isMissingRevision(new ApiError("project_unavailable", ""))).toBe(
      false,
    );
    expect(isMissingRevision(new Error("offline"))).toBe(false);
  });
});
