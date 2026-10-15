import { describe, expect, it } from "vitest";
import { DEFAULT_LANDING, safeRedirect, signInHref } from "./redirect";

describe("safeRedirect", () => {
  it("keeps a same-origin path with its query and hash", () => {
    expect(safeRedirect("/p/demo/board")).toBe("/p/demo/board");
    expect(safeRedirect("/p/demo/diff?from=r1&to=r2#x")).toBe(
      "/p/demo/diff?from=r1&to=r2#x",
    );
  });

  it.each([
    "//evil.example",
    "//evil.example/path",
    "https://evil.example",
    "http://evil.example/p/demo",
    "javascript:alert(1)",
    "/\\evil.example",
    "/\t/evil.example",
    "/\n/evil.example",
    "evil.example",
    "p/demo",
    "",
    "data:text/html,x",
  ])("rejects %j", (target) => {
    expect(safeRedirect(target)).toBe(DEFAULT_LANDING);
  });

  it("falls back for a missing target", () => {
    expect(safeRedirect(undefined)).toBe(DEFAULT_LANDING);
    expect(safeRedirect(null)).toBe(DEFAULT_LANDING);
  });

  it("never returns to the sign-in screen", () => {
    expect(safeRedirect("/sign-in")).toBe(DEFAULT_LANDING);
    expect(safeRedirect("/sign-in?redirect=%2Fprojects")).toBe(DEFAULT_LANDING);
  });
});

describe("signInHref", () => {
  it("encodes the target so it survives as one parameter", () => {
    const href = signInHref("/p/demo/diff?from=r1&to=r2");
    expect(href).toBe(
      "/sign-in?redirect=%2Fp%2Fdemo%2Fdiff%3Ffrom%3Dr1%26to%3Dr2",
    );
    const target = new URL(href, "http://fold.test").searchParams.get(
      "redirect",
    );
    expect(safeRedirect(target)).toBe("/p/demo/diff?from=r1&to=r2");
  });
});
