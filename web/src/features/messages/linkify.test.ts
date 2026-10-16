import { describe, expect, it } from "vitest";
import { linkify } from "./linkify";

describe("linkify", () => {
  it("links each kind of id to its screen", () => {
    const segments = linkify("See T-12, P-3, C-40, r17 and m-210.", "demo");
    expect(segments.filter((s) => s.href)).toEqual([
      { text: "T-12", href: "/p/demo/t/T-12" },
      { text: "P-3", href: "/p/demo/proposals/P-3" },
      { text: "C-40", href: "/p/demo/diff?to=C-40" },
      { text: "r17", href: "/p/demo/repository?revision=r17" },
      { text: "m-210", href: "/p/demo/messages?thread=m-210" },
    ]);
  });
  it("links a request id to the request", () => {
    expect(linkify("see RQ-7", "demo")).toEqual([
      { text: "see " },
      { text: "RQ-7", href: "/p/demo/requests/RQ-7" },
    ]);
  });
  it("keeps the text between ids and rebuilds the body", () => {
    const body = "a T-1 b P-2";
    const segments = linkify(body, "demo");
    expect(segments.map((s) => s.text).join("")).toBe(body);
    expect(segments[0]).toEqual({ text: "a " });
  });
  it("leaves ids without a screen as text", () => {
    const segments = linkify("ws-9 and ck-88", "demo");
    expect(segments).toEqual([{ text: "ws-9 and ck-88" }]);
  });
  it("returns a body without ids as one text segment, and an empty body as none", () => {
    expect(linkify("no ids here", "demo")).toEqual([{ text: "no ids here" }]);
    expect(linkify("", "demo")).toEqual([]);
  });
  it("keeps HTML as text and never makes it a link", () => {
    const body = '<a href="javascript:alert(1)">x</a> <b>T-5</b>';
    const segments = linkify(body, "demo");
    expect(segments.map((s) => s.text).join("")).toBe(body);
    expect(segments.filter((s) => s.href)).toEqual([
      { text: "T-5", href: "/p/demo/t/T-5" },
    ]);
  });
  it("does not link an id inside a longer word", () => {
    expect(linkify("xT-12 T-12a", "demo").some((s) => s.href)).toBe(false);
  });
});
