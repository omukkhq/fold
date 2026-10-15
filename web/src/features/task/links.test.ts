import { describe, expect, it } from "vitest";
import { diffPath, proposalPath, revisionPath, threadPath } from "./links";

describe("links", () => {
  it("opens the diff of a workspace against its base, against main, or at one file", () => {
    expect(diffPath("demo", { to: "ws-2" })).toBe("/p/demo/diff?to=ws-2");
    expect(diffPath("demo", { from: "r8", to: "ws-2" })).toBe(
      "/p/demo/diff?from=r8&to=ws-2",
    );
    expect(diffPath("demo", { to: "ws-2", path: "src/a b.ts" })).toBe(
      "/p/demo/diff?to=ws-2&path=src%2Fa+b.ts",
    );
  });

  it("opens a proposal, a message thread and a revision", () => {
    expect(proposalPath("demo", "P-3")).toBe("/p/demo/proposals/P-3");
    expect(threadPath("demo", "m-41")).toBe("/p/demo/messages?thread=m-41");
    expect(revisionPath("demo", "r8")).toBe("/p/demo/repository?revision=r8");
  });
});
