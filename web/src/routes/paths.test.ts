import { describe, expect, it } from "vitest";
import { idHref, projectPath, withQuery } from "./paths";

describe("projectPath", () => {
  it("joins the slug and the segments below /p", () => {
    expect(projectPath("demo")).toBe("/p/demo");
    expect(projectPath("demo", "proposals", "P-3")).toBe(
      "/p/demo/proposals/P-3",
    );
  });

  it("encodes each segment, so a slash or a space stays inside one", () => {
    expect(projectPath("my app", "t", "a/b")).toBe("/p/my%20app/t/a%2Fb");
  });
});

describe("withQuery", () => {
  it("adds the query only when it has a parameter", () => {
    expect(withQuery("/p/demo/board", { agent: "backend" })).toBe(
      "/p/demo/board?agent=backend",
    );
    expect(withQuery("/p/demo/board", new URLSearchParams())).toBe(
      "/p/demo/board",
    );
    expect(withQuery("/p/demo/board", {})).toBe("/p/demo/board");
  });
});

describe("idHref", () => {
  it("opens every kind of public id on its screen", () => {
    expect(idHref("demo", "task", "T-12")).toBe("/p/demo/t/T-12");
    expect(idHref("demo", "proposal", "P-3")).toBe("/p/demo/proposals/P-3");
    expect(idHref("demo", "request", "RQ-7")).toBe("/p/demo/requests/RQ-7");
    expect(idHref("demo", "revision", "r17")).toBe(
      "/p/demo/repository?revision=r17",
    );
    expect(idHref("demo", "message", "m-210")).toBe(
      "/p/demo/messages?thread=m-210",
    );
    for (const kind of ["changeset", "workspace", "checkpoint"] as const) {
      expect(idHref("demo", kind, "X-1")).toBe("/p/demo/diff?to=X-1");
    }
  });
});
