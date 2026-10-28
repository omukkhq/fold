import { describe, expect, it } from "vitest";
import { missingState } from "./missingState";

describe("missingState", () => {
  it("names a missing item by its id and leads back to the board", () => {
    expect(missingState("demo", { kind: "task", id: "T-999" })).toEqual({
      title: "Task T-999 not found",
      text: expect.stringContaining("no task with this id") as string,
      code: null,
      href: "/p/demo/board",
      linkText: "Back to board",
    });
    expect(missingState("demo", { kind: "proposal", id: "P-77" }).title).toBe(
      "Proposal P-77 not found",
    );
    expect(missingState("demo", { kind: "request", id: "RQ-x" }).title).toBe(
      "Request RQ-x not found",
    );
  });

  it("leads a missing batch back to the proposals with the daemon's code", () => {
    expect(missingState("demo", { kind: "batch", id: "rb-1" })).toEqual({
      title: "Batch rb-1 not found",
      text: expect.stringContaining("no review batch with this id") as string,
      code: "review_batch_not_found",
      href: "/p/demo/proposals",
      linkText: "Back to proposals",
    });
  });

  it("names a page the project does not have", () => {
    expect(missingState("demo", { kind: "page" })).toEqual({
      title: "Page not found",
      text: expect.stringContaining("no page at this address") as string,
      code: null,
      href: "/p/demo/board",
      linkText: "Back to board",
    });
  });
});
