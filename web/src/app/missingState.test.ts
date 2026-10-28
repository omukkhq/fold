import { describe, expect, it } from "vitest";
import { missingState } from "./missingState";

describe("missingState", () => {
  it("names a missing task by its id and leads back to the board", () => {
    expect(missingState("demo", { kind: "task", id: "T-999" })).toEqual({
      title: "Task T-999 not found",
      text: expect.stringContaining("no task with this id") as string,
      href: "/p/demo/board",
      linkText: "Back to board",
    });
  });

  it("leads a missing proposal or request back to its own list", () => {
    expect(missingState("demo", { kind: "proposal", id: "P-77" })).toEqual({
      title: "Proposal P-77 not found",
      text: expect.stringContaining(
        "Open the proposals to find the proposal.",
      ) as string,
      href: "/p/demo/proposals",
      linkText: "Back to proposals",
    });
    expect(missingState("demo", { kind: "request", id: "RQ-x" })).toEqual({
      title: "Request RQ-x not found",
      text: expect.stringContaining(
        "Open the requests to find the request.",
      ) as string,
      href: "/p/demo/requests",
      linkText: "Back to requests",
    });
  });

  it("leads a missing batch back to the proposals", () => {
    expect(missingState("demo", { kind: "batch", id: "rb-1" })).toEqual({
      title: "Batch rb-1 not found",
      text: expect.stringContaining("no review batch with this id") as string,
      href: "/p/demo/proposals",
      linkText: "Back to proposals",
    });
  });

  it("names a page the project does not have", () => {
    expect(missingState("demo", { kind: "page" })).toEqual({
      title: "Page not found",
      text: expect.stringContaining("no page at this address") as string,
      href: "/p/demo/board",
      linkText: "Back to board",
    });
  });
});
